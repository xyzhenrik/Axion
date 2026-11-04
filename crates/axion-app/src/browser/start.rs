//! The browser's side of the native start page (`start_page`): when it is shown instead of the
//! tab's web page, the model it shows, the wallpaper under it, what its buttons do, and its
//! keyboard.
//!
//! The tab keeps its web view navigated to `axion://home/` (URL, history, session restore and
//! the discard/freeze lifecycle stay the engine's); while the native page is shown that view is
//! hidden and not drawn. Where that changes behaviour (zoom, find, printing, the overview's
//! cards, navigation timing) is noted at the place that handles it.

use std::collections::HashMap;

use super::*;
use crate::home::{HomeCommand, SiteEdit, StartModel, Widget as HomeWidget};
use crate::start_page::{Focus, StartAction, StartPage};
use crate::start_wallpaper::{self, Wallpaper};

/// What the page's model was last built from; a change builds it again.
#[derive(Clone, PartialEq)]
struct Inputs {
    view: ViewId,
    home: axion_config::Home,
    search_url: String,
    name: String,
    private: bool,
    accent: axion_config::Color,
    reduce_motion: bool,
    flat: bool,
    language: u32,
    icons: u64,
    backgrounds: u64,
    blocked: u64,
    add_error: Option<String>,
}

/// A tab left the native page for a site: the page stays up until the site's first frame.
struct Handoff {
    view: ViewId,
    serial: u64,
    since: Instant,
}

pub(super) struct StartUi {
    pub page: StartPage,
    wallpaper: Wallpaper,
    inputs: Option<Inputs>,
    /// The view this page hid (the active tab's, while it shows the start page).
    pub hidden_view: Option<ViewId>,
    /// The page each view last committed (the engine's word, not the optimistic `Tab::url`),
    /// so a tab that was sent elsewhere keeps showing the start page until the site commits.
    pub engine_urls: HashMap<ViewId, String>,
    handoff: Option<Handoff>,
    time: Option<(u32, u32)>,
    blocked: (u64, Option<Instant>),
    shown_view: Option<ViewId>,
    pub stale: bool,
    gpu: bool,
    engine_icons_asked: bool,
}

impl StartUi {
    pub fn build(ui: &mut Ui, config: &Config, device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let page = StartPage::build(ui, config);
        // The page lies under the chrome's planes (the sidebar, the toolbar), like a web page:
        // the chrome's glass refracts it.
        let root = ui.root();
        let mut planes = ui.children(root);
        let own = page.planes();
        planes.retain(|n| !own.contains(n));
        planes.splice(0..0, own);
        ui.set_children(root, &planes);
        Self {
            page,
            wallpaper: Wallpaper::new(device, queue),
            inputs: None,
            hidden_view: None,
            engine_urls: HashMap::new(),
            handoff: None,
            time: None,
            blocked: (0, None),
            shown_view: None,
            stale: true,
            gpu: false,
            engine_icons_asked: false,
        }
    }
}

/// `axion://home/?html=1` always opens the HTML page.
fn wants_html(url: &str) -> bool {
    url.split_once('?').is_some_and(|(_, q)| q.split('&').any(|p| p == "html=1"))
}

impl Browser {
    /// Whether the active tab is showing the start page and `home.native` draws it.
    fn start_home_view(&self) -> Option<ViewId> {
        if !self.config.home.native || self.is_locked() {
            return None;
        }
        let tab = self.tabs.active()?;
        let url = self.start_ui.engine_urls.get(&tab.view).unwrap_or(&tab.url);
        (is_home_url(url) && !wants_html(url)).then_some(tab.view)
    }

    /// Something covers the page area with its own plane (the start page then is not drawn, and
    /// its web view stays hidden).
    fn start_covered(&self) -> bool {
        self.overview.is_open()
            || self.settings.is_open()
            || self.playlist_open()
            || self.passwords.is_open()
            || self.any_list_sheet_open()
    }

    /// The native start page is on screen (or handing over to the site that replaces it).
    pub(super) fn start_shown(&self) -> bool {
        self.start_ui.page.is_open()
    }

