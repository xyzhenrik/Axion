//! The drag image of a tab dragged out of the window: a small rounded card of the page (a
//! snapshot of its top, with the site icon and title below it), or, without a snapshot, just
//! the icon, title and host. Painted on the CPU, because the system draws it (a Wayland drag
//! icon surface, an X11 window), far from our GPU frame.

use axion_compositor::glyphon::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight,
};
use image::RgbaImage;

use crate::platform::dragout::Image;

/// Card geometry and look, in logical px (everything is scaled by [`Spec::scale`]).
const RADIUS: f32 = 12.0;
const BORDER: f32 = 1.0;
/// Height of the preview as a part of the card's width.
pub const PREVIEW_ASPECT: f32 = 0.62;
/// Height of the strip below a preview (icon and title).
const STRIP_HEIGHT: f32 = 32.0;
const STRIP_PADDING: f32 = 10.0;
const STRIP_ICON: f32 = 16.0;
const STRIP_TEXT: f32 = 12.0;
/// The card without a preview: padding, icon side, text sizes.
const BARE_PADDING: f32 = 12.0;
const BARE_ICON: f32 = 28.0;
const BARE_TITLE: f32 = 13.0;
const BARE_HOST: f32 = 11.0;
const GAP: f32 = 8.0;
/// Where the pointer sits on the card, from its top-left corner.
const HOTSPOT: (f32, f32) = (14.0, 12.0);
/// The shortest card width that still lays out.
const MIN_WIDTH: f32 = 96.0;

/// What the card shows.
pub struct Spec<'a> {
    /// The card's width (logical px).
    pub width: f32,
    /// Buffer scale: the card is painted at this many pixels per logical px.
    pub scale: u32,
    /// 0 (invisible) to 1 (solid).
    pub opacity: f32,
    pub dark: bool,
    pub title: &'a str,
    pub host: &'a str,
    /// The page as it looks now (any size; its top-left part is shown).
    pub snapshot: Option<&'a RgbaImage>,
    pub icon: Option<&'a RgbaImage>,
    pub font_family: &'a str,
}

/// Paints the card.
pub fn render(spec: &Spec, fonts: &mut FontSystem) -> Image {
    let s = spec.scale.max(1) as f32;
    let width = spec.width.max(MIN_WIDTH);
    let preview = spec.snapshot.map(|_| width * PREVIEW_ASPECT);
    let height = match preview {
        Some(p) => p + STRIP_HEIGHT,
        None => BARE_ICON + 2.0 * BARE_PADDING,
    };
    let (w, h) = ((width * s).round() as u32, (height * s).round() as u32);
    let (ground, ink, dim, edge) = if spec.dark {
        ([36u8, 36, 40], [245u8, 245, 247], [170u8, 170, 178], [255u8, 255, 255])
    } else {
        ([248u8, 248, 250], [28u8, 28, 30], [110u8, 110, 118], [0u8, 0, 0])
    };
    let mut card = RgbaImage::from_fn(w, h, |_, _| image::Rgba([ground[0], ground[1], ground[2], 255]));
    let mut swash = SwashCache::new();

    match (spec.snapshot, preview) {
        (Some(snapshot), Some(preview)) => {
            let (pw, ph) = (w, (preview * s).round() as u32);
            let shot = cover(snapshot, pw, ph);
            image::imageops::overlay(&mut card, &shot, 0, 0);
            // A hairline between the preview and the strip.
            let line = (s.round() as u32).max(1);
            for y in ph..(ph + line).min(h) {
                for x in 0..w {
                    blend(&mut card, x as i32, y as i32, edge, 0.12);
                }
            }
            let pad = STRIP_PADDING * s;
            let icon_side = (STRIP_ICON * s).round() as u32;
            let mid = ph as f32 + (h - ph) as f32 / 2.0;
            let mut text_x = pad;
            if let Some(icon) = spec.icon {
                let icon = fit(icon, icon_side);
                let (ix, iy) = (pad as i64, (mid - icon_side as f32 / 2.0).round() as i64);
                image::imageops::overlay(&mut card, &icon, ix, iy);
                text_x += icon_side as f32 + GAP * s * 0.75;
            }
            let title = if spec.title.is_empty() { spec.host } else { spec.title };
            let max_w = w as f32 - text_x - pad;
            text(
                &mut card,
                fonts,
                &mut swash,
                spec.font_family,
                title,
                (STRIP_TEXT * s, 500),
                max_w,
                (text_x, mid),
                ink,
            );
        }
        _ => {
            let pad = BARE_PADDING * s;
            let icon_side = (BARE_ICON * s).round() as u32;
            let mut text_x = pad;
            if let Some(icon) = spec.icon {
                let icon = fit(icon, icon_side);
                image::imageops::overlay(&mut card, &icon, pad as i64, pad as i64);
                text_x += icon_side as f32 + GAP * s;
            }
            let max_w = w as f32 - text_x - pad;
            let title = if spec.title.is_empty() { spec.host } else { spec.title };
            let (t, hs) = (BARE_TITLE * s, BARE_HOST * s);
            let block = t * 1.25 + hs * 1.25 + 2.0 * s;
            let top = (h as f32 - block) / 2.0;
            text(
                &mut card,
                fonts,
                &mut swash,
                spec.font_family,
                title,
                (t, 600),
                max_w,
                (text_x, top + t * 0.625),
                ink,
            );
            if !spec.host.is_empty() && spec.host != title {
                let y = top + t * 1.25 + 2.0 * s + hs * 0.625;
                text(
                    &mut card,
                    fonts,
                    &mut swash,
                    spec.font_family,
                    spec.host,
                    (hs, 400),
                    max_w,
                    (text_x, y),
                    dim,
                );
            }
        }
    }

    finish(&card, s, spec.opacity.clamp(0.0, 1.0), edge)
}

