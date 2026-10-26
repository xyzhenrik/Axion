//! Pages' permission requests: CEF's two kinds of callbacks (media access for `getUserMedia` /
//! `getDisplayMedia`, permission prompts for everything else) kept until the browser answers,
//! and Chromium's per-site content settings that mirror the browser's remembered decisions.
//!
//! Off-screen (Alloy style) browsers have no permission UI of their own: without a handler,
//! Chromium denies media access and ignores every other prompt. CEF calls all of this on the
//! browser process UI thread (our main thread), so plain `RefCell` state is enough.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use cef::sys::{
    cef_content_setting_types_t as setting, cef_content_setting_values_t as value,
    cef_media_access_permission_types_t as media, cef_permission_request_result_t as result,
    cef_permission_request_types_t as prompt,
};
use cef::*;

use crate::{PermissionAnswer, PermissionId, PermissionKind, ViewId};

/// Media access bits (`cef_media_access_permission_types_t`) and what they ask for.
const MEDIA_KINDS: [(u32, PermissionKind); 4] = [
    (media::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32, PermissionKind::Microphone),
    (media::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32, PermissionKind::Camera),
    (media::CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32, PermissionKind::ScreenShare),
    (media::CEF_MEDIA_PERMISSION_DESKTOP_AUDIO_CAPTURE as u32, PermissionKind::ScreenAudio),
];

/// Permission prompt bits (`cef_permission_request_types_t`) Axion asks the user about. Any
/// other bit is left to Chromium's default for off-screen browsers (ignored, as before).
const PROMPT_KINDS: [(u32, PermissionKind); 16] = [
    (prompt::CEF_PERMISSION_TYPE_MIC_STREAM as u32, PermissionKind::Microphone),
    (prompt::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32, PermissionKind::Camera),
    (prompt::CEF_PERMISSION_TYPE_CAMERA_PAN_TILT_ZOOM as u32, PermissionKind::Camera),
    (prompt::CEF_PERMISSION_TYPE_NOTIFICATIONS as u32, PermissionKind::Notifications),
    (prompt::CEF_PERMISSION_TYPE_GEOLOCATION as u32, PermissionKind::Geolocation),
    (prompt::CEF_PERMISSION_TYPE_CLIPBOARD as u32, PermissionKind::Clipboard),
    (prompt::CEF_PERMISSION_TYPE_MIDI_SYSEX as u32, PermissionKind::Midi),
    (prompt::CEF_PERMISSION_TYPE_LOCAL_NETWORK as u32, PermissionKind::LocalNetwork),
    (prompt::CEF_PERMISSION_TYPE_LOOPBACK_NETWORK as u32, PermissionKind::LocalNetwork),
    (prompt::CEF_PERMISSION_TYPE_WINDOW_MANAGEMENT as u32, PermissionKind::WindowManagement),
    (prompt::CEF_PERMISSION_TYPE_LOCAL_FONTS as u32, PermissionKind::LocalFonts),
    (prompt::CEF_PERMISSION_TYPE_IDLE_DETECTION as u32, PermissionKind::IdleDetection),
    (prompt::CEF_PERMISSION_TYPE_POINTER_LOCK as u32, PermissionKind::PointerLock),
    (prompt::CEF_PERMISSION_TYPE_KEYBOARD_LOCK as u32, PermissionKind::KeyboardLock),
    (prompt::CEF_PERMISSION_TYPE_SENSORS as u32, PermissionKind::Sensors),
    (prompt::CEF_PERMISSION_TYPE_MULTIPLE_DOWNLOADS as u32, PermissionKind::AutomaticDownloads),
];

/// Local network access, which media views get without asking (see `client.rs`).
pub(super) const LOCAL_NETWORK_BITS: u32 =
    prompt::CEF_PERMISSION_TYPE_LOCAL_NETWORK as u32 | prompt::CEF_PERMISSION_TYPE_LOOPBACK_NETWORK as u32;