    /// Every frame: shows or hides the page, keeps its model and wallpaper current, animates.
    pub(super) fn update_start_page(&mut self, dt: f32) {
        let home_view = self.start_home_view();
        self.sync_start_view(home_view);
        let covered = self.start_covered();
        let visible = home_view.is_some() && !covered && !self.is_locked();
        let open = self.start_ui.page.is_open();
        if visible && !open {
            let rect = self.page_rect();
            self.start_ui.page.open(&mut self.ui, &self.config, rect);
            self.start_ui.stale = true;
            if self.start_ui.shown_view != home_view {
                self.start_ui.shown_view = home_view;
                self.start_ui.page.set_page(&mut self.ui, 0);
            }
        } else if !visible && open {
            self.finish_handoff();
        }
        if !self.start_ui.page.is_open() {
            return;
        }
        let rect = self.page_rect();
        self.start_ui.page.place(&mut self.ui, &self.config, rect);
        if let Some(view) = home_view {
            self.update_start_model(view);
        }
        self.update_start_clock();
        let pointer = self.pointer;
        self.start_ui.page.animate(&mut self.ui, &self.config, dt, pointer);
        self.update_wallpaper(dt);
        // The panel's thumbnails arrive from the thumbnail thread.
        let home = &self.home;
        self.start_ui.page.sync_thumbnails(&mut self.ui, &|name| home.thumbnail(name).flatten());
    }

    /// Hides the web view of the start tab while the native page stands in for it, and hands
    /// it back (see [`Handoff`]) when the tab goes elsewhere.
    fn sync_start_view(&mut self, home_view: Option<ViewId>) {
        let current = self.start_ui.hidden_view;
        if current != home_view {
            if let Some(old) = current
                && Some(old) == self.active_view()
                && !self.playlist_open()
            {
                // The tab left the start page for a site: the view shows again, but the native
                // page stays until the site's first frame is there (no flash of the hidden page).
                self.engine.set_hidden(old, false);
                self.engine.resize(old, self.view_size());
                self.engine.set_focus(old, true);
                let serial = self.engine.frame(old).map_or(0, |f| f.serial);
                self.start_ui.handoff = Some(Handoff { view: old, serial, since: Instant::now() });
            }
            if let Some(new) = home_view {
                self.engine.set_focus(new, false);
                self.engine.set_hidden(new, true);
                self.start_ui.handoff = None;
            }
            self.start_ui.hidden_view = home_view;
        }
        let views: std::collections::HashSet<ViewId> =
            self.tabs.all().iter().chain(self.parked.all()).map(|t| t.view).collect();
        self.start_ui.engine_urls.retain(|v, _| views.contains(v));
    }

    /// Closes the page unless a handoff to a site is still waiting for its frame.
    fn finish_handoff(&mut self) {
        let Some(h) = &self.start_ui.handoff else {
            self.start_ui.page.close(&mut self.ui, &self.config);
            return;
        };
        let arrived = self.engine.frame(h.view).is_some_and(|f| f.serial > h.serial);
        let waited =
            h.since.elapsed() >= Duration::from_millis(u64::from(self.config.home.native_handoff_ms));
        let gone = Some(h.view) != self.active_view();
        if arrived || waited || gone {
            self.start_ui.handoff = None;
            self.start_ui.page.close(&mut self.ui, &self.config);
        }
    }

    fn update_start_model(&mut self, view: ViewId) {
        let private = self.tabs.active().is_some_and(|t| t.private);
        let interval = Duration::from_secs_f32(self.config.home.stats_refresh_seconds.max(0.0));
        let total = self.shields.total();
        let due = self.start_ui.blocked.1.is_none_or(|at| at.elapsed() >= interval);
        if total != self.start_ui.blocked.0 && due {
            self.start_ui.blocked = (total, Some(Instant::now()));
        }
        if !self.start_ui.gpu {
            // The picker's thumbnails are textures from the home page's thumbnail thread.
            self.start_ui.gpu = true;
            self.home.set_gpu(crate::home::Textures::new(self.device.clone(), self.queue.clone()));
        }
        if !std::mem::replace(&mut self.start_ui.engine_icons_asked, true) {
            self.fetch_engine_icons();
        }
        let name = self.account().map(|a| a.name.clone()).unwrap_or_default();
        let inputs = Inputs {
            view,
            home: self.config.home.clone(),
            search_url: self.config.general.search_url.clone(),
            name,
            private,
            accent: self.config.appearance.accent,
            reduce_motion: self.config.glass.reduce_motion,
            flat: self.config.glass.is_flat(),
            language: axion_i18n::generation(),
            icons: self.home.icons_version(),
            backgrounds: self.home.backgrounds_generation(),
            blocked: self.start_ui.blocked.0,
            add_error: self.home.add_error().map(str::to_owned),
        };
        let stale = std::mem::take(&mut self.start_ui.stale);
        if !stale && self.start_ui.inputs.as_ref() == Some(&inputs) {
            return;
        }
        let store = if private { None } else { self.store.as_ref() };
        let model: StartModel =
            self.home.start_model(&self.config, store, inputs.blocked, &inputs.name, private);
        self.start_ui.page.set_private(&mut self.ui, private);
        self.start_ui.page.set_model(&mut self.ui, &self.config, model, &self.favicons, inputs.icons);
        self.start_ui.time = None;
        self.start_ui.inputs = Some(inputs);
    }

