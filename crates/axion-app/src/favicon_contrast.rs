//! Making site icons readable on the chrome.
//!
//! A lot of icons are a single dark glyph on a transparent ground (GitHub's octocat, Notion,
//! X): invisible on dark chrome. Sites that care ship a second icon for dark pages (see
//! `favicons.rs`); for the others the icon is checked against the chrome's ground tone, and
//! when most of it would fade into it, it is either inverted (single-colour glyphs, like Safari
//! and Arc do) or put on a small tile in the opposite tone. Icons that are visible, and
//! coloured ones, are never touched. Pure functions on straight-alpha RGBA pixels.

use axion_config::FaviconContrast;

/// Contrast (WCAG ratio) an icon needs against the chrome to count as visible.
const MIN_CONTRAST: f32 = 3.0;
/// How much of `MIN_CONTRAST` a fully saturated colour is excused.
const COLOUR_RELIEF: f32 = 1.5;
/// Share of the icon's pixels (by opacity) that must be below `MIN_CONTRAST` to act.
const WEAK_SHARE: f32 = 0.7;
/// Mean saturation (0-1) below which an icon counts as a single-colour glyph.
const MONO_SATURATION: f32 = 0.16;
/// Relative luminance of the chrome ground the icons sit on, in dark and light chrome.
const GROUND_DARK: f32 = 0.035;
const GROUND_LIGHT: f32 = 0.88;
/// The tile's smallest side, its corner radius (share of the side) and the icon's share of it.
const TILE_MIN_SIDE: u32 = 48;
const TILE_CORNER: f32 = 0.22;
const TILE_ICON_SHARE: f32 = 0.74;

/// How an icon looks against the chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Share (0-1, by opacity) of the icon that has less than `MIN_CONTRAST` against the chrome.
    pub weak: f32,
    /// Mean saturation (0-1) of the icon's visible pixels.
    pub saturation: f32,
    /// Whether there is anything visible at all.
    pub visible: bool,
}

impl Look {
    /// The icon fades into the chrome.
    pub fn is_lost(&self) -> bool {
        self.visible && self.weak >= WEAK_SHARE
    }

    /// The icon is a single-colour glyph.
    pub fn is_mono(&self) -> bool {
        self.saturation <= MONO_SATURATION
    }
}

fn linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// WCAG relative luminance of an sRGB colour.
pub fn luminance(r: u8, g: u8, b: u8) -> f32 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

fn ground(dark: bool) -> f32 {
    if dark { GROUND_DARK } else { GROUND_LIGHT }
}

/// Measures `rgba` against the chrome ground (`dark`: dark chrome).
pub fn analyse(rgba: &[u8], dark: bool) -> Look {
    let ground = ground(dark);
    let (mut weight, mut weak, mut saturation) = (0.0f32, 0.0f32, 0.0f32);
    for px in rgba.as_chunks::<4>().0 {
        let alpha = f32::from(px[3]) / 255.0;
        if alpha < 0.08 {
            continue;
        }
        weight += alpha;
        let (hi, lo) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
        let sat = f32::from(hi - lo) / 255.0;
        // Saturated colours stand out at a lower luminance contrast than greys do.
        let needed = MIN_CONTRAST - COLOUR_RELIEF * sat;
        if contrast(luminance(px[0], px[1], px[2]), ground) < needed {
            weak += alpha;
        }
        saturation += alpha * sat;
    }
    if weight < 1.0 {
        return Look { weak: 0.0, saturation: 0.0, visible: false };
    }
    Look { weak: weak / weight, saturation: saturation / weight, visible: true }
}

/// What to show instead of `rgba` (`width` × `height`) on chrome of the given tone: `None`
/// when it is fine as it is (or `mode` is off). Returns the new size and pixels.
pub fn adapt(
    rgba: &[u8],
    width: u32,
    height: u32,
    dark: bool,
    mode: FaviconContrast,
) -> Option<(u32, u32, Vec<u8>)> {
    if mode == FaviconContrast::Off || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let look = analyse(rgba, dark);
    if !look.is_lost() {
        return None;
    }
    if mode == FaviconContrast::Invert && look.is_mono() {
        let mut out = rgba.to_vec();
        for px in out.as_chunks_mut::<4>().0 {
            for c in &mut px[..3] {
                *c = 255 - *c;
            }
        }
        return Some((width, height, out));
    }
    backed(rgba, width, height, dark)
}

