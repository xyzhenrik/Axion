//! Reader view and page translation in the browser: what the address field's two buttons show,
//! their popovers, the per-tab state, and the glue to the page scripts, the translation
//! backends and the hidden engine page. The models and protocols live in `axion-reader` and
//! `axion-translate`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axion_config::{ReaderTheme, TranslationBackend};
use axion_reader::Look;
use axion_translate::endpoint::EndpointTranslator;
use axion_translate::local::{self, LocalTranslator};
use axion_translate::models::{ModelManager, ModelStore, Phase};
use axion_translate::{Job, Outcome, Translator, helper, lang, page};
use axion_ui::ActionId;

use super::*;
use crate::reader_popover::{Action as ReaderAction, ReaderPopover};
use crate::translate_popover::{Action as TranslateAction, Content, TranslatePopover};

/// The site setting that remembers Reader view's look.
const READER_KEY: &str = "reader";

/// Where translating a tab's page stands.
#[derive(Clone, Debug, PartialEq)]
enum Translating {
    Idle,
    /// Waiting for the models (consent, download).
    Models {
        from: String,
        to: String,
    },
    Running {
        from: String,
        to: String,
        percent: u32,
    },
    Translated {
        from: String,
        to: String,
    },
    Failed(String),
}

struct PageTools {
    readable: bool,
    reader_active: bool,
    look: Option<Look>,
    /// The page's language as it said or as guessed (normalised), and whether the offer was
    /// already shown for this document.
    lang: Option<String>,
    offered: bool,
    translating: Translating,
}

impl Default for PageTools {
    fn default() -> Self {
        Self {
            readable: false,
            reader_active: false,
            look: None,
            lang: None,
            offered: false,
            translating: Translating::Idle,
        }
    }
}

pub(super) struct ReadingUi {
    reader_popover: ReaderPopover,
    translate_popover: TranslatePopover,
    pages: HashMap<ViewId, PageTools>,
    local: LocalTranslator,
    endpoint: Option<EndpointTranslator>,
    endpoint_for: (String, String),
    models: ModelManager,
    /// The hidden page that runs the local engine, and when it last had work.
    helper: Option<ViewId>,
    helper_busy: Instant,
    /// Batches in flight: job tag → (view, batch id).
    jobs: HashMap<u64, (ViewId, u32)>,
    next_tag: u64,
    shown_reader: Option<Option<bool>>,
    shown_translate: Option<Option<bool>>,
    last_dark: bool,
    /// Capture aid (`--open=translate-auto`): agrees to the download by itself.
    auto_consent: bool,
}

impl ReadingUi {
    pub fn build(
        ui: &mut Ui,
        bar: axion_ui::taffy::NodeId,
        profile: &std::path::Path,
        config: &Config,
    ) -> Self {
        let store = ModelStore::new(profile.join("translations"));
        Self {
            reader_popover: ReaderPopover::build(ui, bar),
            translate_popover: TranslatePopover::build(ui, bar),
            pages: HashMap::new(),
            local: LocalTranslator::new(),
            endpoint: EndpointTranslator::new(&config.translation),
            endpoint_for: (
                config.translation.endpoint_url.clone(),
                config.translation.endpoint_api_key.clone(),
            ),
            models: ModelManager::new(store, config.translation.clone()),
            helper: None,
            helper_busy: Instant::now(),
            jobs: HashMap::new(),
            next_tag: 1,
            shown_reader: None,
            shown_translate: None,
            last_dark: false,
            auto_consent: false,
        }
    }

    /// The service that serves the engine page.
    pub fn engine_pages(&self) -> Arc<dyn axion_engine::InternalPages> {
        Arc::new(helper::HelperService::new(self.models.store().clone()))
    }

    pub fn popovers_open(&self) -> bool {
        self.reader_popover.is_open() || self.translate_popover.is_open()
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.reader_popover.contains(ui, p) || self.translate_popover.contains(ui, p)
    }

    pub fn close_popovers(&mut self, ui: &mut Ui, config: &Config) {
        self.reader_popover.close(ui, config);
        self.translate_popover.close(ui, config);
    }