    fn update_start_clock(&mut self) {
        let now = jiff::Zoned::now();
        let time = (now.hour() as u32, now.minute() as u32);
        if self.start_ui.time != Some(time) {
            self.start_ui.time = Some(time);
            self.start_ui.page.set_time(&mut self.ui, time.0, time.1);
        }
    }

    /// Asks for the selected wallpaper, takes what was baked, plays an animated one, and lets the
    /// page know.
    fn update_wallpaper(&mut self, dt: f32) {
        let selected = self.start_ui.page.model().and_then(|m| m.background.selected.clone());
        let (blur, live) =
            self.start_ui.page.model().map_or((0.0, false), |m| (m.background.blur, m.background.live));
        let animated_kind = selected.as_ref().is_some_and(|e| e.kind == crate::home::Kind::Animated);
        match selected.and_then(|e| self.home.background_files(&e.name).map(|f| (e.name, e.kind, f))) {
            Some((name, kind, (source, thumb))) => {
                let rect = self.page_rect();
                let edge = start_wallpaper::edge_for(self.logical_size().max_element() * self.scale());
                let key = start_wallpaper::Key {
                    name,
                    blur: (blur * 10.0).round() as u32,
                    edge,
                    animated: kind == crate::home::Kind::Animated,
                };
                let limits = start_wallpaper::AnimationLimits {
                    max_edge: self.config.home.animated_wallpaper_max_edge.max(64),
                    max_bytes: u64::from(self.config.home.animated_wallpaper_max_mb) * 1024 * 1024,
                };
                self.start_ui.wallpaper.request(key, source, thumb, (rect.size.x, rect.size.y), limits);
            }
            None => self.start_ui.wallpaper.clear(),
        }
        self.start_ui.wallpaper.poll();
        // Playback priority 1: a wallpaper that moves pauses while anything plays media.
        let media_playing = self.tabs.all().iter().chain(self.parked.all()).any(|t| t.playing_media)
            || self.playlist.playback.is_some()
            || self.pip.as_ref().is_some_and(|p| p.is_active());
        let play = start_wallpaper::should_play(start_wallpaper::PlayInputs {
            live: live && animated_kind,
            // A minimized window pauses it (capture runs are minimized on purpose: they play).
            shown: self.start_ui.page.is_open()
                && (self.capture.is_some() || !self.window.is_minimized().unwrap_or(false)),
            media_playing,
            frozen: false,
        });
        self.start_ui.wallpaper.set_playing(play);
        self.start_ui.wallpaper.advance(Duration::from_secs_f32(dt.clamp(0.0, 0.25)));
        let spring = crate::drip::spring(&self.config, self.config.motion.smooth);
        self.start_ui.wallpaper.animate(dt, self.config.glass.reduce_motion, spring);
        let shown = self.start_ui.wallpaper.view().is_some();
        self.start_ui.page.set_wallpaper(&mut self.ui, shown);
    }

    /// What the compositor draws under the page's plane: the wallpaper (cover-fitted, with the
    /// page card's corners) and its dim. `None` without a wallpaper (the page's own ground shows).
    pub(super) fn start_wallpaper_layers(&self) -> Option<(wgpu::TextureView, (u32, u32), f32, f32)> {
        if !self.start_ui.page.is_open() {
            return None;
        }
        let (view, size) = self.start_ui.wallpaper.view()?;
        let dim = self.start_ui.page.model().map_or(0.0, |m| m.background.dim);
        Some((view.clone(), size, self.start_ui.wallpaper.opacity(), dim))
    }

    /// The wallpaper as the overview's card for a start tab (the web view behind it is hidden).
    pub(super) fn start_card_picture(&self) -> Option<(wgpu::TextureView, (u32, u32))> {
        let (view, size) = self.start_ui.wallpaper.view()?;
        Some((view.clone(), size))
    }

    // ---- commands ----

