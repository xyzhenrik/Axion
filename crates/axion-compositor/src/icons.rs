//! Axion's icon set: 24×24 stroke paths from the design system ("Foundations", Icons),
//! rasterized on demand at the exact pixel size into glyphon's atlas.

use glyphon::{ContentType, RasterizeCustomGlyphRequest, RasterizedCustomGlyph};

/// The design's icon stroke is 1.75 units in a 24-unit box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconWeight {
    #[default]
    Regular,
    /// Close buttons on thumbnails (2.2).
    Bold,
    /// Play / pause and "more" dots (2.6).
    Heavy,
}

impl IconWeight {
    const ALL: [IconWeight; 3] = [IconWeight::Regular, IconWeight::Bold, IconWeight::Heavy];

    fn stroke(self) -> f32 {
        match self {
            IconWeight::Regular => 1.75,
            IconWeight::Bold => 2.2,
            IconWeight::Heavy => 2.6,
        }
    }
}

macro_rules! icons {
    ($($name:ident => $path:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($name,)* }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$name,)*];

            /// SVG path data in a 24×24 box.
            pub fn path(self) -> &'static str {
                match self { $(Icon::$name => $path,)* }
            }
        }
    };
}

icons! {
    Back => "M15 5l-7 7 7 7",
    Forward => "M9 5l7 7-7 7",
    Sidebar => "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9.5 4v16",
    Shield => "M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6zM9 12l2 2 4-4",
    Reload => "M20 12a8 8 0 1 1-2.34-5.66M20 4v5h-5",
    Stop => "M6.5 6.5l11 11M17.5 6.5l-11 11",
    Plus => "M12 5v14M5 12h14",
    Extensions => "M9.5 4.5a2 2 0 0 1 4 0V6H17a1.5 1.5 0 0 1 1.5 1.5V11H17a2 2 0 0 0 0 4h1.5v3.5A1.5 1.5 0 0 1 17 20H6.5A1.5 1.5 0 0 1 5 18.5V15h1.5a2 2 0 0 0 0-4H5V7.5A1.5 1.5 0 0 1 6.5 6h3z",
    Overview => "M6 4h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM15 4h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM6 13h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2zM15 13h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2z",
    TabGroup => "M9 8h10a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 19 20H9a1.5 1.5 0 0 1-1.5-1.5v-9A1.5 1.5 0 0 1 9 8zM4 16V5.5A1.5 1.5 0 0 1 5.5 4H16",
    Bookmark => "M7 3.5h10a1 1 0 0 1 1 1V21l-6-4-6 4V4.5a1 1 0 0 1 1-1z",
    History => "M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM12 7.5V12l3 2",
    Media => "M9 18V5.5l11-2V16M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM20 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
    Search => "M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zM16.2 16.2L20.5 20.5",
    Share => "M12 3.5v11M8 7.5l4-4 4 4M5.5 12v7a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5v-7",
    Close => "M6.5 6.5l11 11M17.5 6.5l-11 11",
    PictureInPicture => "M4.5 5h15A1.5 1.5 0 0 1 21 6.5v11a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5v-11A1.5 1.5 0 0 1 4.5 5zM12.5 12h5.5v4.5h-5.5z",
    Fullscreen => "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
    Volume => "M4 9.5h3.5L12.5 5.5v13l-5-4H4zM16 9a4.5 4.5 0 0 1 0 6M18.5 6.5a8 8 0 0 1 0 11",
    Settings => "M4 7h9M17 7h3M4 17h3M11 17h9M15 5v4M9 15v4",
    Pause => "M8.5 5.5v13M15.5 5.5v13",
    More => "M5.5 12h.01M12 12h.01M18.5 12h.01",
    Lock => "M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1zM8 11V7a4 4 0 0 1 8 0v4",
    ChevronDown => "M6 9l6 6 6-6",
    ChevronRight => "M9 6l6 6-6 6",
    Menu => "M5 7h14M5 12h14M5 17h9",
    ZoomIn => "M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zM16.2 16.2L20.5 20.5M8 11h6M11 8v6",
    ZoomOut => "M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zM16.2 16.2L20.5 20.5M8 11h6",
    Book => "M12 6.5C10 5 7 4.5 4 5v13c3-.5 6 0 8 1.5 2-1.5 5-2 8-1.5V5c-3-.5-6 0-8 1.5zM12 6.5V19.5",
    Glasses => "M6.5 11.5a3.5 3.5 0 1 1 0 7 3.5 3.5 0 0 1 0-7zM17.5 11.5a3.5 3.5 0 1 1 0 7 3.5 3.5 0 0 1 0-7zM10 15c1.3-.8 2.7-.8 4 0M3 15V9.5M21 15V9.5",
    Hand => "M8.5 12.5V5.5a1.5 1.5 0 0 1 3 0v6M11.5 11V4a1.5 1.5 0 0 1 3 0v7M14.5 11V5.5a1.5 1.5 0 0 1 3 0V13M8.5 12.5V10a1.5 1.5 0 0 0-3 0v4.5C5.5 18.5 8 21 12 21s5.5-3 5.5-6.5V10.5",
    NewTabGroup => "M8 8h10a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2zM4 15V6a2 2 0 0 1 2-2h9M13 11v6M10 14h6",
    Play => "M8 5.2v13.6l11-6.8z",
    Muted => "M4 9.5h3.5L12.5 5.5v13l-5-4H4zM16.5 9.5l5 5M21.5 9.5l-5 5",
    Captions => "M4.5 5.5h15A1.5 1.5 0 0 1 21 7v10a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17V7a1.5 1.5 0 0 1 1.5-1.5zM10.5 10.2a2.5 2.5 0 1 0 0 3.6M17 10.2a2.5 2.5 0 1 0 0 3.6",
    SkipBack => "M4 12a8 8 0 1 0 2.34-5.66M4 4v5h5",
    SkipForward => "M20 12a8 8 0 1 1-2.34-5.66M20 4v5h-5",
    ExitFullscreen => "M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5",
    Gauge => "M4.5 16.5a7.5 7.5 0 1 1 15 0M12 16.5l3.5-4.5M7.5 16.5h.01M16.5 16.5h.01",
    Check => "M5 12.5l4.5 4.5L19 7.5",
    Download => "M12 4v11M7.5 10.5L12 15l4.5-4.5M5 19.5h14",
    File => "M7 3.5h6.5L18 8v11a1.5 1.5 0 0 1-1.5 1.5h-9.5A1.5 1.5 0 0 1 5.5 19V5A1.5 1.5 0 0 1 7 3.5zM13.5 3.5V8H18",
    Folder => "M4.5 6h5l2 2h8A1.5 1.5 0 0 1 21 9.5v8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5v-10A1.5 1.5 0 0 1 4.5 6z",
    Key => "M8 11a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM10.8 12.2L20 3M16.5 6.5l2.5 2.5M14 9l2 2",
    Card => "M4.5 5.5h15A1.5 1.5 0 0 1 21 7v10a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17V7a1.5 1.5 0 0 1 1.5-1.5zM3 10h18M6.5 15h4",
    Copy => "M9 8h9.5A1.5 1.5 0 0 1 20 9.5v9a1.5 1.5 0 0 1-1.5 1.5H9a1.5 1.5 0 0 1-1.5-1.5v-9A1.5 1.5 0 0 1 9 8zM4 15.5v-10A1.5 1.5 0 0 1 5.5 4H15",
    Eye => "M2.5 12C5 7.5 8.5 5.5 12 5.5s7 2 9.5 6.5c-2.5 4.5-6 6.5-9.5 6.5S5 16.5 2.5 12zM12 9a3 3 0 1 1 0 6 3 3 0 0 1 0-6z",
    EyeOff => "M2.5 12C5 7.5 8.5 5.5 12 5.5s7 2 9.5 6.5c-2.5 4.5-6 6.5-9.5 6.5S5 16.5 2.5 12zM12 9a3 3 0 1 1 0 6 3 3 0 0 1 0-6zM4 4l16 16",
    Note => "M6 3.5h8l4.5 4.5v12a.5.5 0 0 1-.5.5H6a.5.5 0 0 1-.5-.5V4a.5.5 0 0 1 .5-.5zM14 3.5V8h4.5M8.5 12.5h7M8.5 16h5",
    Home => "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5",
    Trash => "M4.5 7h15M9.5 7V4.5h5V7M6.5 7l1 13h9l1-13M10 11v5.5M14 11v5.5",
    Pencil => "M15.5 4.5l4 4-11 11-4.5.5.5-4.5zM13.5 6.5l4 4",
    Pin => "M9 3.5h6M10 3.5v6l-3.5 4.5h11L14 9.5v-6M12 14v6.5",
    Minus => "M5 12h14",
    Globe => "M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM3 12h18M12 3c-2.6 2.5-3.9 5.5-3.9 9s1.3 6.5 3.9 9M12 3c2.6 2.5 3.9 5.5 3.9 9s-1.3 6.5-3.9 9",
    Person => "M12 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM4.5 20.5c.8-3.8 3.8-6 7.5-6s6.7 2.2 7.5 6",
    PersonAdd => "M10 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM3 20.5c.8-3.8 3.4-6 7-6 1.6 0 3 .4 4.2 1.2M18.5 14v6M15.5 17h6",
    ChevronUpDown => "M8 9.5l4-4 4 4M8 14.5l4 4 4-4",
    ChevronUp => "M6 15l6-6 6 6",
    Playlist => "M4 6.5h11M4 11.5h7M4 16.5h5M15 12.5v7l6-3.5z",
    PlaylistAdd => "M4 6.5h11M4 11.5h7M4 16.5h5M17 12v8M13 16h8",
    Moon => "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z",
    Shuffle => "M4 7h3c5 0 5 10 10 10h3M4 17h3c1.5 0 2.6-.8 3.5-2M13.5 9c1-1.3 2.3-2 3.5-2h3M17.5 4.5L20 7l-2.5 2.5M17.5 14.5L20 17l-2.5 2.5",
    Repeat => "M5 11V9a3 3 0 0 1 3-3h11M16 3l3 3-3 3M19 13v2a3 3 0 0 1-3 3H5M8 21l-3-3 3-3",
    Sort => "M7 4.5v15M3.5 16l3.5 3.5 3.5-3.5M17 19.5v-15M13.5 8L17 4.5 20.5 8",
    Previous => "M18 5.5v13l-9-6.5zM6 5.5v13",
    Next => "M6 5.5v13l9-6.5zM18 5.5v13",
    Code => "M8.5 7L3.5 12l5 5M15.5 7l5 5-5 5M13.5 5l-3 14",
    Microphone => "M12 3a3 3 0 0 1 3 3v5a3 3 0 0 1-6 0V6a3 3 0 0 1 3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M8.5 21h7",
    Camera => "M4.5 7h9A1.5 1.5 0 0 1 15 8.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 3 15.5v-7A1.5 1.5 0 0 1 4.5 7zM15 10.5l6-3.5v10l-6-3.5",
    Screen => "M4.5 4.5h15A1.5 1.5 0 0 1 21 6v9.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 15.5V6a1.5 1.5 0 0 1 1.5-1.5zM8.5 21h7M12 17v4",
    Bell => "M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0",
    Location => "M12 21s-6.5-6-6.5-11a6.5 6.5 0 0 1 13 0c0 5-6.5 11-6.5 11zM12 7.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z",
    Clipboard => "M9 3.5h6v3H9zM9 5H6.5A1.5 1.5 0 0 0 5 6.5v13A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5v-13A1.5 1.5 0 0 0 17.5 5H15",
    Info => "M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM12 11v5.5M12 7.5h.01",
}

