//! The browser's side of the first-run introduction (`crate::onboarding`): deciding whether
//! to show it, applying each choice to the live interface and the account, the keyboard, and
//! what happens when it ends.

use super::*;
use crate::onboarding::{
    AccountChoice, AccountView, Click, Ending, FirstRun, Launch, Pick, Reaction, decide,
};

/// The store's note that an introduction was begun and not ended (so it comes back).
const STARTED_META: &str = "onboarding_started";

impl Browser {
    /// Decides at the start whether this is a first run, and shows the introduction if so.
    pub(super) fn start_onboarding(&mut self, options: &Options, accounts_file: bool, config_file: bool) {
        let browsing_data = self.store.as_ref().is_some_and(|store| {
            store.recent_history(1).is_ok_and(|h| !h.is_empty())
                || store.bookmarks().is_ok_and(|b| !b.is_empty())
                || store.session().is_ok_and(|(tabs, _)| !tabs.is_empty())
        });
        let started = self.store.as_ref().and_then(|store| store.meta(STARTED_META).ok().flatten()).is_some();
        let first_run = FirstRun {
            forced: options.onboarding,
            done: self.config.general.onboarding_done,
            started,
            accounts_file,
            config_file,
            browsing_data,
            capture: options.capture.is_some(),
        };
        match decide(&first_run) {
            Launch::Show => {
                if !first_run.forced
                    && let Some(store) = &self.store
                    && let Err(e) = store.set_meta(STARTED_META, Some("1"))
                {
                    tracing::warn!("noting the introduction failed: {e}");
                }
                tracing::info!("showing the first-run introduction");
                self.open_onboarding(0);
            }
            Launch::MarkDone => {
                // Somebody who used Axion before the introduction existed.
                self.config.general.onboarding_done = true;
                self.config_dirty_since.get_or_insert(Instant::now());
            }
            Launch::Skip => {}
        }
    }

    /// Shows the introduction from `step` (counting from 0).
    pub(super) fn open_onboarding(&mut self, step: usize) {
        if self.settings.is_open() {
            self.close_settings();
        }
        self.menu.close(&mut self.ui, &self.config);
        self.account_menu.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.downloads_popover.close(&mut self.ui, &self.config);
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
        if let Some(icon) = crate::app_icon::picture() {
            self.onboarding.set_icon(Some(self.avatars.upload(&icon)));
        }
        let info = self.account_info();
        self.onboarding.set_account(
            AccountView {
                name: info.name,
                initials: info.initials,
                color: info.color,
                picture: info.picture,
            },
            info.has_password,
        );
        let size = self.logical_size();
        let events = self.onboarding.open(&mut self.ui, &self.config, size, step);
        self.handle_ui_events(events);
    }

    /// The account's name, colour or picture changed: the account step shows it.
    pub(super) fn onboarding_account_changed(&mut self) {
        let info = self.account_info();
        self.onboarding.set_account(
            AccountView {
                name: info.name,
                initials: info.initials,
                color: info.color,
                picture: info.picture,
            },
            info.has_password,
        );
        if self.onboarding.is_open() {
            self.onboarding.rebuild(&mut self.ui, &self.config);
        }
    }

    /// A click, toggle or Enter in the introduction.
    pub(super) fn onboarding_event(&mut self, event: UiEvent) {
        let reaction = match event {
            UiEvent::Clicked { action, .. } => match self.onboarding.click(action) {
                Some(click) => self.onboarding.press(&mut self.ui, &self.config, click),
                None => Reaction::Nothing,
            },
            UiEvent::ValueChanged { node, value } => match self.onboarding.toggled(node, value) {
                Some(pick) => Reaction::Config(pick),
                None => Reaction::Nothing,
            },
            UiEvent::Submitted { node, .. } => self.onboarding.submitted(&mut self.ui, &self.config, node),
            _ => Reaction::Nothing,
        };
        self.onboarding_react(reaction);
    }

    fn onboarding_click(&mut self, click: Click) {
        let reaction = self.onboarding.press(&mut self.ui, &self.config, click);
        self.onboarding_react(reaction);
    }

    fn onboarding_react(&mut self, reaction: Reaction) {
        match reaction {
            Reaction::Nothing | Reaction::Moved => {}
            Reaction::Config(pick) => self.onboarding_pick(pick),
            Reaction::ChoosePhoto => self.photo_chooser.choose(PhotoFor::Current),
            Reaction::RemovePhoto => self.settings_outcome(Outcome::RemoveAccountPhoto),
            Reaction::End(ending, choice) => self.finish_onboarding(ending, choice),
        }
    }

    /// A choice: written into the config and applied to the interface behind the sheet.
    fn onboarding_pick(&mut self, pick: Pick) {
        pick.apply(&mut self.config);
        if let Pick::Language(language) = pick {
            crate::locale::apply(language);
        }
        // The usual path of a changed setting: apply, save a moment later.
        self.settings_outcome(Outcome::Changed);
        self.onboarding.rebuild(&mut self.ui, &self.config);
    }