    /// Runs a command of the start page on the active tab (what the HTML page's messages do).
    fn start_command(&mut self, command: HomeCommand) {
        let Some(tab) = self.tabs.active() else { return };
        let (view, private) = (tab.view, tab.private);
        let mut scratch = self.config.clone();
        let (config, store) =
            if private { (&mut scratch, None) } else { (&mut self.config, self.store.as_ref()) };
        let action = self.home.apply(command, config, store);
        self.home_action(view, private, action);
    }

    /// Runs one of the page's actions.
    pub(super) fn start_action(&mut self, action: StartAction) {
        let page = &self.start_ui.page;
        let site = |i: usize| page.site(i).cloned();
        match action {
            StartAction::Gear => {
                self.start_ui.page.menu().close(&mut self.ui, &self.config);
                self.start_ui.page.toggle_panel(&mut self.ui, &self.config);
            }
            StartAction::Engine => {
                if self.start_ui.page.menu().is_open() {
                    self.start_ui.page.menu().close(&mut self.ui, &self.config);
                } else {
                    self.start_ui.page.close_panel(&mut self.ui, &self.config);
                    self.start_ui.page.open_engine_menu(&mut self.ui, &self.config);
                }
            }
            StartAction::Format(format) => self.start_command(HomeCommand::ClockFormat(format)),
            StartAction::Restore => self.start_command(HomeCommand::Site(SiteEdit::Restore)),
            StartAction::MoreSettings => {
                self.start_ui.page.close_panel(&mut self.ui, &self.config);
                self.start_command(HomeCommand::OpenSettings);
            }
            StartAction::ChooseBackground => self.start_command(HomeCommand::ChooseBackground),
            StartAction::AddPath => {
                let field = self.start_ui.page.path_field();
                let path = self.ui.field_text(field).unwrap_or_default().trim().to_owned();
                if !path.is_empty() {
                    self.ui.set_field_text(field, "");
                    self.start_command(HomeCommand::BackgroundPath(path));
                }
            }
            StartAction::BackgroundNone => self.start_command(HomeCommand::SelectBackground(String::new())),
            StartAction::Background(i) | StartAction::BackgroundRemove(i) => {
                let name =
                    self.start_ui.page.model().and_then(|m| m.background.list.get(i)).map(|e| e.name.clone());
                if let Some(name) = name {
                    self.start_command(if matches!(action, StartAction::Background(_)) {
                        HomeCommand::SelectBackground(name)
                    } else {
                        HomeCommand::RemoveBackground(name)
                    });
                }
            }
            StartAction::AddSite => {
                self.start_ui.page.close_panel(&mut self.ui, &self.config);
                self.start_ui.page.open_editor(&mut self.ui, None);
            }
            StartAction::Tile(i) => {
                if let Some(site) = site(i) {
                    if self.modifiers.ctrl {
                        self.start_open_background(&site.url);
                    } else {
                        self.navigate_to(site.url);
                    }
                }
            }
            StartAction::TilePin(i) => {
                if let Some(site) = site(i) {
                    let url = site.url;
                    self.start_command(HomeCommand::Site(if site.pinned {
                        SiteEdit::Unpin { url }
                    } else {
                        SiteEdit::Pin { url, title: site.title }
                    }));
                }
            }
            StartAction::TileEdit(i) => {
                if let Some(site) = site(i) {
                    self.start_ui.page.close_panel(&mut self.ui, &self.config);
                    self.start_ui.page.open_editor(&mut self.ui, Some(&site));
                }
            }
            StartAction::TileRemove(i) => {
                if let Some(site) = site(i) {
                    self.start_command(HomeCommand::Site(SiteEdit::Remove { url: site.url }));
                }
            }
            StartAction::EditorSave => {
                let (old, title, url) = self.start_ui.page.editor_values(&self.ui);
                if !url.is_empty() {
                    self.start_command(HomeCommand::Site(SiteEdit::Edit { old, url, title }));
                    self.start_ui.page.close_editor(&mut self.ui);
                }
            }
            StartAction::EditorCancel => self.start_ui.page.close_editor(&mut self.ui),
            StartAction::Dot(i) => self.start_ui.page.set_page(&mut self.ui, i),
            StartAction::Widget(widget, on) => self.start_command(HomeCommand::Widget(widget, on)),
            StartAction::Look { dim, blur } => self.start_command(HomeCommand::BackgroundLook { dim, blur }),
        }
    }

