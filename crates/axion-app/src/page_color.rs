//! One stable colour per page for the window around it (`appearance.tint_from_page`, Safari's
//! "colour in tab bar") and for flipping the chrome light or dark (`appearance.adapt_to_page`).
//!
//! The colour is the page's `theme-color` (the one for the current colour scheme) if it has
//! one; otherwise the colour most of the page shows, from a histogram over a small grid of the
//! frame measured on the GPU once the page has loaded. Either way it is evened out into a calm
//! light or dark tone so the chrome on it stays legible. It is worked out again only when the
//! page loads, its theme colours change or the colour scheme switches — never while scrolling —
//! and the window colour follows changes on a spring.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axion_engine::{ThemeColor, ViewId};
use axion_physics::{Animated, Spring};

/// Bits kept per channel for the histogram (16 levels: near shades count as one colour).
const HISTOGRAM_BITS: u32 = 4;
/// The top quarter of the page (next to the bar) counts this much more.
const TOP_WEIGHT: f32 = 2.0;
/// A light colour is at least this light, a dark one at most this light (HSL lightness).
const LIGHT_MIN: f32 = 0.86;
const DARK_MAX: f32 = 0.20;
/// The colour's saturation is capped here, so the window never turns garish.
const MAX_SATURATION: f32 = 0.55;

#[derive(Default)]
struct PageColor {
    /// The page's theme colours (all of them, with their media queries).
    theme: Vec<ThemeColor>,
    /// The measured dominant colour.
    measured: Option<[u8; 3]>,
    /// When to measure the page's frame.
    measure_at: Option<Instant>,
}

/// The page colours of all tabs, and the window tint moving between them.
pub struct PageTint {
    pages: HashMap<ViewId, PageColor>,
    /// The tab whose frame is being measured.
    measuring: Option<ViewId>,
    dark: bool,
    tint: Option<[Animated<f32>; 3]>,
}

impl PageTint {
    pub fn new(dark: bool) -> Self {
        Self { pages: HashMap::new(), measuring: None, dark, tint: None }
    }

    /// A new page started loading in `view`: its theme colours are the old page's no more.
    pub fn page_started(&mut self, view: ViewId) {
        let page = self.pages.entry(view).or_default();
        page.theme.clear();
        page.measure_at = None;
    }

    /// The page finished loading: measure it once `delay` has passed (late styles are in).
    pub fn page_loaded(&mut self, view: ViewId, delay: Duration) {
        self.pages.entry(view).or_default().measure_at = Some(Instant::now() + delay);
    }

    /// The page reported its theme colours.
    /// The page reported its theme colours. When they change on a page that is already shown
    /// (a script restyled it), it is measured again after `delay`.
    pub fn theme_colors(&mut self, view: ViewId, colors: &[ThemeColor], delay: Duration) {
        let page = self.pages.entry(view).or_default();
        if page.theme != colors {
            if page.measured.is_some() {
                page.measure_at = Some(Instant::now() + delay);
            }
            page.theme = colors.to_vec();
        }
    }

    /// The colour scheme switched: pages may look different now, measure the shown one again.
    pub fn set_dark(&mut self, dark: bool, shown: Option<ViewId>, delay: Duration) {
        if dark == self.dark {
            return;
        }
        self.dark = dark;
        for page in self.pages.values_mut() {
            page.measured = None;
        }
        if let Some(view) = shown {
            self.pages.entry(view).or_default().measure_at = Some(Instant::now() + delay);
        }
    }

    /// `view` was shown: if it has no colour yet, measure it soon.
    pub fn shown(&mut self, view: ViewId, loading: bool, delay: Duration) {
        let page = self.pages.entry(view).or_default();
        if !loading && page.measured.is_none() && page.measure_at.is_none() {
            page.measure_at = Some(Instant::now() + delay);
        }
    }

    pub fn closed(&mut self, view: ViewId) {
        self.pages.remove(&view);
    }

    /// Whether `view`'s frame should be measured now (then call [`PageTint::measuring`]).
    pub fn wants_measure(&self, view: ViewId) -> bool {
        self.measuring.is_none()
            && self.pages.get(&view).and_then(|p| p.measure_at).is_some_and(|at| Instant::now() >= at)
    }