/// Scales `src` to cover `w` x `h`, keeping the top-left part (a page's top is what identifies it).
fn cover(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 || w == 0 || h == 0 {
        return RgbaImage::new(w.max(1), h.max(1));
    }
    let k = (w as f32 / sw as f32).max(h as f32 / sh as f32);
    let (cw, ch) =
        (((w as f32 / k).round() as u32).clamp(1, sw), ((h as f32 / k).round() as u32).clamp(1, sh));
    let cropped = image::imageops::crop_imm(src, 0, 0, cw, ch).to_image();
    image::imageops::resize(&cropped, w, h, image::imageops::FilterType::Triangle)
}

/// Scales an icon into a `side` x `side` square, keeping its proportions.
fn fit(icon: &RgbaImage, side: u32) -> RgbaImage {
    let (w, h) = icon.dimensions();
    let side = side.max(1);
    if w == 0 || h == 0 {
        return RgbaImage::new(side, side);
    }
    let k = side as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let small = image::imageops::resize(icon, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut out = RgbaImage::new(side, side);
    image::imageops::overlay(&mut out, &small, ((side - nw) / 2) as i64, ((side - nh) / 2) as i64);
    out
}

/// Source-over of `rgb` with `alpha` at a pixel of an opaque card.
fn blend(card: &mut RgbaImage, x: i32, y: i32, rgb: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x as u32 >= card.width() || y as u32 >= card.height() {
        return;
    }
    let px = card.get_pixel_mut(x as u32, y as u32);
    for (c, &s) in px.0.iter_mut().zip(rgb.iter()) {
        *c = (f32::from(*c) * (1.0 - alpha) + f32::from(s) * alpha).round() as u8;
    }
}

/// One line of `text` (`size` px, `weight`), cut with an ellipsis to `max_w`, vertically
/// centred on `at.1` and starting at `at.0`.
#[allow(clippy::too_many_arguments)]
fn text(
    card: &mut RgbaImage,
    fonts: &mut FontSystem,
    swash: &mut SwashCache,
    family: &str,
    text: &str,
    (size, weight): (f32, u16),
    max_w: f32,
    at: (f32, f32),
    rgb: [u8; 3],
) {
    let line = (size * 1.25).round();
    let mut buffer = Buffer::new(fonts, Metrics::new(size, line));
    buffer.set_wrap(axion_compositor::glyphon::Wrap::None);
    let attrs = Attrs::new().family(Family::Name(family)).weight(Weight(weight));
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut shown = text.clone();
    loop {
        buffer.set_text(&shown, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(fonts, false);
        let width = buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        if width <= max_w || shown.is_empty() {
            break;
        }
        // Cut from the end, letter by letter, until the ellipsis fits.
        let mut chars: Vec<char> = text.chars().collect();
        let keep = shown.trim_end_matches('…').chars().count().saturating_sub(1);
        chars.truncate(keep);
        shown = format!("{}…", chars.into_iter().collect::<String>().trim_end());
        if keep == 0 {
            break;
        }
    }
    let (ox, oy) = (at.0.round() as i32, (at.1 - line / 2.0).round() as i32);
    buffer.draw(fonts, swash, Color::rgb(rgb[0], rgb[1], rgb[2]), |x, y, w, h, color| {
        let alpha = f32::from(color.a()) / 255.0;
        if alpha <= 0.0 {
            return;
        }
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                blend(card, ox + x + dx, oy + y + dy, rgb, alpha);
            }
        }
    });
}