    /// The introduction ended: keep the account's choices, remember that it is done.
    fn finish_onboarding(&mut self, ending: Ending, choice: Option<AccountChoice>) {
        self.onboarding.close(&mut self.ui);
        if let Some(choice) = choice {
            if self.account().is_none_or(|a| a.name != choice.name) {
                match self.accounts.rename(&self.account_id, &choice.name) {
                    Ok(_) => self.show_account(),
                    Err(e) => tracing::error!("naming the account failed: {e}"),
                }
            }
            if let Some(password) = choice.password
                && !self.passwords.has_password()
            {
                self.settings_outcome(Outcome::SetAccountPassword { current: None, new: password });
            }
        }
        self.config.general.onboarding_done = true;
        self.config_dirty_since.get_or_insert(Instant::now());
        if let Some(store) = &self.store
            && let Err(e) = store.set_meta(STARTED_META, None)
        {
            tracing::warn!("noting the end of the introduction failed: {e}");
        }
        tracing::info!(?ending, "first-run introduction over");
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, true);
        }
        // The rest of the interface was built in the language it started in.
        if axion_i18n::language() != self.start_language && self.capture.is_none() {
            let id = self.account_id.clone();
            self.relaunch_into(&id);
        }
    }

    /// The keyboard while the introduction is up: Tab and the arrows move over its controls,
    /// Enter continues (or presses the focused one), Space presses the focused control, Esc
    /// goes back. Nothing reaches the page or the shortcuts behind it. Returns whether to quit.
    pub(super) fn onboarding_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        let key = input::key(event);
        if let Some(chord) = input::chord(event, self.modifiers)
            && self.keymap.action_for(&chord) == Some(Action::Quit)
        {
            return true;
        }
        let named = |wanted: &str| matches!(&key, Some(Key::Named(name)) if name == wanted);
        if named("Tab") {
            self.onboarding.move_focus(&mut self.ui, !self.modifiers.shift);
            return false;
        }
        if named("Escape") {
            let _ = self.ui.blur();
            self.onboarding_click(Click::Back);
            return false;
        }
        if self.onboarding.editing(&self.ui) {
            // Typing in a field of the account step.
            if let Some(key) = key {
                let is_text = matches!(key, Key::Char(_)) && !self.modifiers.ctrl && !self.modifiers.logo;
                if !is_text {
                    let (_, events) = self.ui.handle(UiInput::Key { key, modifiers: self.modifiers });
                    self.handle_ui_events(events);
                }
            }
            if !self.modifiers.ctrl
                && !self.modifiers.logo
                && let Some(text) = event.text.as_ref().filter(|t| !t.chars().all(char::is_control))
            {
                let (_, events) = self.ui.handle(UiInput::Text(text.to_string()));
                self.handle_ui_events(events);
            }
            return false;
        }
        if named("Enter") || named("Space") {
            match self.onboarding.focused_action() {
                Some(action) => {
                    self.onboarding_event(UiEvent::Clicked { action, node: self.onboarding.root() })
                }
                None if named("Enter") => self.onboarding_click(Click::Continue),
                None => {}
            }
        } else if named("Left") || named("Right") {
            let forward = named("Right");
            if self.onboarding.has_focus() {
                self.onboarding.move_focus(&mut self.ui, forward);
            } else {
                self.onboarding_click(if forward { Click::Continue } else { Click::Back });
            }
        }
        false
    }

    /// Capture aids of the introduction (`--open=onb-…`); returns whether `what` was one.
    pub(super) fn onboarding_capture_step(&mut self, what: &str) -> bool {
        let Some(rest) = what.strip_prefix("onb-").or_else(|| what.strip_prefix("onboarding")) else {
            return false;
        };
        let rest = rest.trim_start_matches('-');
        let (name, value) = rest.split_once(':').unwrap_or((rest, ""));
        let step = |this: &mut Self, index: usize| {
            this.onboarding.goto(&mut this.ui, &this.config, index);
        };
        match name {
            "" => self.open_onboarding(0),
            "step" => {
                let index = value.parse::<usize>().unwrap_or(1).saturating_sub(1);
                if !self.onboarding.is_open() {
                    self.open_onboarding(index);
                }
                step(self, index);
            }
            "next" => self.onboarding_click(Click::Continue),
            "back" => self.onboarding_click(Click::Back),
            "skip" => self.onboarding_click(Click::Skip),
            "tab" => {
                for _ in 0..value.parse::<usize>().unwrap_or(1) {
                    self.onboarding.move_focus(&mut self.ui, true);
                }
            }
            "lang" => {
                let language = match value {
                    "de" => UiLanguage::German,
                    "en" => UiLanguage::English,
                    _ => UiLanguage::System,
                };
                self.onboarding_pick(Pick::Language(language));
            }
            "theme" => {
                let mode = match value {
                    "dark" => ThemeMode::Dark,
                    "light" => ThemeMode::Light,
                    _ => ThemeMode::Auto,
                };
                self.onboarding_pick(Pick::Theme(mode));
            }
            "accent" => self.onboarding_pick(Pick::Accent(value.parse().unwrap_or(0))),
            "tabs" => self.onboarding_pick(Pick::TabBar(if value == "separate" {
                TabBarStyle::Separate
            } else {
                TabBarStyle::Compact
            })),
            "sidebar" => self.onboarding_pick(Pick::FloatSidebar(value == "float")),
            "glass" => self.onboarding_pick(Pick::Glass(if value == "flat" {
                GlassStyle::Flat
            } else {
                GlassStyle::Liquid
            })),
            "motion" => self.onboarding_pick(Pick::ReduceMotion(value != "off")),
            "transparency" => self.onboarding_pick(Pick::ReduceTransparency(value != "off")),
            "name" => self.onboarding.set_name(&mut self.ui, value),
            "password" => self.onboarding.set_password(&mut self.ui, value),
            other => tracing::warn!("--open=onb-{other} is not a known step"),
        }
        true
    }
}
