//! Dragging a tab out of the window into other apps: the drop there is the tab's link (a
//! `text/uri-list` for browsers and file managers, plain text for text fields).
//!
//! winit 0.30 cannot be a drag source, so each display server gets its own implementation:
//! Wayland starts a `wl_data_source` drag on winit's own connection ([`wayland`]); X11 (and
//! XWayland) speaks XDND from a small thread of its own ([`x11`]).

mod wayland;
mod x11;

use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

/// What a dropped tab carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub url: String,
}

impl Payload {
    /// `text/uri-list` (RFC 2483: CRLF-terminated lines).
    pub fn uri_list(&self) -> Vec<u8> {
        format!("{}\r\n", self.url).into_bytes()
    }

    /// `text/plain`: the link itself.
    pub fn plain(&self) -> Vec<u8> {
        self.url.clone().into_bytes()
    }
}

/// The MIME types a drop offers, in order of preference.
pub const MIME_URI_LIST: &str = "text/uri-list";
pub const MIME_PLAIN_UTF8: &str = "text/plain;charset=utf-8";
pub const MIME_PLAIN: &str = "text/plain";

/// The bytes of `mime` for `payload`, if it is one of ours.
pub fn data_for(payload: &Payload, mime: &str) -> Option<Vec<u8>> {
    match mime {
        MIME_URI_LIST => Some(payload.uri_list()),
        MIME_PLAIN_UTF8 | MIME_PLAIN | "UTF8_STRING" | "STRING" | "TEXT" => Some(payload.plain()),
        _ => None,
    }
}

enum Backend {
    Wayland(wayland::Source),
    X11(x11::Source),
}

/// Starts drags out of the window. One per window, made while it is shown (the Wayland side has
/// to listen to the pointer from the start to know the serial of the press that begins a drag).
pub struct DragOut {
    backend: Backend,
}

impl DragOut {
    /// Connects to the window's display server; `None` if the drag cannot be started there.
    pub fn connect(window: &Window) -> Option<Self> {
        let display = window.display_handle().ok()?.as_raw();
        let backend = match display {
            RawDisplayHandle::Wayland(handle) => {
                let surface = match window.window_handle().ok()?.as_raw() {
                    RawWindowHandle::Wayland(h) => h.surface,
                    _ => return None,
                };
                Backend::Wayland(wayland::Source::connect(handle.display, surface)?)
            }
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => {
                let id = match window.window_handle().ok()?.as_raw() {
                    RawWindowHandle::Xlib(h) => h.window as u32,
                    RawWindowHandle::Xcb(h) => h.window.get(),
                    _ => return None,
                };
                Backend::X11(x11::Source::connect(id)?)
            }
            _ => return None,
        };
        Some(Self { backend })
    }

    /// Starts the drag of `payload` from the button press that is held now. Returns whether the
    /// system took it (then the pointer belongs to the drag until it ends).
    pub fn start(&mut self, payload: Payload) -> bool {
        match &mut self.backend {
            Backend::Wayland(source) => source.start(payload),
            Backend::X11(source) => source.start(payload),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_formats() {
        let p = Payload { url: "https://example.com/a?b=1".into() };
        assert_eq!(data_for(&p, MIME_URI_LIST).as_deref(), Some(&b"https://example.com/a?b=1\r\n"[..]));
        assert_eq!(data_for(&p, MIME_PLAIN).as_deref(), Some(&b"https://example.com/a?b=1"[..]));
        assert_eq!(data_for(&p, "UTF8_STRING"), data_for(&p, MIME_PLAIN_UTF8));
        assert_eq!(data_for(&p, "image/png"), None);
    }
}