/// Rotated icons (a spinning chevron) are rasterized in steps of this many degrees.
const ROTATION_STEP: f32 = 4.0;
/// Rotations are limited to ±this (degrees); springs overshooting further are clamped.
const ROTATION_LIMIT: f32 = 180.0;
const ROTATION_STEPS: usize = (2.0 * ROTATION_LIMIT / ROTATION_STEP) as usize + 1;

/// Stroke multipliers are rasterized in steps of 1/100 (the cache key), within these limits.
const STROKE_MIN: f32 = 0.5;
const STROKE_MAX: f32 = 2.0;

/// The stroke multiplier as its cache-key step (hundredths); 1.0 -> 100.
pub(crate) fn stroke_key(multiplier: f32) -> u16 {
    let m = if multiplier.is_finite() { multiplier } else { 1.0 };
    (m.clamp(STROKE_MIN, STROKE_MAX) * 100.0).round() as u16
}

/// Stroke width in the 24-unit box for a weight and a multiplier key (see [`stroke_key`]).
pub(crate) fn stroke_width(weight: IconWeight, key: u16) -> f32 {
    weight.stroke() * (key as f32 / 100.0)
}

/// What a glyphon custom glyph id stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    icon: u16,
    weight: u8,
    stroke: u16,
    rotation: u16,
}