    /// A measurement of `view`'s frame started.
    pub fn measuring(&mut self, view: ViewId) {
        self.measuring = Some(view);
        if let Some(page) = self.pages.get_mut(&view) {
            page.measure_at = None;
        }
    }

    /// The measurement finished: the frame's colours, row by row (top first).
    pub fn measured(&mut self, cells: &[[u8; 3]]) {
        let Some(view) = self.measuring.take() else { return };
        if let Some(color) = dominant_color(cells) {
            self.pages.entry(view).or_default().measured = Some(color);
        }
    }

    /// The colour of `view`'s page (sRGB 0–1), evened out for legibility.
    pub fn color(&self, view: ViewId) -> Option<[f32; 3]> {
        let page = self.pages.get(&view)?;
        let raw = theme_color(&page.theme, self.dark).or(page.measured)?;
        Some(in_scheme(raw, self.dark))
    }

    /// Moves the window tint towards `target` on `spring` (`None`: no tint; it snaps when it
    /// first appears). Returns the colour to show.
    pub fn step(&mut self, target: Option<[f32; 3]>, spring: Spring, dt: f32) -> Option<[f32; 4]> {
        let Some(target) = target else {
            self.tint = None;
            return None;
        };
        let tint = self.tint.get_or_insert_with(|| target.map(|c| Animated::new(c, spring)));
        for (channel, value) in tint.iter_mut().zip(target) {
            channel.spring = spring;
            channel.target = value;
            channel.step(dt);
        }
        Some([tint[0].value, tint[1].value, tint[2].value, 1.0])
    }
}

/// The theme colour for the scheme: one whose media query names it, else one without a query.
fn theme_color(colors: &[ThemeColor], dark: bool) -> Option<[u8; 3]> {
    let scheme = |media: &str| {
        let m: String = media.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        if m.contains("prefers-color-scheme:dark") {
            Some(true)
        } else if m.contains("prefers-color-scheme:light") {
            Some(false)
        } else {
            None
        }
    };
    colors
        .iter()
        .find(|c| scheme(&c.media) == Some(dark))
        .or_else(|| colors.iter().find(|c| scheme(&c.media).is_none()))
        .map(|c| c.rgb)
}

/// The colour most of the frame shows: the fullest bin of a coarse colour histogram (the
/// page's top quarter weighs more), as the average of the colours in it.
pub fn dominant_color(cells: &[[u8; 3]]) -> Option<[u8; 3]> {
    if cells.is_empty() {
        return None;
    }
    let side = (cells.len() as f32).sqrt().max(1.0) as usize;
    let shift = 8 - HISTOGRAM_BITS;
    let mut bins: HashMap<u32, (f32, [f32; 3])> = HashMap::new();
    for (i, c) in cells.iter().enumerate() {
        let weight = if i / side < side / 4 { TOP_WEIGHT } else { 1.0 };
        let key = (u32::from(c[0]) >> shift) << (2 * HISTOGRAM_BITS)
            | (u32::from(c[1]) >> shift) << HISTOGRAM_BITS
            | u32::from(c[2]) >> shift;
        let bin = bins.entry(key).or_insert((0.0, [0.0; 3]));
        bin.0 += weight;
        for (sum, channel) in bin.1.iter_mut().zip(c) {
            *sum += weight * f32::from(*channel);
        }
    }
    let (_, (weight, sum)) = bins.into_iter().max_by(|a, b| a.1.0.total_cmp(&b.1.0).then(b.0.cmp(&a.0)))?;
    Some(sum.map(|s| (s / weight).round().clamp(0.0, 255.0) as u8))
}

/// `rgb` as a calm tone of the current scheme: the page's hue, light in light mode and dark
/// in dark mode, so switching the mode switches the tint with it.
pub fn in_scheme(rgb: [u8; 3], dark: bool) -> [f32; 3] {
    let color = rgb.map(|c| f32::from(c) / 255.0);
    let (h, s, l) = to_hsl(color);
    let l = if dark { l.min(DARK_MAX) } else { l.max(LIGHT_MIN) };
    from_hsl(h, s.min(MAX_SATURATION), l)
}