/// `rgba` centred on a rounded tile in the tone opposite to the chrome's, with a hairline.
fn backed(rgba: &[u8], width: u32, height: u32, dark: bool) -> Option<(u32, u32, Vec<u8>)> {
    let side = width.max(height).max(TILE_MIN_SIDE);
    let source = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    // The icon, scaled to fit the tile's inner box.
    let inner = (side as f32 * TILE_ICON_SHARE).round().max(1.0);
    let scale = (inner / width as f32).min(inner / height as f32);
    let iw = (width as f32 * scale).round().max(1.0) as u32;
    let ih = (height as f32 * scale).round().max(1.0) as u32;
    let icon = image::imageops::resize(&source, iw, ih, image::imageops::FilterType::Triangle);
    let (ox, oy) = ((side - iw) / 2, (side - ih) / 2);

    let (tile, line): ([f32; 3], [f32; 3]) = if dark {
        ([238.0, 238.0, 242.0], [255.0, 255.0, 255.0])
    } else {
        ([40.0, 40.0, 44.0], [0.0, 0.0, 0.0])
    };
    let radius = side as f32 * TILE_CORNER;
    let half = side as f32 / 2.0;
    let hair = (side as f32 / 56.0).max(1.0);
    let mut out = vec![0u8; side as usize * side as usize * 4];
    for y in 0..side {
        for x in 0..side {
            // Signed distance to the rounded square (negative inside), at the pixel centre.
            let (px, py) = (x as f32 + 0.5 - half, y as f32 + 0.5 - half);
            let (qx, qy) = (px.abs() - (half - radius), py.abs() - (half - radius));
            let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            let cover = (0.5 - dist).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let ring = (1.0 - ((-dist) / hair).clamp(0.0, 1.0)) * 0.28;
            let mut color = [0.0f32; 3];
            for i in 0..3 {
                color[i] = tile[i] * (1.0 - ring) + line[i] * ring;
            }
            // The icon over the tile (straight alpha).
            if x >= ox && y >= oy && x < ox + iw && y < oy + ih {
                let p = icon.get_pixel(x - ox, y - oy).0;
                let a = f32::from(p[3]) / 255.0;
                for i in 0..3 {
                    color[i] = f32::from(p[i]) * a + color[i] * (1.0 - a);
                }
            }
            let at = (y as usize * side as usize + x as usize) * 4;
            for i in 0..3 {
                out[at + i] = color[i].round().clamp(0.0, 255.0) as u8;
            }
            out[at + 3] = (cover * 255.0).round() as u8;
        }
    }
    Some((side, side, out))
}

