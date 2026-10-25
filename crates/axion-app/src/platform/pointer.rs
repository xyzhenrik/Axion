//! Where the pointer is while something is dragged over the window from another app.
//!
//! During a drag the source app holds the pointer, so the window gets no pointer motion, and
//! winit 0.30 reports files dragged in (XDND) without their position. On X11 (and XWayland)
//! the server still knows where the pointer is: this asks it. winit 0.30 doesn't support
//! drag and drop on native Wayland at all.

use glam::Vec2;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::rust_connection::RustConnection;

pub struct PointerProbe {
    conn: RustConnection,
    root: u32,
}

impl PointerProbe {
    /// A connection to the window's X server; `None` if the window isn't on X11.
    pub fn connect(window: &Window) -> Option<Self> {
        let on_x11 = window
            .window_handle()
            .is_ok_and(|h| matches!(h.as_raw(), RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_)));
        if !on_x11 {
            return None;
        }
        let (conn, screen) = x11rb::connect(None)
            .inspect_err(|e| tracing::warn!("no X11 connection for drag positions: {e}"))
            .ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some(Self { conn, root })
    }

    /// The pointer in the window's logical pixels.
    pub fn position(&self, window: &Window) -> Option<Vec2> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        let origin = window.inner_position().ok()?;
        let physical =
            Vec2::new(f32::from(reply.root_x) - origin.x as f32, f32::from(reply.root_y) - origin.y as f32);
        Some(physical / window.scale_factor() as f32)
    }
}
