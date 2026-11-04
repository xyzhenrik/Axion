//! The browser's side of "Axion is the default browser": asking the desktop (off the main
//! thread), the Settings row, the first-run offer and the startup hint (a toast with
//! "Make Default" and "Don't ask again").

use std::sync::Arc;

use super::*;
use crate::default_browser::{Done, Service, State, SystemRunner};
use crate::desktop_integration::Target;
use crate::settings::DefaultBrowserView;

/// What the toast's buttons mean right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ToastKind {
    /// "Update": opens About.
    #[default]
    Update,
    /// "Make Default" / "Don't ask again".
    DefaultBrowser,
    /// "Restart" / "Later": a language change waits for a restart.
    LanguageRestart,
}

pub(super) struct DefaultBrowser {
    service: Service,
    state: Option<State>,
    error: Option<String>,
    /// The startup hint may still be shown this session (once the state is known).
    hint_due: bool,
}

impl DefaultBrowser {
    pub(super) fn new(hint_wanted: bool) -> Self {
        let target = Target::current();
        Self {
            service: Service::new(Arc::new(SystemRunner), target),
            state: None,
            error: None,
            hint_due: hint_wanted,
        }
    }
}

impl Browser {
    /// Asks the desktop who the default browser is (the answer arrives through `pump`).
    pub(super) fn start_default_browser_check(&mut self) {
        if cfg!(target_os = "linux") {
            self.default_browser.service.refresh();
        }
    }

    /// What the General page and the first-run step show.
    pub(super) fn default_browser_view(&self) -> DefaultBrowserView {
        DefaultBrowserView {
            state: self.default_browser.state.clone(),
            busy: self.default_browser.service.is_busy(),
            error: self.default_browser.error.clone(),
        }
    }

    /// Makes Axion the default browser (Settings, the first-run step, the hint's button).
    pub(super) fn make_default_browser(&mut self) {
        self.default_browser.error = None;
        self.default_browser.service.make_default();
        self.sync_default_browser_views();
    }

    /// Takes the answers of finished jobs, then shows the hint when it is time.
    pub(super) fn poll_default_browser(&mut self) {
        while let Some(done) = self.default_browser.service.poll() {
            match done {
                Done::State(state) => self.default_browser.state = Some(state),
                Done::Made(Ok(state)) => {
                    self.default_browser.state = Some(state);
                    self.default_browser.error = None;
                    self.show_toast(Icon::Check, tr("Axion is now your default browser"));
                    self.toast_kind = ToastKind::Update;
                }
                Done::Made(Err(reason)) => {
                    tracing::warn!(%reason, "could not make Axion the default browser");
                    self.default_browser.error = Some(reason);
                    // Ask again: the desktop may have changed it after all.
                    self.default_browser.service.refresh();
                }
            }
            self.sync_default_browser_views();
        }
        self.maybe_show_default_hint();
    }

    /// Passes the state on to the open Settings page and the introduction.
    fn sync_default_browser_views(&mut self) {
        let view = self.default_browser_view();
        self.settings.set_default_browser(view);
        if self.settings.is_open() && self.settings.category() == Category::General {
            self.settings.show(&mut self.ui, &self.config, &self.keymap);
        }
        if self.onboarding.is_open() {
            self.onboarding.set_default_state(self.default_browser.state.clone());
            self.onboarding.rebuild(&mut self.ui, &self.config);
        }
    }

    /// At start, once: a toast offering to make Axion the default browser when it is not.
    fn maybe_show_default_hint(&mut self) {
        if !self.default_browser.hint_due {
            return;
        }
        if !self.config.general.default_browser_hint || self.capture.is_some() {
            self.default_browser.hint_due = false;
            return;
        }
        // Wait for the answer, and for the introduction (it has its own offer) to be over.
        let Some(state) = &self.default_browser.state else { return };
        if self.onboarding.is_open() || self.update_alert.is_open() {
            return;
        }
        self.default_browser.hint_due = false;
        if !state.can_change() {
            return;
        }
        self.show_default_browser_hint_now();
    }

    /// Raises the hint toast (capture aid `--open=default-hint` shows it regardless of state).
    pub(super) fn show_default_browser_hint_now(&mut self) {
        let mut anchor = self.chrome.share_frame(&self.ui);
        if anchor.size.x <= 0.0 {
            anchor = self.chrome.address_frame(&self.ui);
        }
        let window = self.logical_size();
        self.toast_kind = ToastKind::DefaultBrowser;
        self.toast.show_with_actions(
            &mut self.ui,
            &self.config,
            anchor,
            window,
            Icon::Globe,
            tr("Axion is not your default browser"),
            Some(tr("Make Default")),
            Some(tr("Don’t ask again")),
        );
    }

    /// The toast's first button.
    pub(super) fn toast_primary(&mut self) {
        self.toast.dismiss(&mut self.ui, &self.config);
        match self.toast_kind {
            ToastKind::Update => self.open_about(false),
            ToastKind::DefaultBrowser => self.make_default_browser(),
            ToastKind::LanguageRestart => self.restart_to_apply_settings(),
        }
    }

    /// The toast's second button ("Don't ask again"): the hint is switched off for good.
    pub(super) fn toast_secondary(&mut self) {
        self.toast.dismiss(&mut self.ui, &self.config);
        if self.toast_kind == ToastKind::LanguageRestart {
            self.settings.postpone_language(&self.config);
        }
        if self.toast_kind == ToastKind::DefaultBrowser {
            self.config.general.default_browser_hint = false;
            self.config_dirty_since.get_or_insert_with(Instant::now);
            if self.settings.is_open() {
                self.settings.show(&mut self.ui, &self.config, &self.keymap);
            }
        }
    }
}
