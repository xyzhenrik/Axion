//! The screens and windows a page can be offered to share (X11 and XWayland).
//!
//! Chromium captures `screen:<id>:0` / `window:<xid>:0` sources: on X11 a screen's id is the
//! atom of its RandR monitor name, a window's the X window id, and `screen:-1:0` is every
//! screen together. This lists them with a small thumbnail of each. On Wayland no client may
//! list or grab other windows; sharing there goes through the desktop's screen cast portal
//! (its own picker), and this finds nothing.

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

/// What a source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Screen,
    Window,
}

/// A small RGBA picture.
#[derive(Clone, Debug)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One screen or window the user can share.
#[derive(Clone, Debug)]
pub struct Source {
    /// Chromium's desktop media id (`axion_engine::ScreenPick::id`).
    pub id: String,
    pub kind: SourceKind,
    pub name: String,
    pub thumbnail: Option<Thumbnail>,
}

/// Biggest area read back from the server for a thumbnail (pixels): bigger sources are
/// shown without one rather than stalling the browser on a huge transfer.
const MAX_THUMBNAIL_SOURCE_PIXELS: u64 = 3840 * 2160;

/// Lists screens first, then windows, at most `max` in all, with thumbnails `thumbnail_width`
/// pixels wide (16:9). `own_pid` leaves the browser's own windows out. Empty when there is
/// no X server to ask.
pub fn list(max: usize, thumbnail_width: u32, own_pid: u32) -> Vec<Source> {
    let Ok((conn, screen)) = x11rb::connect(None) else { return Vec::new() };
    let Some(root) = conn.setup().roots.get(screen).map(|s| s.root) else { return Vec::new() };
    let little_endian = conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
    let thumb_height = thumbnail_width * 9 / 16;
    let mut sources = Vec::new();

    let monitors = conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.monitors)
        .unwrap_or_default();
    if monitors.len() > 1 {
        let (w, h) = monitors.iter().fold((0i32, 0i32), |(w, h), m| {
            (w.max(i32::from(m.x) + i32::from(m.width)), h.max(i32::from(m.y) + i32::from(m.height)))
        });
        sources.push(Source {
            id: "screen:-1:0".into(),
            kind: SourceKind::Screen,
            name: axion_i18n::tr("All Screens").to_owned(),
            thumbnail: thumbnail(
                &conn,
                root,
                (0, 0, w.max(1) as u16, h.max(1) as u16),
                thumbnail_width,
                thumb_height,
                little_endian,
            ),
        });
    }
    for monitor in &monitors {
        let name = conn
            .get_atom_name(monitor.name)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .unwrap_or_default();
        sources.push(Source {
            id: format!("screen:{}:0", monitor.name),
            kind: SourceKind::Screen,
            name,
            thumbnail: thumbnail(
                &conn,
                root,
                (monitor.x, monitor.y, monitor.width, monitor.height),
                thumbnail_width,
                thumb_height,
                little_endian,
            ),
        });
    }
    if monitors.is_empty() {
        sources.push(Source {
            id: "screen:-1:0".into(),
            kind: SourceKind::Screen,
            name: axion_i18n::tr("Entire Screen").to_owned(),
            thumbnail: None,
        });
    }

    for window in client_windows(&conn, root) {
        if sources.len() >= max {
            break;
        }
        if window_pid(&conn, window) == Some(own_pid) {
            continue;
        }
        let name = window_title(&conn, window);
        if name.is_empty() {
            continue;
        }
        let Some(geometry) = conn.get_geometry(window).ok().and_then(|c| c.reply().ok()) else { continue };
        sources.push(Source {
            id: format!("window:{window}:0"),
            kind: SourceKind::Window,
            name,
            thumbnail: thumbnail(
                &conn,
                window,
                (0, 0, geometry.width, geometry.height),
                thumbnail_width,
                thumb_height,
                little_endian,
            ),
        });
    }
    sources.truncate(max);
    sources
}

fn atom(conn: &RustConnection, name: &str) -> Option<u32> {
    Some(conn.intern_atom(true, name.as_bytes()).ok()?.reply().ok()?.atom).filter(|a| *a != 0)
}

fn client_windows(conn: &RustConnection, root: Window) -> Vec<Window> {
    let Some(list) = atom(conn, "_NET_CLIENT_LIST") else { return Vec::new() };
    let Some(reply) =
        conn.get_property(false, root, list, AtomEnum::WINDOW, 0, 1024).ok().and_then(|c| c.reply().ok())
    else {
        return Vec::new();
    };
    reply.value32().map(Iterator::collect).unwrap_or_default()
}

fn window_pid(conn: &RustConnection, window: Window) -> Option<u32> {
    let pid = atom(conn, "_NET_WM_PID")?;
    let reply = conn.get_property(false, window, pid, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
    reply.value32()?.next()
}

fn window_title(conn: &RustConnection, window: Window) -> String {
    if let Some(name) = atom(conn, "_NET_WM_NAME")
        && let Some(utf8) = atom(conn, "UTF8_STRING")
        && let Some(reply) =
            conn.get_property(false, window, name, utf8, 0, 256).ok().and_then(|c| c.reply().ok())
        && !reply.value.is_empty()
    {
        return String::from_utf8_lossy(&reply.value).into_owned();
    }
    conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 256)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| String::from_utf8_lossy(&r.value).into_owned())
        .unwrap_or_default()
}

/// The area `(x, y, width, height)` of `drawable`, shrunk to fit `out_w` x `out_h` (nearest
/// neighbour, letterboxed on black). `None` if the server won't hand it over.
fn thumbnail(
    conn: &RustConnection,
    drawable: u32,
    area: (i16, i16, u16, u16),
    out_w: u32,
    out_h: u32,
    little_endian: bool,
) -> Option<Thumbnail> {
    let (x, y, w, h) = area;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_THUMBNAIL_SOURCE_PIXELS || !little_endian {
        return None;
    }
    let image = conn.get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, !0).ok()?.reply().ok()?;
    // Only the common 32 bits per pixel layout (BGRx) is read.
    if image.data.len() < usize::from(w) * usize::from(h) * 4 {
        return None;
    }
    let (sw, sh) = (u32::from(w), u32::from(h));
    let scale = (out_w as f32 / sw as f32).min(out_h as f32 / sh as f32);
    let (tw, th) =
        (((sw as f32 * scale) as u32).clamp(1, out_w), ((sh as f32 * scale) as u32).clamp(1, out_h));
    // Opaque black background (the letterbox).
    let mut rgba = [0u8, 0, 0, 255].repeat((out_w * out_h) as usize);
    let (ox, oy) = ((out_w - tw) / 2, (out_h - th) / 2);
    for ty in 0..th {
        let sy = (ty * sh / th).min(sh - 1);
        for tx in 0..tw {
            let sx = (tx * sw / tw).min(sw - 1);
            let src = ((sy * sw + sx) * 4) as usize;
            let dst = (((oy + ty) * out_w + ox + tx) * 4) as usize;
            rgba[dst] = image.data[src + 2];
            rgba[dst + 1] = image.data[src + 1];
            rgba[dst + 2] = image.data[src];
        }
    }
    Some(Thumbnail { width: out_w, height: out_h, rgba })
}