/// [`adapt`] for a cached PNG file's content; returns the PNG to draw instead, if any.
pub fn adapt_png(png: &[u8], dark: bool, mode: FaviconContrast) -> Option<Vec<u8>> {
    if mode == FaviconContrast::Off {
        return None;
    }
    let image = image::load_from_memory(png).ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    let (w, h, rgba) = adapt(image.as_raw(), w, h, dark, mode)?;
    let mut out = Vec::new();
    image::RgbaImage::from_raw(w, h, rgba)?
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// An SVG icon with its `prefers-color-scheme` media queries decided for `dark` chrome
/// (a renderer would evaluate them against the page, not the chrome): the wanted scheme's
/// queries become always-true, the other's never. `None` when the SVG has no such query.
pub fn svg_for_scheme(svg: &str, dark: bool) -> Option<String> {
    const KEY: &str = "prefers-color-scheme";
    const ALWAYS: &str = "(min-width:0px)";
    const NEVER: &str = "(min-width:99999px)";
    let lower = svg.to_ascii_lowercase();
    if !lower.contains(KEY) {
        return None;
    }
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(KEY) {
        let key_at = at + found;
        // The parenthesised feature around the key: "(prefers-color-scheme: dark)".
        let open = lower[at..key_at].rfind('(').map(|i| at + i);
        let close = lower[key_at..].find(')').map(|i| key_at + i);
        let (Some(open), Some(close)) = (open, close) else {
            out.push_str(&svg[at..key_at + KEY.len()]);
            at = key_at + KEY.len();
            continue;
        };
        let value: String =
            lower[key_at + KEY.len()..close].chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
        let matches = match value.as_str() {
            "dark" => dark,
            // "light", "no-preference" or anything else: the light look.
            _ => !dark,
        };
        out.push_str(&svg[at..open]);
        out.push_str(if matches { ALWAYS } else { NEVER });
        at = close + 1;
    }
    out.push_str(&svg[at..]);
    Some(out)
}

/// `svg` as a `data:` URL Chromium can download as an image.
pub fn svg_data_url(svg: &str) -> String {
    let mut url = String::from("data:image/svg+xml;charset=utf-8,");
    for b in svg.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'=' | b';' | b',')
        {
            url.push(char::from(b));
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(rgb: [u8; 3], alpha: u8, n: usize) -> Vec<u8> {
        (0..n * n).flat_map(|_| [rgb[0], rgb[1], rgb[2], alpha]).collect()
    }

    #[test]
    fn black_glyph_is_lost_on_dark_chrome_only() {
        let black = solid([10, 10, 10], 255, 16);
        assert!(analyse(&black, true).is_lost());
        assert!(!analyse(&black, false).is_lost());
        let white = solid([250, 250, 250], 255, 16);
        assert!(analyse(&white, false).is_lost());
        assert!(!analyse(&white, true).is_lost());
    }

    #[test]
    fn colourful_and_empty_icons_are_left_alone() {
        let blue = solid([30, 90, 255], 255, 16);
        assert!(!analyse(&blue, true).is_lost());
        assert!(adapt(&blue, 16, 16, true, FaviconContrast::Invert).is_none());
        let empty = solid([0, 0, 0], 0, 16);
        assert!(!analyse(&empty, true).is_lost());
        assert!(adapt(&empty, 16, 16, true, FaviconContrast::Backing).is_none());
    }

    #[test]
    fn off_changes_nothing() {
        assert!(adapt(&solid([0, 0, 0], 255, 16), 16, 16, true, FaviconContrast::Off).is_none());
    }

    #[test]
    fn mono_glyph_is_inverted_and_keeps_its_alpha() {
        let (w, h, out) =
            adapt(&solid([10, 10, 10], 200, 16), 16, 16, true, FaviconContrast::Invert).unwrap();
        assert_eq!((w, h), (16, 16));
        assert_eq!(&out[..4], &[245, 245, 245, 200]);
    }

    #[test]
    fn backing_puts_the_glyph_on_an_opposite_tile() {
        let (w, h, out) =
            adapt(&solid([10, 10, 10], 255, 16), 16, 16, true, FaviconContrast::Backing).unwrap();
        assert_eq!((w, h), (TILE_MIN_SIDE, TILE_MIN_SIDE));
        // The corner outside the rounded tile is clear.
        assert_eq!(out[3], 0);
        // Between the edge and the glyph: the light tile; in the middle: the dark glyph.
        let rim = ((3 * w + 24) * 4) as usize;
        assert!(out[rim] > 200 && out[rim + 3] == 255, "{:?}", &out[rim..rim + 4]);
        let mid = ((24 * w + 24) * 4) as usize;
        assert!(out[mid] < 30, "the glyph stays dark on the tile");
        // In light chrome the tile is dark.
        let (_, _, light) =
            adapt(&solid([250, 250, 250], 255, 16), 16, 16, false, FaviconContrast::Backing).unwrap();
        assert!(light[rim] < 80);
    }

    #[test]
    fn coloured_lost_icon_gets_a_tile_even_in_invert_mode() {
        // Dark navy: nearly black, but not a grey.
        let navy = solid([5, 10, 60], 255, 16);
        let look = analyse(&navy, true);
        assert!(look.is_lost() && !look.is_mono());
        let (w, _, _) = adapt(&navy, 16, 16, true, FaviconContrast::Invert).unwrap();
        assert_eq!(w, TILE_MIN_SIDE);
    }

    #[test]
    fn svg_media_queries_are_decided() {
        let svg = "<svg><style>path{fill:#000}@media (prefers-color-scheme: dark){path{fill:#fff}}\
                   @media screen and (PREFERS-COLOR-SCHEME:light){path{stroke:red}}</style></svg>";
        let dark = svg_for_scheme(svg, true).unwrap();
        assert!(dark.contains("@media (min-width:0px){path{fill:#fff}}"), "{dark}");
        assert!(dark.contains("@media screen and (min-width:99999px){path{stroke:red}}"), "{dark}");
        let light = svg_for_scheme(svg, false).unwrap();
        assert!(light.contains("@media (min-width:99999px){path{fill:#fff}}"), "{light}");
        assert!(light.contains("screen and (min-width:0px)"), "{light}");
        assert!(svg_for_scheme("<svg><path fill=\"#000\"/></svg>", true).is_none());
    }

    #[test]
    fn svg_data_urls_are_escaped() {
        let url = svg_data_url("<svg a=\"b\">#</svg>");
        assert!(url.starts_with("data:image/svg+xml;charset=utf-8,"));
        assert!(url.contains("%3Csvg%20a=%22b%22%3E%23%3C/svg%3E"), "{url}");
    }

    #[test]
    fn cached_png_is_adapted_for_the_start_page() {
        let mut png = Vec::new();
        image::RgbaImage::from_raw(16, 16, solid([10, 10, 10], 255, 16))
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = adapt_png(&png, true, FaviconContrast::Invert).unwrap();
        let pixel = image::load_from_memory(&out).unwrap().into_rgba8().get_pixel(0, 0).0;
        assert_eq!(pixel, [245, 245, 245, 255]);
        assert!(adapt_png(&png, false, FaviconContrast::Invert).is_none());
        assert!(adapt_png(&png, true, FaviconContrast::Off).is_none());
    }
}