    pub fn reader_action(&self, id: ActionId) -> Option<ReaderAction> {
        self.reader_popover.action(id)
    }

    pub fn translate_action(&self, id: ActionId) -> Option<TranslateAction> {
        self.translate_popover.action(id)
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.reader_popover.animate(ui, dt);
        self.translate_popover.animate(ui, dt);
    }
}

/// The host a site setting is kept under.
fn site_of(url: &str) -> Option<String> {
    if url.starts_with("file://") {
        return Some("file".to_owned());
    }
    let host = crate::url::display_host(url);
    (url.starts_with("http://") || url.starts_with("https://")).then_some(host).filter(|h| !h.is_empty())
}

/// Whether `host` is one of `sites` or a subdomain of one.
fn site_listed(sites: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    sites.iter().any(|s| {
        let s = s.trim().to_ascii_lowercase();
        !s.is_empty() && (host == s || host.ends_with(&format!(".{s}")))
    })
}

impl Browser {
    // ---- messages from pages ----

    /// A console message that belongs to Reader view or translation. Returns whether it did.
    pub(super) fn reading_message(&mut self, view: ViewId, message: &str) -> bool {
        if message.starts_with(axion_reader::MARKER) {
            match axion_reader::parse(message) {
                Some(axion_reader::Message::Probe { readable, active }) => {
                    let tools = self.reading.pages.entry(view).or_default();
                    tools.readable = readable;
                    tools.reader_active = active;
                }
                Some(axion_reader::Message::State { active }) => {
                    self.reading.pages.entry(view).or_default().reader_active = active;
                    if !active {
                        self.reading.reader_popover.close(&mut self.ui, &self.config);
                    }
                }
                None => {}
            }
            return true;
        }
        if message.starts_with(page::MARKER) {
            if let Some(message) = page::parse(message) {
                self.translate_page_message(view, message);
            }
            return true;
        }
        if message.starts_with(local::MARKER) {
            if self.reading.helper == Some(view)
                && let Some(message) = local::parse(message)
            {
                self.reading.local.on_message(message);
            }
            return true;
        }
        false
    }

    fn translate_page_message(&mut self, view: ViewId, message: page::Message) {
        match message {
            page::Message::Lang { lang, .. } => {
                let tools = self.reading.pages.entry(view).or_default();
                tools.lang = lang::normalize(&lang);
                tools.offered = false;
            }
            page::Message::Batch { id, texts } => {
                let Some(Translating::Running { from, to, .. }) =
                    self.reading.pages.get(&view).map(|t| t.translating.clone())
                else {
                    return;
                };
                let tag = self.reading.next_tag;
                self.reading.next_tag += 1;
                self.reading.jobs.insert(tag, (view, id));
                self.reading.helper_busy = Instant::now();
                let job = Job { tag, from, to, texts };
                self.translator().submit(job);
            }
            page::Message::Progress { done, total } => {
                let Some(tools) = self.reading.pages.get_mut(&view) else { return };
                if let Translating::Running { from, to, percent } = &mut tools.translating {
                    *percent = (done * 100).checked_div(total).map_or(100, |p| p.min(100));
                    if total > 0 && done >= total {
                        tools.translating = Translating::Translated { from: from.clone(), to: to.clone() };
                    }
                }
            }
            page::Message::State { active } => {
                if !active && let Some(tools) = self.reading.pages.get_mut(&view) {
                    tools.translating = Translating::Idle;
                }
            }
        }
    }

    /// The backend the settings choose (the endpoint when it is chosen and configured).
    fn translator(&mut self) -> &mut dyn Translator {
        let t = &self.config.translation;
        let wanted = (t.endpoint_url.clone(), t.endpoint_api_key.clone());
        if self.reading.endpoint_for != wanted {
            self.reading.endpoint = EndpointTranslator::new(t);
            self.reading.endpoint_for = wanted;
        }
        if self.config.translation.backend == TranslationBackend::Endpoint
            && let Some(endpoint) = &mut self.reading.endpoint
        {
            return endpoint;
        }
        &mut self.reading.local
    }