fn kinds_of(bits: u32, table: &[(u32, PermissionKind)]) -> (Vec<PermissionKind>, u32) {
    let mut kinds = Vec::new();
    let mut known = 0;
    for &(bit, kind) in table {
        if bits & bit != 0 {
            known |= bit;
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    (kinds, bits & !known)
}

/// What a media access request asks for (and bits Axion doesn't know).
pub(super) fn media_kinds(bits: u32) -> (Vec<PermissionKind>, u32) {
    kinds_of(bits, &MEDIA_KINDS)
}

/// What a permission prompt asks for (and bits Axion doesn't know).
pub(super) fn prompt_kinds(bits: u32) -> (Vec<PermissionKind>, u32) {
    kinds_of(bits, &PROMPT_KINDS)
}

/// Chromium's content settings that hold a kind's per-site decision (none for screen
/// sharing: it is never remembered).
fn content_settings(kind: PermissionKind) -> &'static [setting] {
    use setting::*;
    match kind {
        PermissionKind::Microphone => &[CEF_CONTENT_SETTING_TYPE_MEDIASTREAM_MIC],
        PermissionKind::Camera => &[CEF_CONTENT_SETTING_TYPE_MEDIASTREAM_CAMERA],
        PermissionKind::ScreenShare | PermissionKind::ScreenAudio => &[],
        PermissionKind::Notifications => &[CEF_CONTENT_SETTING_TYPE_NOTIFICATIONS],
        PermissionKind::Geolocation => &[CEF_CONTENT_SETTING_TYPE_GEOLOCATION],
        PermissionKind::Clipboard => &[CEF_CONTENT_SETTING_TYPE_CLIPBOARD_READ_WRITE],
        PermissionKind::Midi => &[CEF_CONTENT_SETTING_TYPE_MIDI_SYSEX],
        PermissionKind::LocalNetwork => {
            &[CEF_CONTENT_SETTING_TYPE_LOCAL_NETWORK, CEF_CONTENT_SETTING_TYPE_LOOPBACK_NETWORK]
        }
        PermissionKind::WindowManagement => &[CEF_CONTENT_SETTING_TYPE_WINDOW_MANAGEMENT],
        PermissionKind::LocalFonts => &[CEF_CONTENT_SETTING_TYPE_LOCAL_FONTS],
        PermissionKind::IdleDetection => &[CEF_CONTENT_SETTING_TYPE_IDLE_DETECTION],
        PermissionKind::PointerLock => &[CEF_CONTENT_SETTING_TYPE_POINTER_LOCK],
        PermissionKind::KeyboardLock => &[CEF_CONTENT_SETTING_TYPE_KEYBOARD_LOCK],
        PermissionKind::Sensors => &[CEF_CONTENT_SETTING_TYPE_SENSORS],
        PermissionKind::AutomaticDownloads => &[CEF_CONTENT_SETTING_TYPE_AUTOMATIC_DOWNLOADS],
    }
}

/// Whether Chromium decides the kind through a permission prompt (and so keeps its own copy
/// of the decision, and honours a default): everything but media capture, which CEF asks
/// about every time.
fn prompted_by_chromium(kind: PermissionKind) -> bool {
    !kind.is_capture()
}

/// Sets the site's decision for `kind` in Chromium's content settings of `context` (`None`:
/// back to the default, so Chromium asks again).
pub(super) fn set_site_setting(
    context: &RequestContext,
    origin: &str,
    kind: PermissionKind,
    allow: Option<bool>,
) {
    let value = match allow {
        Some(true) => value::CEF_CONTENT_SETTING_VALUE_ALLOW,
        Some(false) => value::CEF_CONTENT_SETTING_VALUE_BLOCK,
        None => value::CEF_CONTENT_SETTING_VALUE_DEFAULT,
    };
    let url = CefString::from(origin);
    for &content in content_settings(kind) {
        context.set_content_setting(Some(&url), Some(&url), content.into(), value.into());
    }
}

/// Sets Chromium's default for a prompted kind: blocked, or asking (Chromium's own default).
/// Media capture is asked about by CEF whatever the default says, so it is left alone.
pub(super) fn set_default_setting(context: &RequestContext, kind: PermissionKind, blocked: bool) {
    if !prompted_by_chromium(kind) {
        return;
    }
    let value = if blocked {
        value::CEF_CONTENT_SETTING_VALUE_BLOCK
    } else {
        value::CEF_CONTENT_SETTING_VALUE_DEFAULT
    };
    for &content in content_settings(kind) {
        context.set_content_setting(None, None, content.into(), value.into());
    }
}

/// A request waiting for the browser's answer.
enum Callback {
    Media {
        callback: MediaAccessCallback,
        /// The bits asked for: `getUserMedia` must be granted exactly these.
        requested: u32,
    },
    Prompt {
        callback: PermissionPromptCallback,
        /// Chromium's id of the prompt (see `on_dismiss_permission_prompt`).
        prompt_id: u64,
    },
}

struct Pending {
    view: ViewId,
    callback: Callback,
}

/// The requests waiting for an answer, by id.
#[derive(Default)]
pub(super) struct Permissions {
    next: Cell<u64>,
    pending: RefCell<HashMap<u64, Pending>>,
}

impl Permissions {
    fn insert(&self, view: ViewId, callback: Callback) -> PermissionId {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.pending.borrow_mut().insert(id, Pending { view, callback });
        PermissionId(id)
    }

    pub fn add_media(&self, view: ViewId, callback: MediaAccessCallback, requested: u32) -> PermissionId {
        self.insert(view, Callback::Media { callback, requested })
    }

    pub fn add_prompt(
        &self,
        view: ViewId,
        callback: PermissionPromptCallback,
        prompt_id: u64,
    ) -> PermissionId {
        self.insert(view, Callback::Prompt { callback, prompt_id })
    }

    /// Chromium closed a prompt itself (navigation, the page closed): forgets it and returns
    /// its id if it was still waiting.
    pub fn prompt_closed(&self, prompt_id: u64) -> Option<(ViewId, PermissionId)> {
        let mut pending = self.pending.borrow_mut();
        let id = pending
            .iter()
            .find(|(_, p)| matches!(p.callback, Callback::Prompt { prompt_id: p, .. } if p == prompt_id))
            .map(|(id, _)| *id)?;
        let view = pending.remove(&id)?.view;
        Some((view, PermissionId(id)))
    }

    /// Answers a request (nothing happens for one that is gone, or of another view).
    pub fn answer(&self, view: ViewId, id: PermissionId, answer: PermissionAnswer) {
        let pending = {
            let mut pending = self.pending.borrow_mut();
            match pending.get(&id.0) {
                Some(p) if p.view == view => pending.remove(&id.0),
                _ => None,
            }
        };
        // Called with the map released: continuing can run Chromium code that asks again.
        let Some(pending) = pending else { return };
        match pending.callback {
            Callback::Media { callback, requested } => match answer {
                PermissionAnswer::Allow => callback.cont(requested),
                PermissionAnswer::Deny => callback.cont(media::CEF_MEDIA_PERMISSION_NONE as u32),
                PermissionAnswer::Dismiss => callback.cancel(),
            },
            Callback::Prompt { callback, .. } => {
                let result = match answer {
                    PermissionAnswer::Allow => result::CEF_PERMISSION_RESULT_ACCEPT,
                    PermissionAnswer::Deny => result::CEF_PERMISSION_RESULT_DENY,
                    PermissionAnswer::Dismiss => result::CEF_PERMISSION_RESULT_DISMISS,
                };
                callback.cont(result.into());
            }
        }
    }

    /// The view's page closed or was discarded: its waiting media requests are cancelled
    /// (Chromium closes prompts itself) and all of them forgotten.
    pub fn drop_view(&self, view: ViewId) {
        let gone: Vec<Pending> = {
            let mut pending = self.pending.borrow_mut();
            let ids: Vec<u64> = pending.iter().filter(|(_, p)| p.view == view).map(|(id, _)| *id).collect();
            ids.into_iter().filter_map(|id| pending.remove(&id)).collect()
        };
        for pending in gone {
            if let Callback::Media { callback, .. } = pending.callback {
                callback.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_bits_map_to_kinds() {
        let audio_video = media::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32
            | media::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32;
        assert_eq!(media_kinds(audio_video), (vec![PermissionKind::Microphone, PermissionKind::Camera], 0));
        let screen = media::CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32
            | media::CEF_MEDIA_PERMISSION_DESKTOP_AUDIO_CAPTURE as u32;
        assert_eq!(media_kinds(screen), (vec![PermissionKind::ScreenShare, PermissionKind::ScreenAudio], 0));
        assert_eq!(media_kinds(0), (vec![], 0));
    }

    #[test]
    fn prompt_bits_map_to_kinds_and_report_unknown_ones() {
        let bits = prompt::CEF_PERMISSION_TYPE_NOTIFICATIONS as u32
            | prompt::CEF_PERMISSION_TYPE_GEOLOCATION as u32
            | prompt::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32
            | prompt::CEF_PERMISSION_TYPE_CAMERA_PAN_TILT_ZOOM as u32;
        let (kinds, unknown) = prompt_kinds(bits);
        // Pan/tilt/zoom is part of the camera, listed once.
        assert_eq!(
            kinds,
            vec![PermissionKind::Camera, PermissionKind::Notifications, PermissionKind::Geolocation]
        );
        assert_eq!(unknown, 0);
        let vr = prompt::CEF_PERMISSION_TYPE_VR_SESSION as u32;
        assert_eq!(prompt_kinds(vr | prompt::CEF_PERMISSION_TYPE_CLIPBOARD as u32).1, vr);
        assert_eq!(prompt_kinds(LOCAL_NETWORK_BITS).0, vec![PermissionKind::LocalNetwork]);
    }

    #[test]
    fn screen_sharing_has_no_content_setting() {
        assert!(content_settings(PermissionKind::ScreenShare).is_empty());
        assert!(content_settings(PermissionKind::ScreenAudio).is_empty());
        for kind in PermissionKind::ALL.into_iter().filter(|k| !k.is_capture()) {
            assert!(!content_settings(kind).is_empty(), "{kind:?}");
            assert!(prompted_by_chromium(kind));
        }
    }

    #[test]
    fn kind_keys_round_trip() {
        for kind in PermissionKind::ALL {
            assert_eq!(PermissionKind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(PermissionKind::from_key("nope"), None);
    }
}
