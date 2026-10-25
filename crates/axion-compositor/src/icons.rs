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
}

/// Packs icon and weight into glyphon's custom glyph id.
pub(crate) fn glyph_id(icon: Icon, weight: IconWeight) -> u16 {
    let i = Icon::ALL.iter().position(|&x| x == icon).unwrap_or(0);
    let w = IconWeight::ALL.iter().position(|&x| x == weight).unwrap_or(0);
    (i * IconWeight::ALL.len() + w) as u16
}

fn from_glyph_id(id: u16) -> Option<(Icon, IconWeight)> {
    let id = id as usize;
    let icon = *Icon::ALL.get(id / IconWeight::ALL.len())?;
    Some((icon, IconWeight::ALL[id % IconWeight::ALL.len()]))
}

/// glyphon's rasterizer callback: strokes the icon into an alpha mask.
pub(crate) fn rasterize(request: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
    let (icon, weight) = from_glyph_id(request.id)?;
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
        width: weight.stroke() * scale as f32,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
    let data = pixmap.pixels().iter().map(|px| px.alpha()).collect();
    Some(RasterizedCustomGlyph { data, content_type: ContentType::Mask })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterizes() {
        for &icon in Icon::ALL {
            for weight in IconWeight::ALL {
                let id = glyph_id(icon, weight);
                assert_eq!(from_glyph_id(id), Some((icon, weight)));
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
}