    fn uses_endpoint(&self) -> bool {
        self.config.translation.backend == TranslationBackend::Endpoint && self.reading.endpoint.is_some()
    }

    /// The tab was navigated to a new document: what was known about the old one is gone.
    pub(super) fn reading_navigated(&mut self, view: ViewId) {
        if let Some(tools) = self.reading.pages.get_mut(&view) {
            *tools = PageTools::default();
        }
        self.reading.jobs.retain(|_, (v, _)| *v != view);
        if self.active_view() == Some(view) {
            self.reading.close_popovers(&mut self.ui, &self.config);
        }
    }

    pub(super) fn reading_view_closed(&mut self, view: ViewId) {
        if self.reading.helper == Some(view) {
            self.reading.helper = None;
            self.reading.local.helper_lost();
        }
        self.reading.pages.remove(&view);
        self.reading.jobs.retain(|_, (v, _)| *v != view);
    }

    // ---- Reader view ----

    fn reader_site(&self) -> Option<String> {
        self.tabs.active().and_then(|t| site_of(&t.url))
    }

    /// The look for `site`: its own when remembered, else the settings'. And whether it is the
    /// site's own.
    fn reader_look(&self, site: Option<&str>) -> (Look, bool) {
        let global = Look::from_config(&self.config.reader);
        if !self.config.reader.remember_per_site {
            return (global, false);
        }
        let own = site.and_then(|site| {
            let store = self.store.as_ref()?;
            let rows = store.site_settings(READER_KEY).ok()?;
            let (_, value) = rows.into_iter().find(|(host, _)| host == site)?;
            Look::decode(&value)
        });
        (own.unwrap_or(global), self.config.reader.remember_per_site && self.store.is_some())
    }

    fn reader_enabled_here(&self) -> bool {
        self.config.reader.button && !self.is_locked() && !self.playlist_open()
    }

    pub(super) fn toggle_reader(&mut self) {
        let Some(view) = self.active_view() else { return };
        let tools = self.reading.pages.entry(view).or_default();
        if tools.reader_active {
            self.engine.run_script(view, axion_reader::exit_script(), false);
            self.reading.reader_popover.close(&mut self.ui, &self.config);
        } else if tools.readable {
            let (look, _) = self.reader_look(self.reader_site().as_deref());
            let dark = self.dark;
            self.engine.run_script(view, &axion_reader::enter_script(&look, dark), false);
            if let Some(tools) = self.reading.pages.get_mut(&view) {
                tools.look = Some(look);
            }
        } else {
            self.show_toast(Icon::TextSize, tr("This page has no article to show in Reader View"));
        }
    }

    /// The Reader button: enters Reader view, and once in it opens the Aa menu.
    pub(super) fn reader_button_clicked(&mut self) {
        let Some(view) = self.active_view() else { return };
        let active = self.reading.pages.get(&view).is_some_and(|t| t.reader_active);
        if !active {
            self.close_popups();
            self.toggle_reader();
        } else if self.reading.reader_popover.is_open() {
            self.reading.reader_popover.close(&mut self.ui, &self.config);
        } else {
            self.close_popups();
            self.open_reader_popover();
        }
    }

    /// Capture aid (`--open=translate-auto`): translates, agreeing to any download.
    pub(super) fn capture_translate_auto(&mut self) {
        self.reading.auto_consent = true;
        self.translate_page_action();
    }

    /// Capture aid (`--open=reader-popover`): Reader view, with its Aa menu open.
    pub(super) fn capture_reader_popover(&mut self) {
        let Some(view) = self.active_view() else { return };
        if !self.reading.pages.get(&view).is_some_and(|t| t.reader_active) {
            self.toggle_reader();
        }
        self.open_reader_popover();
    }

    pub(super) fn open_reader_popover(&mut self) {
        let Some(view) = self.active_view() else { return };
        let site = self.reader_site();
        let (stored, per_site) = self.reader_look(site.as_deref());
        let look = self.reading.pages.get(&view).and_then(|t| t.look).unwrap_or(stored);
        let anchor = self.chrome.reader_button_frame(&self.ui);
        let width = self.logical_size().x;
        self.reading.reader_popover.open(&mut self.ui, &self.config, anchor, width, &look, per_site);
    }