/// glyphon's glyph ids are 16 bits, too few to pack icon x weight x stroke x rotation, so ids are
/// handed out on first use and live as long as the process (the atlas cache is keyed by them,
/// so a changed stroke multiplier gets a new id and is rasterized afresh).
#[derive(Default)]
struct Registry {
    keys: Vec<GlyphKey>,
    ids: std::collections::HashMap<GlyphKey, u16>,
}

static REGISTRY: std::sync::Mutex<Option<Registry>> = std::sync::Mutex::new(None);

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = REGISTRY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(Registry::default))
}

/// A glyph id for icon, weight, stroke multiplier and rotation (degrees, clockwise).
pub(crate) fn glyph_id(icon: Icon, weight: IconWeight, stroke: f32, rotation: f32) -> u16 {
    let key = GlyphKey {
        icon: Icon::ALL.iter().position(|&x| x == icon).unwrap_or(0) as u16,
        weight: IconWeight::ALL.iter().position(|&x| x == weight).unwrap_or(0) as u8,
        stroke: stroke_key(stroke),
        rotation: ((rotation.clamp(-ROTATION_LIMIT, ROTATION_LIMIT) + ROTATION_LIMIT) / ROTATION_STEP).round()
            as u16,
    };
    with_registry(|r| {
        if let Some(&id) = r.ids.get(&key) {
            return id;
        }
        let Ok(id) = u16::try_from(r.keys.len()) else {
            // Every id is taken (tens of thousands of distinct icon variants): reuse the first.
            return 0;
        };
        r.keys.push(key);
        r.ids.insert(key, id);
        id
    })
}

