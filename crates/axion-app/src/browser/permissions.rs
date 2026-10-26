//! The browser's side of site permissions: pages' requests (answered from the defaults and
//! remembered decisions, or put to the user in the prompt under the address field), the
//! recording indicator and its Stop buttons, and Settings › Privacy › Site Permissions.
//! The rules are in `crate::permissions`, the prompt in `crate::permission_prompt`.

use axion_engine::{PermissionAnswer, PermissionId, PermissionKind};

use super::*;
use crate::permission_prompt::CaptureAction;
use crate::permissions::{Choice, Decision, Group, Request, SitePermissions};

/// The browser's permission state (a field of [`Browser`]).
pub(super) struct PermissionsUi {
    pub model: SitePermissions,
    pub prompt: crate::permission_prompt::PermissionPrompt,
    pub capture: crate::permission_prompt::CapturePopover,
    /// Stops asked for: if the tab still records these kinds by then, it is reloaded.
    stops: Vec<(ViewId, Vec<PermissionKind>, Instant)>,
    /// The `[permissions]` defaults last handed to the engine.
    applied: Option<axion_config::Permissions>,
}

impl PermissionsUi {
    pub fn build(ui: &mut Ui, bar: axion_ui::taffy::NodeId, store: Option<&axion_store::Store>) -> Self {
        let remembered = store
            .map(|s| {
                s.site_permissions().inspect_err(|e| tracing::warn!("reading site permissions failed: {e}"))
            })
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.origin, p.kind, p.allowed));
        Self {
            model: SitePermissions::new(remembered),
            prompt: crate::permission_prompt::PermissionPrompt::build(ui, bar),
            capture: crate::permission_prompt::CapturePopover::build(ui, bar),
            stops: Vec::new(),
            applied: None,
        }
    }
}

impl Browser {
    /// At start: the engine's own permission store gets the remembered decisions and defaults
    /// (it answers prompts it knows the answer to without asking).
    pub(super) fn start_permissions(&mut self) {
        for (origin, kind, allow) in self.permissions.model.remembered() {
            self.engine.set_site_permission(&origin, kind, Some(allow), false);
        }
        self.apply_permission_defaults();
    }

    /// Hands changed `[permissions]` defaults to the engine.
    pub(super) fn apply_permission_defaults(&mut self) {
        if self.permissions.applied.as_ref() == Some(&self.config.permissions) {
            return;
        }
        for kind in PermissionKind::ALL {
            let blocked = Group::of(kind).default_of(&self.config).blocks();
            self.engine.set_permission_default(kind, blocked);
        }
        self.permissions.applied = Some(self.config.permissions.clone());
    }

    pub(super) fn permission_requested(
        &mut self,
        view: ViewId,
        id: PermissionId,
        origin: String,
        kinds: Vec<PermissionKind>,
    ) {
        let Some(private) = find_tab(&self.tabs, &self.parked, view).map(|t| t.private) else {
            // Not a tab (the Playlist's views): they never get to record anything.
            self.engine.answer_permission(view, id, PermissionAnswer::Deny);
            return;
        };
        let decision = self.permissions.model.decide(&self.config, view, &origin, &kinds, private);
        tracing::info!(?view, %origin, ?kinds, ?decision, "permission request");
        match decision {
            Decision::Allow => {
                self.permissions.model.grant(view, &kinds);
                self.engine.answer_permission(view, id, PermissionAnswer::Allow);
            }
            Decision::Block => self.engine.answer_permission(view, id, PermissionAnswer::Deny),
            Decision::Ask => {
                let request = Request { view, id, origin, kinds, private };
                if let Some(old) = self.permissions.model.enqueue(request) {
                    self.engine.answer_permission(old.view, old.id, PermissionAnswer::Dismiss);
                }
                self.sync_permission_prompt();
            }
        }
    }

    /// Chromium withdrew a request (the page navigated or closed).
    pub(super) fn permission_cancelled(&mut self, view: ViewId, id: PermissionId) {
        self.permissions.model.take(view, id);
        self.sync_permission_prompt();
    }