    pub(super) fn reader_popover_action(&mut self, action: ReaderAction) {
        let Some(view) = self.active_view() else { return };
        if action == ReaderAction::Exit {
            self.toggle_reader();
            return;
        }
        let r = self.config.reader.clone();
        let site = self.reader_site();
        let (stored, per_site) = self.reader_look(site.as_deref());
        let mut look = self.reading.pages.get(&view).and_then(|t| t.look).unwrap_or(stored);
        match action {
            ReaderAction::SmallerText => look.size = (look.size - r.font_step).max(r.min_font_size),
            ReaderAction::LargerText => look.size = (look.size + r.font_step).min(r.max_font_size),
            ReaderAction::NarrowerColumn => look.width = (look.width - r.width_step).max(r.min_width),
            ReaderAction::WiderColumn => look.width = (look.width + r.width_step).min(r.max_width),
            ReaderAction::Font(font) => look.font = font,
            ReaderAction::Theme(theme) => look.theme = theme,
            ReaderAction::Exit => {}
        }
        if let Some(tools) = self.reading.pages.get_mut(&view) {
            tools.look = Some(look);
        }
        // Remembered for the site, else for every site (the settings).
        match (per_site, &self.store, &site) {
            (true, Some(store), Some(site)) => {
                if let Err(e) = store.set_site_setting(site, READER_KEY, Some(&look.encode())) {
                    tracing::warn!("remembering the Reader look failed: {e}");
                }
            }
            _ => {
                let r = &mut self.config.reader;
                (r.font, r.font_size, r.line_height, r.width, r.theme) =
                    (look.font, look.size, look.line_height, look.width, look.theme);
                self.config_dirty_since.get_or_insert(Instant::now());
            }
        }
        self.engine.run_script(view, &axion_reader::apply_script(&look, self.dark), false);
        self.open_reader_popover();
    }

    // ---- translation ----

    /// The language pages are translated into.
    fn translate_target(&self) -> String {
        lang::normalize(&self.config.translation.target_language).filter(|l| !l.is_empty()).unwrap_or_else(
            || {
                match axion_i18n::language() {
                    axion_i18n::Language::German => "de",
                    axion_i18n::Language::English => "en",
                }
                .to_owned()
            },
        )
    }

    /// Whether `tab` is worth offering a translation: its language differs from the target and
    /// the backend can translate it.
    fn translation_offered(&self, view: ViewId, url: &str) -> Option<(String, String)> {
        let t = &self.config.translation;
        let from = self.reading.pages.get(&view)?.lang.clone()?;
        let to = self.translate_target();
        let host = site_of(url)?;
        let can = if self.uses_endpoint() { from != to } else { lang::route(&from, &to).is_some() };
        let known = t.known_languages.iter().filter_map(|l| lang::normalize(l)).any(|l| l == from);
        (t.offer && can && !known && !site_listed(&t.never_sites, &host)).then_some((from, to))
    }

    fn language_name(code: &str) -> String {
        lang::english_name(code).map_or_else(|| code.to_owned(), |n| tr(n).to_owned())
    }

    fn translate_content(&self, view: ViewId) -> Option<Content> {
        let tools = self.reading.pages.get(&view)?;
        let name = |c: &String| Self::language_name(c);
        Some(match &tools.translating {
            Translating::Idle => {
                let from = tools.lang.clone()?;
                Content::Offer { from: name(&from), to: name(&self.translate_target()) }
            }
            Translating::Models { from, to } => match self.reading.models.phase() {
                Phase::Idle | Phase::Planning | Phase::Ready => Content::Planning,
                Phase::NeedsConsent { bytes } => Content::Consent { from: name(from), to: name(to), bytes },
                Phase::Downloading { done, total } => Content::Downloading { done, total },
                Phase::Failed(reason) => Content::Failed { reason },
            },
            Translating::Running { from, percent, .. } => {
                Content::Translating { from: name(from), percent: *percent }
            }
            Translating::Translated { from, to } => Content::Translated { from: name(from), to: name(to) },
            Translating::Failed(reason) => Content::Failed { reason: reason.clone() },
        })
    }