/// The icon, weight, stroke multiplier key and rotation behind a glyph id.
fn from_glyph_id(id: u16) -> Option<(Icon, IconWeight, u16, f32)> {
    let key = with_registry(|r| r.keys.get(id as usize).copied())?;
    if key.rotation as usize >= ROTATION_STEPS {
        return None;
    }
    Some((
        *Icon::ALL.get(key.icon as usize)?,
        *IconWeight::ALL.get(key.weight as usize)?,
        key.stroke,
        key.rotation as f32 * ROTATION_STEP - ROTATION_LIMIT,
    ))
}

/// glyphon's rasterizer callback: strokes the icon into an alpha mask.
pub(crate) fn rasterize(request: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
    let (icon, weight, stroke_key, rotation) = from_glyph_id(request.id)?;
    let (w, h) = (request.width as u32, request.height as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    let path = kurbo::BezPath::from_svg(icon.path()).ok()?;
    let scale = w.min(h) as f64 / 24.0;

    let mut builder = tiny_skia::PathBuilder::new();
    for el in path.elements() {
        let p = |pt: kurbo::Point| ((pt.x * scale) as f32, (pt.y * scale) as f32);
        match *el {
            kurbo::PathEl::MoveTo(a) => {
                let (x, y) = p(a);
                builder.move_to(x, y);
            }
            kurbo::PathEl::LineTo(a) => {
                let (x, y) = p(a);
                builder.line_to(x, y);
            }
            kurbo::PathEl::QuadTo(a, b) => {
                let ((x1, y1), (x, y)) = (p(a), p(b));
                builder.quad_to(x1, y1, x, y);
            }
            kurbo::PathEl::CurveTo(a, b, c) => {
                let ((x1, y1), (x2, y2), (x, y)) = (p(a), p(b), p(c));
                builder.cubic_to(x1, y1, x2, y2, x, y);
            }
            kurbo::PathEl::ClosePath => builder.close(),
        }
    }
    let path = builder.finish()?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let stroke = tiny_skia::Stroke {
        width: stroke_width(weight, stroke_key) * scale as f32,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    let transform = if rotation == 0.0 {
        // A square box (every resting icon) is drawn as is. A box stretched by the glass it
        // sits on (held buttons) stretches the icon with it, per axis, about the box's centre.
        let side = w.min(h) as f32;
        if w == h {
            tiny_skia::Transform::identity()
        } else {
            tiny_skia::Transform::from_scale(w as f32 / side, h as f32 / side)
        }
    } else {
        tiny_skia::Transform::from_rotate_at(rotation, w as f32 / 2.0, h as f32 / 2.0)
    };
    pixmap.stroke_path(&path, &paint, &stroke, transform, None);
    let data = pixmap.pixels().iter().map(|px| px.alpha()).collect();
    Some(RasterizedCustomGlyph { data, content_type: ContentType::Mask })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(icon: Icon, w: u16, h: u16) -> Vec<u8> {
        let request = RasterizeCustomGlyphRequest {
            id: glyph_id(icon, IconWeight::Regular, 1.0, 0.0),
            width: w,
            height: h,
            x_bin: glyphon::cosmic_text::SubpixelBin::Zero,
            y_bin: glyphon::cosmic_text::SubpixelBin::Zero,
            scale: 1.0,
        };
        rasterize(request).expect("rasterizes").data
    }

    /// Bounding box (x0, y0, x1, y1) of the pixels with alpha above a threshold.
    fn bounds(data: &[u8], w: usize) -> (usize, usize, usize, usize) {
        let mut b = (usize::MAX, usize::MAX, 0, 0);
        for (i, &a) in data.iter().enumerate() {
            if a > 40 {
                let (x, y) = (i % w, i / w);
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            }
        }
        b
    }

    /// A glyph box stretched by its glass (taller than wide) stretches the icon with it and
    /// keeps it centred in the box, instead of leaving it in the box's top-left corner.
    #[test]
    fn a_stretched_box_stretches_the_icon_about_its_centre() {
        let square = bounds(&mask(Icon::Plus, 20, 20), 20);
        let tall = bounds(&mask(Icon::Plus, 20, 30), 20);
        let (sw, sh) = (square.2 - square.0, square.3 - square.1);
        let (tw, th) = (tall.2 - tall.0, tall.3 - tall.1);
        assert_eq!(sw, sh, "a plus is square at rest");
        assert!(th as f32 > sh as f32 * 1.35 && tw.abs_diff(sw) <= 1, "stretched: {square:?} → {tall:?}");
        // Centred in the 20×30 box.
        let (cx, cy) = ((tall.0 + tall.2) as f32 / 2.0, (tall.1 + tall.3) as f32 / 2.0);
        assert!((cx - 10.0).abs() <= 1.0 && (cy - 15.0).abs() <= 1.0, "centre ({cx}, {cy})");
    }

    #[test]
    fn every_icon_rasterizes() {
        for &icon in Icon::ALL {
            for weight in IconWeight::ALL {
                let id = glyph_id(icon, weight, 1.0, 0.0);
                assert_eq!(from_glyph_id(id), Some((icon, weight, 100, 0.0)));
                let request = RasterizeCustomGlyphRequest {
                    id,
                    width: 32,
                    height: 32,
                    x_bin: glyphon::cosmic_text::SubpixelBin::Zero,
                    y_bin: glyphon::cosmic_text::SubpixelBin::Zero,
                    scale: 1.0,
                };
                let glyph = rasterize(request).unwrap_or_else(|| panic!("{icon:?} failed"));
                assert_eq!(glyph.data.len(), 32 * 32);
                assert!(glyph.data.iter().any(|&a| a > 128), "{icon:?} is empty");
            }
        }
    }

    fn mask_with(stroke: f32, size: u16) -> Vec<u8> {
        let request = RasterizeCustomGlyphRequest {
            id: glyph_id(Icon::Back, IconWeight::Regular, stroke, 0.0),
            width: size,
            height: size,
            x_bin: glyphon::cosmic_text::SubpixelBin::Zero,
            y_bin: glyphon::cosmic_text::SubpixelBin::Zero,
            scale: 1.0,
        };
        rasterize(request).expect("rasterizes").data
    }

    #[test]
    fn stroke_multiplier_scales_the_stroke_width() {
        assert_eq!(stroke_key(1.0), 100);
        assert_eq!(stroke_width(IconWeight::Regular, stroke_key(1.0)), 1.75);
        assert!((stroke_width(IconWeight::Regular, stroke_key(1.2)) - 2.1).abs() < 1e-5);
        assert!((stroke_width(IconWeight::Heavy, stroke_key(1.2)) - 3.12).abs() < 1e-5);
        // Out-of-range and non-finite values are limited.
        assert_eq!(stroke_key(0.0), 50);
        assert_eq!(stroke_key(9.0), 200);
        assert_eq!(stroke_key(f32::NAN), 100);
    }

    #[test]
    fn the_cache_key_includes_the_stroke_weight() {
        let regular = glyph_id(Icon::Plus, IconWeight::Regular, 1.0, 0.0);
        let medium = glyph_id(Icon::Plus, IconWeight::Regular, 1.2, 0.0);
        assert_ne!(regular, medium);
        assert_eq!(regular, glyph_id(Icon::Plus, IconWeight::Regular, 1.0, 0.0));
        // Steps finer than 1/100 share a key; coarser ones don't.
        assert_eq!(medium, glyph_id(Icon::Plus, IconWeight::Regular, 1.201, 0.0));
        assert_ne!(medium, glyph_id(Icon::Plus, IconWeight::Regular, 1.25, 0.0));
        // A heavier stroke inks more of the mask.
        let ink = |m: &[u8]| m.iter().map(|&a| a as u32).sum::<u32>();
        assert!(ink(&mask_with(1.2, 24)) > ink(&mask_with(1.0, 24)) * 11 / 10);
    }

    /// A multiplier of 1.0 (the dark default) draws exactly the original stroke.
    #[test]
    fn a_unit_multiplier_keeps_the_original_rasterization() {
        assert_eq!(mask(Icon::Back, 24, 24), mask_with(1.0, 24));
    }
}
