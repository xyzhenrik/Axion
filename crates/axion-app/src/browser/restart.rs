//! The browser's side of "this change needs a restart": when a setting that is read at start
//! changes (the graphics card, how page frames are shared, hardware video decoding, the
//! interface language), the glass alert (the updater's, above every sheet) says so once per
//! pending change: "Restart Axion to apply this change" with **Restart Axion** and **Later**.
//! "Later" leaves a small Restart pill in the window (`general.restart_hint_pill`) that opens
//! the alert again. Nothing shows over fullscreen video; the alert waits for it to end.

use super::*;
use crate::hardware::Startup;

/// What waits for a restart: the pending interface language and, when the hardware settings
/// differ from what this process started with, those settings (so a second change to them is a
/// new pending change).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pending {
    language: Option<axion_i18n::Language>,
    hardware: Option<Startup>,
}

#[derive(Default)]
pub(super) struct RestartHint {
    /// What waits for a restart now.
    pending: Option<Pending>,
    /// The pending change the alert was raised for.
    prompted: Option<Pending>,
    /// "Later" was chosen for the pending change: the pill shows.
    dismissed: bool,
    /// The alert shows the restart notice (and not another of the alert's uses).
    pub(super) alert_open: bool,
    /// A setting changed: look at what is pending in the next frame.
    check_due: bool,
}

impl Browser {
    /// A setting or the config file changed: whether a restart is pending is looked at in the
    /// next frame.
    pub(super) fn restart_check(&mut self) {
        self.restart.check_due = true;
    }

    fn pending_restart(&self) -> Option<Pending> {
        let language = self.settings.pending_language(&self.config);
        let hardware =
            self.hardware_status.startup.needs_restart(&self.config).then(|| Startup::of(&self.config));
        (language.is_some() || hardware.is_some()).then_some(Pending { language, hardware })
    }

    /// Fullscreen video (Axion's player, the Playlist's, or a page's element): nothing is
    /// drawn over it.
    fn video_fullscreen(&self) -> bool {
        self.player.mode() == PlayerMode::Fullscreen
            || self.playlist.fullscreen
            || self.chrome.page_fullscreen()
    }

    /// The alert can be raised now: nothing else uses it, the tour is over, no video is fullscreen.
    fn restart_alert_free(&self) -> bool {
        !self.update_alert.is_open()
            && !self.onboarding.is_open()
            && !self.is_locked()
            && !self.video_fullscreen()
    }

    /// Something big is on screen (a sheet, the overview): the pill waits.
    fn restart_pill_hidden(&self) -> bool {
        self.settings.is_open()
            || self.passwords.is_open()
            || self.any_list_sheet_open()
            || self.overview.is_open()
            || self.onboarding.is_open()
            || self.update_alert.is_open()
            || self.print_dialog.is_open()
            || self.is_locked()
            || self.video_fullscreen()
    }

    /// Every frame: raises the alert for a new pending change, takes it away when the change is
    /// undone, and shows or hides the pill.
    pub(super) fn animate_restart_hint(&mut self, dt: f32) {
        if self.restart.check_due {
            self.restart.check_due = false;
            let now = self.pending_restart();
            if now != self.restart.pending {
                self.restart.pending = now;
                if self.restart.pending.is_none() {
                    // The change was undone: nothing waits any more.
                    self.restart.prompted = None;
                    self.restart.dismissed = false;
                    if self.restart.alert_open {
                        self.restart.alert_open = false;
                        self.update_alert.close();
                    }
                }
            }
        }
        let new_change = self.restart.pending.is_some() && self.restart.pending != self.restart.prompted;
        if new_change && self.restart_alert_free() {
            self.restart.prompted = self.restart.pending.clone();
            self.restart.dismissed = false;
            self.show_restart_alert();
        }
        let wanted = self.restart.pending.is_some()
            && self.restart.dismissed
            && self.config.general.restart_hint_pill
            && !self.restart.alert_open
            && !self.restart_pill_hidden();
        self.restart_pill.set_shown(&mut self.ui, &self.config, wanted);
        let mut anchor = self.chrome.share_frame(&self.ui);
        if anchor.size.x <= 0.0 {
            anchor = self.chrome.address_frame(&self.ui);
        }
        let window = self.logical_size();
        self.restart_pill.animate(&mut self.ui, anchor, window, dt);
    }

    /// The alert: "Restart Axion to apply this change".
    pub(super) fn show_restart_alert(&mut self) {
        self.restart.alert_open = true;
        self.gpu_confirm = false;
        self.sheet_delete_confirm = None;
        self.update_alert.show(
            &mut self.ui,
            &self.config,
            AlertView {
                icon: Some(Icon::Reload),
                title: tr("Restart Axion to apply this change").to_owned(),
                lines: vec![tr("Your tabs come back after the restart.").to_owned()],
                primary: Some((tr("Restart Axion").to_owned(), Tone::Accent)),
                secondary: Some(tr("Later").to_owned()),
                dismissable: true,
                ..Default::default()
            },
        );
    }

    /// The pill was clicked: the alert again.
    pub(super) fn reopen_restart_alert(&mut self) {
        if self.restart_alert_free() && self.restart.pending.is_some() {
            self.show_restart_alert();
        }
    }

    /// A button of the restart alert (or Escape, or a click outside it).
    pub(super) fn restart_alert_answered(&mut self, action: AlertAction) {
        self.restart.alert_open = false;
        self.update_alert.close();
        if action == AlertAction::Primary {
            self.restart_to_apply_settings();
        } else {
            self.restart.dismissed = true;
            // The inline notice under the language picker follows.
            self.settings.postpone_language(&self.config);
            if self.settings.is_open() {
                self.settings.show(&mut self.ui, &self.config, &self.keymap);
            }
        }
    }

    /// Capture aid `restart-pending`: pretends a restart is pending (the hardware settings differ
    /// from what this run started with) without touching the config.
    pub(super) fn restart_pending_for_test(&mut self) {
        self.hardware_status.startup.video_decoding = !self.config.media.hardware_decoding;
        self.restart_check();
    }
}