    pub(super) fn translate_button_clicked(&mut self) {
        if self.reading.translate_popover.is_open() {
            self.reading.translate_popover.close(&mut self.ui, &self.config);
        } else {
            self.close_popups();
            self.open_translate_popover();
        }
    }

    fn open_translate_popover(&mut self) {
        let Some(view) = self.active_view() else { return };
        let Some(content) = self.translate_content(view) else { return };
        let anchor = self.chrome.translate_button_frame(&self.ui);
        let width = self.logical_size().x;
        self.reading.translate_popover.open(&mut self.ui, &self.config, anchor, width, &content);
    }

    /// The keymap's `translate_page`: translates, or shows the original again.
    pub(super) fn translate_page_action(&mut self) {
        let Some(view) = self.active_view() else { return };
        let translated = matches!(
            self.reading.pages.get(&view).map(|t| &t.translating),
            Some(Translating::Running { .. } | Translating::Translated { .. })
        );
        if translated {
            self.translate_show_original(view);
            return;
        }
        let known_language = self.reading.pages.get(&view).is_some_and(|t| t.lang.is_some());
        if !known_language {
            self.show_toast(Icon::Translate, tr("Couldn’t tell which language this page is in"));
            return;
        }
        self.translate_start(view);
        self.close_popups();
        self.open_translate_popover();
    }

    fn translate_start(&mut self, view: ViewId) {
        let Some(url) = find_tab(&self.tabs, &self.parked, view).map(|t| t.url.clone()) else { return };
        let Some(from) = self.reading.pages.get(&view).and_then(|t| t.lang.clone()) else { return };
        let to = self.translate_target();
        let supported = if self.uses_endpoint() { from != to } else { lang::route(&from, &to).is_some() };
        let _ = url;
        let endpoint_ready = self.uses_endpoint();
        let wants_endpoint = self.config.translation.backend == TranslationBackend::Endpoint;
        let Some(tools) = self.reading.pages.get_mut(&view) else { return };
        if !supported {
            tools.translating = Translating::Failed(tr("This language isn’t available.").to_owned());
            return;
        }
        if wants_endpoint && !endpoint_ready {
            tools.translating = Translating::Failed(
                tr("No translation server is set up. Set translation.endpoint_url in the settings file.")
                    .to_owned(),
            );
            return;
        }
        if endpoint_ready {
            tools.translating = Translating::Running { from: from.clone(), to: to.clone(), percent: 0 };
            let script = page::start_script(&self.config.translation, &to);
            self.engine.run_script(view, &script, false);
        } else {
            tools.translating = Translating::Models { from: from.clone(), to: to.clone() };
            self.reading.models.set_settings(self.config.translation.clone());
            self.reading.models.request(&from, &to);
        }
    }

    fn translate_show_original(&mut self, view: ViewId) {
        self.engine.run_script(view, page::stop_script(), false);
        self.reading.jobs.retain(|_, (v, _)| *v != view);
        if let Some(tools) = self.reading.pages.get_mut(&view) {
            tools.translating = Translating::Idle;
        }
    }