    /// A tile opened in a new background tab (Ctrl+click, middle click).
    fn start_open_background(&mut self, url: &str) {
        if let Err(e) = self.open_tab(url, false) {
            tracing::error!("opening {url} failed: {e:#}");
        }
    }

    /// A UI event that belongs to the page. Returns whether it did.
    pub(super) fn start_event(&mut self, event: &UiEvent) -> bool {
        if !self.start_ui.page.is_open() {
            return false;
        }
        match event {
            UiEvent::Clicked { action, .. } => {
                if let Some(action) = StartAction::from_id(*action) {
                    self.start_action(action);
                    return true;
                }
                if self.start_ui.page.menu().owns(*action) {
                    let click = self.start_ui.page.menu().click(&mut self.ui, &self.config, *action);
                    if let Some(click) = click {
                        self.start_ui.page.menu().close(&mut self.ui, &self.config);
                        if let MenuAction::StartEngine(i) = click.action {
                            self.start_command(HomeCommand::Engine(i));
                        }
                    }
                    return true;
                }
                false
            }
            UiEvent::ValueChanged { node, value } => match self.start_ui.page.value_changed(*node, *value) {
                Some(action) => {
                    self.start_action(action);
                    true
                }
                None => false,
            },
            UiEvent::Submitted { node, text } => {
                if *node == self.start_ui.page.query_field() {
                    self.start_command(HomeCommand::Go(text.clone()));
                    true
                } else if let Some(action) = self.start_ui.page.submitted(*node) {
                    self.start_action(action);
                    true
                } else {
                    false
                }
            }
            UiEvent::Cancelled { node } => {
                let (name, url) = self.start_ui.page.editor_fields();
                if *node == name || *node == url {
                    self.start_ui.page.close_editor(&mut self.ui);
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Esc: closes the dialog, the engine menu or the panel. Returns whether it closed any.
    pub(super) fn start_escape(&mut self) -> bool {
        if !self.start_shown() {
            return false;
        }
        let page = &mut self.start_ui.page;
        if page.editor_open() {
            page.close_editor(&mut self.ui);
        } else if page.menu().is_open() {
            page.menu().close(&mut self.ui, &self.config);
        } else if page.panel_open() {
            page.close_panel(&mut self.ui, &self.config);
        } else if page.focus() != Focus::None {
            page.set_focus(&mut self.ui, Focus::None);
        } else {
            return false;
        }
        true
    }

    /// A press outside the panel or the engine menu closes it (the press goes no further).
    pub(super) fn start_press(&mut self, p: Vec2) -> bool {
        if !self.start_shown() {
            return false;
        }
        let page = &mut self.start_ui.page;
        if page.menu().is_open() && !page.menu().contains(&self.ui, p) {
            page.menu().close(&mut self.ui, &self.config);
            return true;
        }
        if page.panel_open() && !page.panel_contains(&self.ui, p) && !page.gear_frame(&self.ui).contains(p) {
            page.close_panel(&mut self.ui, &self.config);
            return true;
        }
        false
    }

    /// A middle click on a tile opens it in a background tab.
    pub(super) fn start_middle_click(&mut self, p: Vec2) -> bool {
        if !self.start_shown() {
            return false;
        }
        match self.start_ui.page.tile_at(&self.ui, p).and_then(|i| self.start_ui.page.site(i).cloned()) {
            Some(site) => {
                self.start_open_background(&site.url);
                true
            }
            None => false,
        }
    }

    /// A horizontal swipe over the top sites flips a page. Returns whether it was used.
    pub(super) fn start_wheel(&mut self, p: Vec2, delta: Vec2) -> bool {
        if !self.start_shown() || !self.start_ui.page.sites_contain(&self.ui, p) {
            return false;
        }
        if delta.x.abs() <= delta.y.abs() {
            return false;
        }
        self.start_ui.page.swipe(&mut self.ui, &self.config, delta.x)
    }

    /// The registry menu target of the tile under `p`.
    pub(super) fn start_tile_target(&self, p: Vec2) -> Option<crate::context_registry::Target> {
        if !self.start_shown() {
            return None;
        }
        let site = self.start_ui.page.tile_at(&self.ui, p).and_then(|i| self.start_ui.page.site(i))?;
        Some(crate::context_registry::Target::Tile { url: site.url.clone(), pinned: site.pinned })
    }

    /// Keys for the page while nothing in the UI has the keyboard: Tab walks the controls,
    /// arrows the tiles, Enter and Space press, Esc closes, a typed character goes to the search
    /// field. Returns whether the key was used.
    pub(super) fn start_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        if !self.start_shown() || !event.state.is_pressed() || self.start_ui.page.editor_open() && false {
            return false;
        }
        let key = input::key(event);
        let focused = self.ui.focused();
        let in_query = focused == Some(self.start_ui.page.query_field());
        if focused.is_some() && !in_query {
            return false;
        }
        let shift = self.modifiers.shift;
        match key {
            Some(Key::Named(name)) if name == "Tab" => {
                self.start_ui.page.move_focus(&mut self.ui, !shift);
                true
            }
            Some(Key::Named(name)) if in_query => {
                let _ = name;
                false
            }
            Some(Key::Named(name)) if name == "Enter" || name == "Space" => {
                match self.start_ui.page.activate_focus() {
                    Some(action) => {
                        self.start_action(action);
                        true
                    }
                    None => false,
                }
            }
            Some(Key::Named(name)) if matches!(name.as_str(), "Left" | "Right" | "Up" | "Down") => {
                let (dx, dy) = match name.as_str() {
                    "Left" => (-1, 0),
                    "Right" => (1, 0),
                    "Up" => (0, -1),
                    _ => (0, 1),
                };
                if matches!(self.start_ui.page.focus(), Focus::Tile(_)) {
                    self.start_ui.page.move_tile_focus(&mut self.ui, dx, dy);
                    return true;
                }
                false
            }
            Some(Key::Char(_))
                if !in_query && !self.modifiers.ctrl && !self.modifiers.alt && !self.modifiers.logo =>
            {
                let Some(text) = event.text.as_ref().filter(|t| !t.chars().all(char::is_control)) else {
                    return false;
                };
                if !self.start_ui.page.model().is_some_and(|m| m.search.show) {
                    return false;
                }
                // Typing on the page starts the search, with this key as its first character.
                self.start_ui.page.set_focus(&mut self.ui, Focus::Query);
                let (_, events) = self.ui.handle(UiInput::Text(text.to_string()));
                self.handle_ui_events(events);
                true
            }
            _ => false,
        }
    }

    /// Tab out of the search field moves on to the next control.
    pub(super) fn start_field_tab(&mut self, event: &winit::event::KeyEvent) -> bool {
        if !self.start_shown()
            || !event.state.is_pressed()
            || self.ui.focused() != Some(self.start_ui.page.query_field())
        {
            return false;
        }
        match input::key(event) {
            Some(Key::Named(name)) if name == "Tab" => {
                let shift = self.modifiers.shift;
                self.start_ui.page.move_focus(&mut self.ui, !shift);
                true
            }
            _ => false,
        }
    }

    /// Capture aids (`--open=…`): `start-panel`, `start-edit`, `start-engine`, `start-seed:N`
    /// (N visited sites in the throwaway profile) and `start-do:NAME`.
    pub(super) fn start_aid(&mut self, aid: &str) -> bool {
        tracing::info!(aid, shown = self.start_shown(), "start page aid");
        match aid {
            "start-panel" => {
                if self.start_shown() {
                    self.start_ui.page.open_panel(&mut self.ui, &self.config);
                }
            }
            "start-edit" => self.start_action(StartAction::AddSite),
            "start-engine" => self.start_action(StartAction::Engine),
            "start-focus" => {
                self.start_ui.page.move_focus(&mut self.ui, true);
            }
            "start-page-next" => {
                let next = self.start_ui.page.current_page() + 1;
                self.start_ui.page.set_page(&mut self.ui, next);
            }
            seed if seed.starts_with("start-seed:") && std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                let n = seed["start-seed:".len()..].parse::<usize>().unwrap_or(0);
                if let Some(store) = &self.store {
                    let now = unix_now();
                    for i in 0..n {
                        let url = format!("https://site{i}.example/");
                        for _ in 0..(n - i) {
                            let _ = store.record_visit(&url, &format!("Site {i}"), now);
                        }
                    }
                }
                self.start_ui.stale = true;
            }
            "start-pin-first" => self.start_action(StartAction::TilePin(0)),
            "start-clock-off" => self.start_action(StartAction::Widget(HomeWidget::Clock, false)),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_hatch() {
        assert!(wants_html("axion://home/?html=1"));
        assert!(wants_html("axion://home/?a=2&html=1"));
        assert!(!wants_html("axion://home/"));
        assert!(!wants_html("axion://home/?panel=1"));
    }
}