/// Rec. 709 luma of an sRGB colour (what the eye judges as light or dark).
pub fn lightness(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON);
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s.min(1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m].map(|v| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prevalent_colour_wins_over_a_brighter_minority() {
        // A dark page (GitHub-like) with some white text and a blue button.
        let mut cells = vec![[13, 17, 23]; 32 * 32];
        for c in cells.iter_mut().skip(300).step_by(5) {
            *c = [255, 255, 255];
        }
        cells[900] = [46, 107, 255];
        let color = dominant_color(&cells).unwrap();
        assert!(color.iter().zip([13, 17, 23]).all(|(a, b)| a.abs_diff(b) <= 2), "{color:?}");
        assert_eq!(dominant_color(&[]), None);
    }

    #[test]
    fn colours_become_calm_tones_of_the_scheme() {
        let white = in_scheme([255, 255, 255], false);
        assert!(white.iter().all(|c| (c - 1.0).abs() < 1e-3));
        let mid_blue = in_scheme([60, 110, 200], true);
        assert!(lightness(mid_blue) < 0.25, "{mid_blue:?}");
        let pale = in_scheme([200, 210, 230], false);
        assert!(lightness(pale) > 0.8, "{pale:?}");
        let neon = in_scheme([255, 0, 255], false);
        let (_, s, _) = to_hsl(neon);
        assert!(s <= MAX_SATURATION + 1e-3);
    }

    #[test]
    fn the_tint_follows_the_mode() {
        let blue = [99, 124, 249];
        assert!(lightness(in_scheme(blue, true)) < 0.25);
        assert!(lightness(in_scheme(blue, false)) > 0.8);
    }

    #[test]
    fn theme_colour_for_the_scheme() {
        let colors = [
            ThemeColor { rgb: [1, 1, 1], media: "(prefers-color-scheme: dark)".into() },
            ThemeColor { rgb: [250, 250, 250], media: String::new() },
        ];
        assert_eq!(theme_color(&colors, true), Some([1, 1, 1]));
        assert_eq!(theme_color(&colors, false), Some([250, 250, 250]));
        assert_eq!(theme_color(&[], false), None);
    }

    #[test]
    fn measured_once_per_load_not_per_frame() {
        let view = ViewId(1);
        let mut tint = PageTint::new(false);
        assert!(!tint.wants_measure(view));
        tint.page_loaded(view, Duration::ZERO);
        assert!(tint.wants_measure(view));
        tint.measuring(view);
        assert!(!tint.wants_measure(view), "no second measurement while one runs");
        tint.measured(&[[255, 255, 255]; 4]);
        assert!(!tint.wants_measure(view), "and none after it: scrolling changes nothing");
        let measured = tint.color(view).unwrap();
        // A theme colour takes precedence (its hue, in the light scheme's tone).
        tint.theme_colors(view, &[ThemeColor { rgb: [200, 30, 30], media: String::new() }], Duration::ZERO);
        let themed = tint.color(view).unwrap();
        assert!(themed[0] > themed[2] && themed != measured, "{themed:?}");
        // The scheme switch measures again.
        tint.set_dark(true, Some(view), Duration::ZERO);
        assert!(tint.wants_measure(view));
    }

    #[test]
    fn the_tint_springs_to_a_new_colour() {
        let mut tint = PageTint::new(false);
        let spring = Spring::new(0.4, 1.0);
        assert_eq!(tint.step(Some([1.0; 3]), spring, 0.016), Some([1.0, 1.0, 1.0, 1.0]));
        let mid = tint.step(Some([0.0; 3]), spring, 0.05).unwrap();
        assert!(mid[0] > 0.0 && mid[0] < 1.0, "{mid:?}");
        let mut last = mid;
        for _ in 0..200 {
            last = tint.step(Some([0.0; 3]), spring, 0.016).unwrap();
        }
        assert!(last[0].abs() < 1e-3, "{last:?}");
    }
}