    pub(super) fn translate_popover_action(&mut self, action: TranslateAction) {
        let Some(view) = self.active_view() else { return };
        match action {
            TranslateAction::Translate => self.translate_start(view),
            TranslateAction::Download => self.reading.models.consent(),
            TranslateAction::Cancel => {
                self.reading.models.cancel();
                if let Some(tools) = self.reading.pages.get_mut(&view)
                    && matches!(tools.translating, Translating::Models { .. })
                {
                    tools.translating = Translating::Idle;
                }
                self.reading.translate_popover.close(&mut self.ui, &self.config);
                return;
            }
            TranslateAction::ShowOriginal => self.translate_show_original(view),
            TranslateAction::NeverThisSite => {
                if let Some(host) = self.tabs.active().and_then(|t| site_of(&t.url)) {
                    if !site_listed(&self.config.translation.never_sites, &host) {
                        self.config.translation.never_sites.push(host);
                        self.config_dirty_since.get_or_insert(Instant::now());
                    }
                    self.translate_show_original(view);
                    self.reading.translate_popover.close(&mut self.ui, &self.config);
                    self.show_toast(Icon::Translate, tr("Won’t offer to translate this site"));
                }
                return;
            }
            TranslateAction::OpenSettings => {
                self.reading.translate_popover.close(&mut self.ui, &self.config);
                self.settings.set_category(Category::Reading);
                self.run(Action::OpenSettings);
                return;
            }
        }
        self.open_translate_popover();
    }

    /// Removes the downloaded translation models (Settings).
    pub(super) fn remove_translation_models(&mut self) {
        self.reading.models.cancel();
        if let Err(e) = self.reading.models.store().remove_all() {
            tracing::warn!("removing the translation models failed: {e}");
        }
        for tools in self.reading.pages.values_mut() {
            if matches!(tools.translating, Translating::Models { .. }) {
                tools.translating = Translating::Idle;
            }
        }
    }

    // ---- every frame ----

    /// Keeps the buttons, popovers, backends and the engine page in step (called every frame).
    pub(super) fn update_reading(&mut self) {
        // Pages that are gone.
        if self.reading.pages.len() > 64 {
            let tabs: Vec<ViewId> = self.tabs.all().iter().chain(self.parked.all()).map(|t| t.view).collect();
            self.reading.pages.retain(|v, _| tabs.contains(v));
        }
        let active = self.active_view().filter(|_| !self.is_locked());

        // Reader view follows the chrome's colours while its theme is Auto.
        if self.reading.last_dark != self.dark {
            self.reading.last_dark = self.dark;
            for (view, tools) in &self.reading.pages {
                if let (true, Some(look)) = (tools.reader_active, tools.look)
                    && look.theme == ReaderTheme::Auto
                {
                    self.engine.run_script(*view, &axion_reader::apply_script(&look, self.dark), false);
                }
            }
        }

        // The Reader button.
        let reader_state = active.and_then(|view| {
            let tools = self.reading.pages.get(&view)?;
            (self.reader_enabled_here() && (tools.readable || tools.reader_active))
                .then_some(tools.reader_active)
        });
        if Some(reader_state) != self.reading.shown_reader {
            self.reading.shown_reader = Some(reader_state);
            self.chrome.set_reader_button(&mut self.ui, reader_state);
            if reader_state != Some(true) {
                self.reading.reader_popover.close(&mut self.ui, &self.config);
            }
        }

        // Translation: models, backends, results.
        self.update_translation(active);

        // The Translate button and its popover.
        let translate_state = active.and_then(|view| {
            let tab = find_tab(&self.tabs, &self.parked, view)?;
            let tools = self.reading.pages.get(&view)?;
            let busy = tools.translating != Translating::Idle;
            let offered = self.translation_offered(view, &tab.url).is_some();
            (busy || offered).then_some(matches!(
                tools.translating,
                Translating::Running { .. } | Translating::Translated { .. }
            ))
        });
        // A button shown for a frame has a place in the layout (the offer opens at it).
        let button_placed = self.reading.shown_translate == Some(translate_state);
        if Some(translate_state) != self.reading.shown_translate {
            self.reading.shown_translate = Some(translate_state);
            self.chrome.set_translate_button(&mut self.ui, translate_state);
            if translate_state.is_none() {
                self.reading.translate_popover.close(&mut self.ui, &self.config);
            }
        }
        if let Some(view) = active {
            // The offer opens by itself once per document.
            if self.config.translation.prompt
                && translate_state == Some(false)
                && button_placed
                && !self.popup_open()
                && let Some(tab) = find_tab(&self.tabs, &self.parked, view)
                && !tab.loading
                && let Some(tools) = self.reading.pages.get(&view)
                && !tools.offered
                && tools.translating == Translating::Idle
            {
                if let Some(tools) = self.reading.pages.get_mut(&view) {
                    tools.offered = true;
                }
                self.open_translate_popover();
            }
            if self.reading.translate_popover.is_open()
                && let Some(content) = self.translate_content(view)
            {
                let anchor = self.chrome.translate_button_frame(&self.ui);
                let width = self.logical_size().x;
                self.reading.translate_popover.update(&mut self.ui, &self.config, anchor, width, &content);
            }
        }
    }