/// Rounds the corners, draws the hairline border, applies `opacity` and converts to premultiplied
/// B, G, R, A.
fn finish(card: &RgbaImage, s: f32, opacity: f32, edge: [u8; 3]) -> Image {
    let (w, h) = card.dimensions();
    let (radius, border) = (RADIUS * s, BORDER * s);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            // Signed distance to the rounded rectangle (negative inside), at the pixel's centre.
            let (px, py) = ((x as f32 + 0.5 - cx).abs(), (y as f32 + 0.5 - cy).abs());
            let (qx, qy) = (px - (cx - radius), py - (cy - radius));
            let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            let coverage = (0.5 - dist).clamp(0.0, 1.0);
            let ring = (1.0 - (-dist - border * 0.5).abs() / (border * 0.5 + 0.5)).clamp(0.0, 1.0);
            let p = card.get_pixel(x, y).0;
            let a = coverage * opacity;
            let mix = ring * 0.18;
            let channel = |c: u8, e: u8| (f32::from(c) * (1.0 - mix) + f32::from(e) * mix) * a;
            bgra.extend_from_slice(&[
                channel(p[2], edge[2]).round() as u8,
                channel(p[1], edge[1]).round() as u8,
                channel(p[0], edge[0]).round() as u8,
                (a * 255.0).round() as u8,
            ]);
        }
    }
    Image {
        width: w,
        height: h,
        scale: s as i32,
        bgra,
        hotspot: ((HOTSPOT.0 * s).round() as i32, (HOTSPOT.1 * s).round() as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(snapshot: Option<&'a RgbaImage>, icon: Option<&'a RgbaImage>) -> Spec<'a> {
        Spec {
            width: 200.0,
            scale: 2,
            opacity: 0.9,
            dark: false,
            title: "A long page title that cannot possibly fit on a single short line of the card",
            host: "example.com",
            snapshot,
            icon,
            font_family: "sans-serif",
        }
    }

    #[test]
    fn card_with_a_snapshot_is_rounded_and_premultiplied() {
        let shot = RgbaImage::from_pixel(800, 600, image::Rgba([200, 30, 30, 255]));
        let icon = RgbaImage::from_pixel(32, 32, image::Rgba([0, 0, 255, 255]));
        let img = render(&spec(Some(&shot), Some(&icon)), &mut FontSystem::new());
        assert_eq!((img.width, img.height), (400, ((200.0 * PREVIEW_ASPECT + STRIP_HEIGHT) * 2.0) as u32));
        assert_eq!(img.bgra.len(), (img.width * img.height * 4) as usize);
        // The corner is transparent, the preview's middle is the snapshot at the card's opacity.
        assert_eq!(img.bgra[3], 0);
        let mid = ((60 * img.width + 200) * 4) as usize;
        assert_eq!(img.bgra[mid + 3], (0.9f32 * 255.0).round() as u8);
        assert!(img.bgra[mid + 2] > img.bgra[mid], "red channel leads: {:?}", &img.bgra[mid..mid + 4]);
        // Premultiplied: no colour above its alpha.
        assert!(img.bgra.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }

    #[test]
    fn card_without_a_snapshot_is_a_bare_title_card() {
        let img = render(&spec(None, None), &mut FontSystem::new());
        assert_eq!(img.height, ((BARE_ICON + 2.0 * BARE_PADDING) * 2.0) as u32);
        assert!(img.hotspot.0 > 0 && img.hotspot.1 > 0);
    }
}