    /// The tab is on a new address: leaving a site dismisses its waiting requests.
    pub(super) fn permissions_navigated(&mut self, view: ViewId, url: &str) {
        for request in self.permissions.model.navigated(view, url) {
            self.engine.answer_permission(request.view, request.id, PermissionAnswer::Dismiss);
        }
        self.sync_permission_prompt();
    }

    pub(super) fn permissions_closed(&mut self, view: ViewId) {
        self.permissions.model.closed(view);
        self.permissions.stops.retain(|(v, ..)| *v != view);
        self.sync_permission_prompt();
    }

    pub(super) fn capture_changed(&mut self, view: ViewId, audio: bool, video: bool) {
        let capture = self.permissions.model.capture_changed(view, audio, video);
        tracing::info!(?view, ?capture, "capture changed");
        if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
            tab.capture = capture;
            self.tabs_changed = true;
        }
        if self.permissions.capture.view() == Some(view) {
            self.open_capture_popover();
        }
    }

    /// Shows the shown tab's oldest waiting request in the prompt, or closes it.
    pub(super) fn sync_permission_prompt(&mut self) {
        let view = self.active_view().filter(|_| !self.is_locked() && !self.settings.is_open());
        let request = self.permissions.model.next_for(view).cloned();
        match request {
            Some(request) => {
                // One prompt under the address field at a time.
                self.passwords.hide_prompt(Some(request.view), true);
                let picker = self.engine.screen_share_has_picker();
                self.permissions.prompt.show(&mut self.ui, &self.config, &request, picker);
            }
            None => self.permissions.prompt.hide(),
        }
    }

    /// A click in the prompt.
    pub(super) fn answer_permission_prompt(&mut self, choice: Choice) {
        let Some((view, id)) = self.permissions.prompt.shown() else { return };
        let Some(request) = self.permissions.model.take(view, id) else {
            self.permissions.prompt.hide();
            return;
        };
        let answer = match choice {
            Choice::Allow | Choice::AllowWhileVisiting => PermissionAnswer::Allow,
            Choice::Block => PermissionAnswer::Deny,
            Choice::Dismiss => PermissionAnswer::Dismiss,
        };
        tracing::info!(?view, origin = %request.origin, kinds = ?request.kinds, ?choice, "permission answered");
        let now = unix_now();
        for remember in self.permissions.model.record(&request, choice) {
            if !remember.private
                && let Some(store) = &self.store
                && let Err(e) = store.set_site_permission(
                    &remember.origin,
                    remember.kind.key(),
                    Some(remember.allow),
                    now,
                )
            {
                tracing::warn!("saving a site permission failed: {e}");
            }
            self.engine.set_site_permission(
                &remember.origin,
                remember.kind,
                Some(remember.allow),
                remember.private,
            );
        }
        self.engine.answer_permission(view, id, answer);
        self.permissions.prompt.hide();
        self.sync_permission_prompt();
        self.refresh_site_permissions();
    }

    /// Esc closes the capture popover, else dismisses the prompt. Returns whether it did.
    pub(super) fn permissions_escape(&mut self) -> bool {
        if self.permissions.capture.is_open() {
            self.permissions.capture.close(&mut self.ui, &self.config);
            return true;
        }
        if self.permissions.prompt.is_open() {
            self.answer_permission_prompt(Choice::Dismiss);
            return true;
        }
        false
    }

    /// A press outside the capture popover closes it (the prompt stays until answered).
    pub(super) fn permissions_press(&mut self, p: Vec2) -> bool {
        if self.permissions.capture.is_open() && !self.permissions.capture.contains(&self.ui, p) {
            self.permissions.capture.close(&mut self.ui, &self.config);
            return true;
        }
        false
    }

    pub(super) fn toggle_capture_popover(&mut self) {
        if self.permissions.capture.is_open() {
            self.permissions.capture.close(&mut self.ui, &self.config);
        } else {
            self.open_capture_popover();
        }
    }

    fn open_capture_popover(&mut self) {
        let Some(tab) = self.tabs.active() else { return };
        let (view, capture, host) = (tab.view, tab.capture, display_host(&tab.url));
        let anchor = self.chrome.capture_frame(&self.ui);
        let width = self.logical_size().x;
        self.permissions.capture.open(&mut self.ui, &self.config, anchor, width, (view, &host, capture));
    }

    pub(super) fn capture_action(&mut self, action: CaptureAction) {
        let Some(view) = self.permissions.capture.view().or_else(|| self.active_view()) else { return };
        let kinds = match action {
            CaptureAction::Stop(kind) => vec![kind],
            CaptureAction::StopAll => {
                find_tab(&self.tabs, &self.parked, view).map(|t| t.capture.kinds()).unwrap_or_default()
            }
        };
        self.stop_capture(view, kinds);
        self.permissions.capture.close(&mut self.ui, &self.config);
    }

    /// Ends the tab's capture of `kinds` (and a visit's grant for them); a page that keeps
    /// recording is reloaded a moment later.
    fn stop_capture(&mut self, view: ViewId, mut kinds: Vec<PermissionKind>) {
        if kinds.contains(&PermissionKind::ScreenShare) {
            kinds.push(PermissionKind::ScreenAudio);
        }
        self.permissions.model.stopped(view, &kinds);
        self.engine.stop_capture(view, &kinds);
        let wait = Duration::from_millis(u64::from(self.config.permissions.stop_reload_after_ms));
        self.permissions.stops.push((view, kinds, Instant::now() + wait));
    }

    /// Reloads tabs that kept recording after Stop (see `stop_capture`).
    pub(super) fn check_capture_stops(&mut self) {
        let now = Instant::now();
        let due: Vec<(ViewId, Vec<PermissionKind>)> = {
            let stops = &mut self.permissions.stops;
            let (due, waiting) = std::mem::take(stops).into_iter().partition(|(_, _, at)| *at <= now);
            *stops = waiting;
            due.into_iter().map(|(v, k, _)| (v, k)).collect()
        };
        for (view, kinds) in due {
            let capture = self.permissions.model.capture(view);
            let still = kinds.iter().any(|k| match k {
                PermissionKind::Microphone => capture.microphone,
                PermissionKind::Camera => capture.camera,
                PermissionKind::ScreenShare | PermissionKind::ScreenAudio => capture.screen,
                _ => false,
            });
            if still {
                tracing::info!(?view, ?kinds, "the page kept recording after Stop; reloading it");
                self.engine.reload(view, false);
            }
        }
    }

    pub(super) fn animate_permissions(&mut self, dt: f32, address: Rect) {
        // The shown tab, the lock or Settings may have changed since.
        self.sync_permission_prompt();
        self.permissions.prompt.animate(&mut self.ui, &self.config, dt, address);
        self.permissions.capture.animate(&mut self.ui, dt);
    }

    /// Settings › Privacy lists the remembered decisions.
    pub(super) fn refresh_site_permissions(&mut self) {
        self.settings.set_site_permissions(self.permissions.model.remembered());
        self.refresh_settings();
    }

    /// Settings forgot one site's decision (`Some`), or all of them.
    pub(super) fn forget_site_permissions(&mut self, which: Option<(String, PermissionKind)>) {
        let forgotten = match which {
            Some((origin, kind)) => {
                self.permissions.model.forget(&origin, kind);
                vec![(origin, kind)]
            }
            None => self.permissions.model.forget_all().into_iter().map(|(o, k, _)| (o, k)).collect(),
        };
        for (origin, kind) in &forgotten {
            if let Some(store) = &self.store
                && let Err(e) = store.set_site_permission(origin, kind.key(), None, unix_now())
            {
                tracing::warn!("forgetting a site permission failed: {e}");
            }
            self.engine.set_site_permission(origin, *kind, None, false);
        }
        tracing::info!(count = forgotten.len(), "forgot site permissions");
        self.refresh_site_permissions();
    }

    /// Test aid (`--open=permission-allow|permission-visit|permission-block|permission-dismiss`).
    pub(super) fn answer_permission_for_test(&mut self, what: &str) {
        let choice = match what {
            "permission-allow" => Choice::Allow,
            "permission-visit" => Choice::AllowWhileVisiting,
            "permission-block" => Choice::Block,
            _ => Choice::Dismiss,
        };
        self.answer_permission_prompt(choice);
    }
}
