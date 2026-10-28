//! Axion's icon set: 24×24 stroke paths from Lucide (<https://lucide.dev>, ISC licence, see
//! `THIRD_PARTY_LICENSES`; lucide-static 0.468.0, each icon's elements merged into one path),
//! rasterized on demand at the exact pixel size into glyphon's atlas.

use glyphon::{ContentType, RasterizeCustomGlyphRequest, RasterizedCustomGlyph};

/// Lucide draws at 2 units; our regular stroke is 1.75 units in a 24-unit box.
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
    Back => "M15 18l-6-6 6-6",
    Forward => "M9 18l6-6-6-6",
    Sidebar => "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-14a2 2 0 0 1 2 -2zM9 3v18",
    Shield => "M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1zM9 12l2 2 4-4",
    Reload => "M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8M21 3v5h-5",
    Stop => "M18 6 6 18M6 6l12 12",
    Plus => "M5 12h14M12 5v14",
    Extensions => "M15.39 4.39a1 1 0 0 0 1.68-.474 2.5 2.5 0 1 1 3.014 3.015 1 1 0 0 0-.474 1.68l1.683 1.682a2.414 2.414 0 0 1 0 3.414L19.61 15.39a1 1 0 0 1-1.68-.474 2.5 2.5 0 1 0-3.014 3.015 1 1 0 0 1 .474 1.68l-1.683 1.682a2.414 2.414 0 0 1-3.414 0L8.61 19.61a1 1 0 0 0-1.68.474 2.5 2.5 0 1 1-3.014-3.015 1 1 0 0 0 .474-1.68l-1.683-1.682a2.414 2.414 0 0 1 0-3.414L4.39 8.61a1 1 0 0 1 1.68.474 2.5 2.5 0 1 0 3.014-3.015 1 1 0 0 1-.474-1.68l1.683-1.682a2.414 2.414 0 0 1 3.414 0z",
    Overview => "M4 3h5a1 1 0 0 1 1 1v5a1 1 0 0 1 -1 1h-5a1 1 0 0 1 -1 -1v-5a1 1 0 0 1 1 -1zM15 3h5a1 1 0 0 1 1 1v5a1 1 0 0 1 -1 1h-5a1 1 0 0 1 -1 -1v-5a1 1 0 0 1 1 -1zM15 14h5a1 1 0 0 1 1 1v5a1 1 0 0 1 -1 1h-5a1 1 0 0 1 -1 -1v-5a1 1 0 0 1 1 -1zM4 14h5a1 1 0 0 1 1 1v5a1 1 0 0 1 -1 1h-5a1 1 0 0 1 -1 -1v-5a1 1 0 0 1 1 -1z",
    TabGroup => "M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83zM2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17",
    Bookmark => "M19 21l-7-4-7 4V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v16z",
    History => "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8M3 3v5h5M12 7v5l4 2",
    Media => "M9 18V5l12-2v13M3 18a3 3 0 1 0 6 0a3 3 0 1 0 -6 0zM15 16a3 3 0 1 0 6 0a3 3 0 1 0 -6 0z",
    Search => "M3 11a8 8 0 1 0 16 0a8 8 0 1 0 -16 0zM21 21l-4.3-4.3",
    Share => "M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8M16 6L12 2L8 6M12 2L12 15",
    Close => "M18 6 6 18M6 6l12 12",
    PictureInPicture => "M21 9V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v10c0 1.1.9 2 2 2h4M14 13h6a2 2 0 0 1 2 2v3a2 2 0 0 1 -2 2h-6a2 2 0 0 1 -2 -2v-3a2 2 0 0 1 2 -2z",
    Fullscreen => "M8 3H5a2 2 0 0 0-2 2v3M21 8V5a2 2 0 0 0-2-2h-3M3 16v3a2 2 0 0 0 2 2h3M16 21h3a2 2 0 0 0 2-2v-3",
    Volume => "M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298zM16 9a5 5 0 0 1 0 6M19.364 18.364a9 9 0 0 0 0-12.728",
    Settings => "M21 4L14 4M10 4L3 4M21 12L12 12M8 12L3 12M21 20L16 20M12 20L3 20M14 2L14 6M8 10L8 14M16 18L16 22",
    Pause => "M15 4h2a1 1 0 0 1 1 1v14a1 1 0 0 1 -1 1h-2a1 1 0 0 1 -1 -1v-14a1 1 0 0 1 1 -1zM7 4h2a1 1 0 0 1 1 1v14a1 1 0 0 1 -1 1h-2a1 1 0 0 1 -1 -1v-14a1 1 0 0 1 1 -1z",
    More => "M11 12a1 1 0 1 0 2 0a1 1 0 1 0 -2 0zM18 12a1 1 0 1 0 2 0a1 1 0 1 0 -2 0zM4 12a1 1 0 1 0 2 0a1 1 0 1 0 -2 0z",
    Lock => "M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-7a2 2 0 0 1 2 -2zM7 11V7a5 5 0 0 1 10 0v4",
    ChevronDown => "M6 9l6 6 6-6",
    ChevronRight => "M9 18l6-6-6-6",
    Menu => "M4 12L20 12M4 6L20 6M4 18L20 18",
    ZoomIn => "M3 11a8 8 0 1 0 16 0a8 8 0 1 0 -16 0zM21 21L16.65 16.65M11 8L11 14M8 11L14 11",
    ZoomOut => "M3 11a8 8 0 1 0 16 0a8 8 0 1 0 -16 0zM21 21L16.65 16.65M8 11L14 11",
    Book => "M12 7v14M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z",
    Glasses => "M2 15a4 4 0 1 0 8 0a4 4 0 1 0 -8 0zM14 15a4 4 0 1 0 8 0a4 4 0 1 0 -8 0zM14 15a2 2 0 0 0-2-2 2 2 0 0 0-2 2M2.5 13 5 7c.7-1.3 1.4-2 3-2M21.5 13 19 7c-.7-1.3-1.5-2-3-2",
    Hand => "M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15",
    NewTabGroup => "M12 10v6M9 13h6M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
    Play => "M6 3L20 12L6 21L6 3z",
    Muted => "M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298zM22 9L16 15M16 9L22 15",
    Captions => "M5 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-10a2 2 0 0 1 2 -2zM7 15h4M15 15h2M7 11h2M13 11h4",
    SkipBack => "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8M3 3v5h5",
    SkipForward => "M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8M21 3v5h-5",
    ExitFullscreen => "M8 3v3a2 2 0 0 1-2 2H3M21 8h-3a2 2 0 0 1-2-2V3M3 16h3a2 2 0 0 1 2 2v3M16 21v-3a2 2 0 0 1 2-2h3",
    Gauge => "M12 14l4-4M3.34 19a10 10 0 1 1 17.32 0",
    Check => "M20 6 9 17l-5-5",
    Download => "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10L12 15L17 10M12 15L12 3",
    File => "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7ZM14 2v4a2 2 0 0 0 2 2h4",
    Folder => "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
    Key => "M15.5 7.5l2.3 2.3a1 1 0 0 0 1.4 0l2.1-2.1a1 1 0 0 0 0-1.4L19 4M21 2l-9.6 9.6M2 15.5a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0 -11 0z",
    Card => "M4 5h16a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-16a2 2 0 0 1 -2 -2v-10a2 2 0 0 1 2 -2zM2 10L22 10",
    Copy => "M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2v-10a2 2 0 0 1 2 -2zM4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2",
    Eye => "M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0z",
    EyeOff => "M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49M14.084 14.158a3 3 0 0 1-4.242-4.242M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143M2 2l20 20",
    Note => "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7ZM14 2v4a2 2 0 0 0 2 2h4M10 9H8M16 13H8M16 17H8",
    Home => "M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
    Trash => "M3 6h18M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2M10 11L10 17M14 11L14 17",
    Pencil => "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497zM15 5l4 4",
    Pin => "M12 17v5M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z",
    Minus => "M5 12h14",
    Globe => "M2 12a10 10 0 1 0 20 0a10 10 0 1 0 -20 0zM12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20M2 12h20",
    Person => "M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2M8 7a4 4 0 1 0 8 0a4 4 0 1 0 -8 0z",
    PersonAdd => "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M5 7a4 4 0 1 0 8 0a4 4 0 1 0 -8 0zM19 8L19 14M22 11L16 11",
    ChevronUpDown => "M7 15l5 5 5-5M7 9l5-5 5 5",
    ChevronUp => "M18 15l-6-6-6 6",
    Playlist => "M21 15V6M18.5 18a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5ZM12 12H3M16 6H3M12 18H3",
    PlaylistAdd => "M11 12H3M16 6H3M16 18H3M18 9v6M21 12h-6",
    Translate => "m5 8 6 6M4 14l6-6 2-3M2 5h12M7 2h1M22 22l-5-10-5 10M14 18h6",
    TextSize => "M21 14h-5M16 16v-3.5a2.5 2.5 0 0 1 5 0V16M4.5 13h6M3 16l4.5-9 4.5 9",
    Moon => "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z",
    Shuffle => "M18 14l4 4-4 4M18 2l4 4-4 4M2 18h1.973a4 4 0 0 0 3.3-1.7l5.454-8.6a4 4 0 0 1 3.3-1.7H22M2 6h1.972a4 4 0 0 1 3.6 2.2M22 18h-6.041a4 4 0 0 1-3.3-1.8l-.359-.45",
    Repeat => "M17 2l4 4-4 4M3 11v-1a4 4 0 0 1 4-4h14M7 22l-4-4 4-4M21 13v1a4 4 0 0 1-4 4H3",
    Sort => "M21 16l-4 4-4-4M17 20V4M3 8l4-4 4 4M7 4v16",
    Previous => "M19 20L9 12L19 4L19 20zM5 19L5 5",
    Next => "M5 4L15 12L5 20L5 4zM19 5L19 19",
    Code => "M18 16l4-4-4-4M6 8l-4 4 4 4M14.5 4l-5 16",
    Microphone => "M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3ZM19 10v2a7 7 0 0 1-14 0v-2M12 19L12 22",
    Camera => "M16 13l5.223 3.482a.5.5 0 0 0 .777-.416V7.87a.5.5 0 0 0-.752-.432L16 10.5M4 6h10a2 2 0 0 1 2 2v8a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2v-8a2 2 0 0 1 2 -2z",
    Screen => "M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-16a2 2 0 0 1 -2 -2v-10a2 2 0 0 1 2 -2zM8 21L16 21M12 17L12 21",
    Bell => "M10.268 21a2 2 0 0 0 3.464 0M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326",
    Location => "M20 10c0 4.993-5.539 10.193-7.399 11.799a1 1 0 0 1-1.202 0C9.539 20.193 4 14.993 4 10a8 8 0 0 1 16 0M9 10a3 3 0 1 0 6 0a3 3 0 1 0 -6 0z",
    Clipboard => "M9 2h6a1 1 0 0 1 1 1v2a1 1 0 0 1 -1 1h-6a1 1 0 0 1 -1 -1v-2a1 1 0 0 1 1 -1zM16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2",
    Info => "M2 12a10 10 0 1 0 20 0a10 10 0 1 0 -20 0zM12 16v-4M12 8h.01",
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

    /// Every path is valid SVG data (the merged Lucide elements) and, stroked at the regular
    /// weight, stays inside the 24-unit box: nothing is clipped at the edge of the glyph.
    #[test]
    fn every_icon_parses_and_fits_its_box() {
        for &icon in Icon::ALL {
            assert!(kurbo::BezPath::from_svg(icon.path()).is_ok(), "{icon:?} path does not parse");
            let size = 48usize;
            let data = mask(icon, size as u16, size as u16);
            let (x0, y0, x1, y1) = bounds(&data, size);
            assert!(
                x0 > 0 && y0 > 0 && x1 < size - 1 && y1 < size - 1,
                "{icon:?} touches the box edge: {:?}",
                (x0, y0, x1, y1)
            );
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