    fn popup_open(&self) -> bool {
        self.reading.popovers_open()
            || self.shields_popover.is_open()
            || self.menu.is_open()
            || self.playlist_ui.popover.is_open()
            || self.ui.focused() == Some(self.chrome.address_field)
    }

    fn update_translation(&mut self, active: Option<ViewId>) {
        // Models that arrived start the waiting tabs.
        if matches!(self.reading.models.phase(), Phase::Ready) {
            let waiting: Vec<(ViewId, String, String)> = self
                .reading
                .pages
                .iter()
                .filter_map(|(v, t)| match &t.translating {
                    Translating::Models { from, to } => Some((*v, from.clone(), to.clone())),
                    _ => None,
                })
                .collect();
            for (view, from, to) in waiting {
                if self.reading.models.store().ready(&from, &to) {
                    if let Some(tools) = self.reading.pages.get_mut(&view) {
                        tools.translating = Translating::Running { from, to: to.clone(), percent: 0 };
                    }
                    let script = page::start_script(&self.config.translation, &to);
                    self.engine.run_script(view, &script, false);
                }
            }
        } else if let Phase::Failed(reason) = self.reading.models.phase() {
            for tools in self.reading.pages.values_mut() {
                if matches!(tools.translating, Translating::Models { .. }) {
                    tools.translating = Translating::Failed(reason.clone());
                }
            }
        }
        let _ = active;
        if self.reading.auto_consent && matches!(self.reading.models.phase(), Phase::NeedsConsent { .. }) {
            self.reading.models.consent();
        }

        // The engine page lives while there is work, and a while after.
        if self.reading.local.wants_helper() {
            self.reading.helper_busy = Instant::now();
            if self.reading.helper.is_none() && !self.uses_endpoint() {
                let size = ViewSize { logical: Vec2::new(64.0, 64.0), scale: self.scale() };
                match self.engine.create_view(helper::URL, size, false) {
                    Ok(view) => {
                        self.engine.set_audio_muted(view, true);
                        self.reading.helper = Some(view);
                    }
                    Err(e) => tracing::error!("starting the translation engine failed: {e:#}"),
                }
            }
        }
        if let Some(helper) = self.reading.helper {
            for script in self.reading.local.take_scripts() {
                self.engine.run_script(helper, &script, false);
            }
            let idle = Duration::from_secs(u64::from(self.config.translation.engine_idle_seconds.max(1)));
            if !self.reading.local.wants_helper() && self.reading.helper_busy.elapsed() > idle {
                self.engine.close_view(helper);
                self.reading.helper = None;
                self.reading.local.helper_lost();
            }
        }

        // Answers.
        let outcomes = {
            let mut all = self.reading.local.poll();
            if let Some(endpoint) = &mut self.reading.endpoint {
                all.extend(endpoint.poll());
            }
            all
        };
        for outcome in outcomes {
            let Some((view, batch)) = self.reading.jobs.remove(&outcome.tag()) else { continue };
            match outcome {
                Outcome::Done { texts, .. } => {
                    self.engine.run_script(view, &page::apply_script(batch, &texts), false);
                }
                Outcome::Failed { reason, .. } => {
                    tracing::warn!(%reason, "translation failed");
                    self.engine.run_script(view, &page::fail_script(batch), false);
                    self.engine.run_script(view, page::stop_script(), false);
                    self.reading.jobs.retain(|_, (v, _)| *v != view);
                    if let Some(tools) = self.reading.pages.get_mut(&view) {
                        tools.translating = Translating::Failed(reason);
                    }
                }
            }
        }
    }
}
