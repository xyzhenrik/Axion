//! Resolves the configuration into concrete paint for one colour scheme.

use axion_config::{Color, Config, GlassMaterial};
use axion_glass::GlassParams;
use glam::Vec2;

/// How much a glass surface separates from what's behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GlassLevel {
    Thin,
    Regular,
    Thick,
    /// The clear, strongly refracting lens of a held switch or slider knob.
    Lens,
}

/// Semantic colours, resolved per scheme so a theme switch needs no rebuild.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Label,
    Secondary,
    Accent,
    Shield,
    Surface,
    Ground,
    Hairline,
    /// A fixed colour (e.g. a tab group's dot).
    Fixed(Color),
}

/// Text styles from the design's type scale (size / line height / weight).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextStyle {
    LargeTitle,
    Title1,
    Title2,
    /// 15/22 regular — address field, sidebar rows.
    Body,
    /// 15/22 medium — the address field's host name.
    BodyEmphasis,
    /// 14/20 medium — tab pills.
    Callout,
    /// 13/18 — secondary lines.
    Footnote,
    /// 11/14 semibold, upper case — section headers.
    Caption,
}

impl TextStyle {
    /// Size, line height and weight in logical pixels.
    pub fn metrics(self) -> (f32, f32, u16) {
        match self {
            TextStyle::LargeTitle => (34.0, 40.0, 700),
            TextStyle::Title1 => (26.0, 32.0, 650),
            TextStyle::Title2 => (20.0, 26.0, 600),
            TextStyle::Body => (15.0, 22.0, 400),
            TextStyle::BodyEmphasis => (15.0, 22.0, 500),
            TextStyle::Callout => (14.0, 20.0, 500),
            TextStyle::Footnote => (13.0, 18.0, 400),
            TextStyle::Caption => (11.0, 14.0, 600),
        }
    }
}

/// The lens (see `Theme::lens`): bezel width (logical px) and multipliers on the material.
const LENS_BEZEL: f32 = 16.0;
const LENS_REFRACTION: f32 = 1.6;
const LENS_DISPERSION: f32 = 1.5;
const LENS_TINT: f32 = 0.03;
const LENS_RIM: f32 = 1.5;
const LENS_SHADOW: f32 = 0.45;
/// Reduce Transparency: how far the opaque lens leans from the glass tint towards the label
/// colour (lighter in dark, darker in light), so text on it keeps its contrast.
const LENS_OPAQUE_MIX: f32 = 0.14;

/// Flat mode: how opaque a card of each level is (a hint of the page shows through the thin ones,
/// never blurred) and its shadow (logical px: offset, blur; strength in light / dark chrome).
const FLAT_OPACITY: [f32; 3] = [0.94, 0.97, 1.0];
const FLAT_SHADOW: [(f32, f32, [f32; 2]); 3] =
    [(1.0, 5.0, [0.08, 0.20]), (2.0, 14.0, [0.12, 0.34]), (4.0, 28.0, [0.16, 0.46])];
/// Width of a flat card's border (logical px).
pub const FLAT_BORDER: f32 = 1.0;
/// A tinted (primary) button in flat mode: the colour, almost solid.
const FLAT_TINTED_OPACITY: f32 = 0.96;
/// A lens-level background (a hovered sidebar row's highlight) in flat mode: the label colour at
/// this strength, over whatever it lies on.
const FLAT_HIGHLIGHT_ALPHA: f32 = 0.08;

/// A flat card: what `Background::Glass(level)` is painted as without glass (see
/// [`Theme::flat_paint`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatPaint {
    pub fill: [f32; 4],
    /// The 1px outline; fully transparent for none.
    pub border: [f32; 4],
    /// Shadow colour (black with its strength as alpha), offset and blur in logical px.
    /// Nested cards have none.
    pub shadow: [f32; 4],
    pub shadow_dy: f32,
    pub shadow_blur: f32,
}

/// Tint strength of every glass level under Reduce Transparency (nearly opaque).
const REDUCED_TRANSPARENCY_OPACITY: f32 = 0.94;

fn pick_level(l: axion_config::Levels, level: GlassLevel) -> f32 {
    match level {
        GlassLevel::Thin | GlassLevel::Lens => l.thin,
        GlassLevel::Regular => l.regular,
        GlassLevel::Thick => l.thick,
    }
}

/// Limits of the icon stroke multiplier.
const MIN_ICON_STROKE: f32 = 0.5;
const MAX_ICON_STROKE: f32 = 2.0;

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    config: Config,
}

impl Theme {
    pub fn new(config: &Config, dark: bool) -> Self {
        Self { dark, config: config.clone() }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn color(&self, paint: Paint) -> [f32; 4] {
        let palette = if self.dark { &self.config.colors.dark } else { &self.config.colors.light };
        match paint {
            Paint::Label => palette.label,
            Paint::Secondary => palette.secondary,
            Paint::Accent => self.config.appearance.accent,
            Paint::Shield => self.config.colors.shield.unwrap_or(self.config.appearance.accent),
            Paint::Surface => palette.surface,
            Paint::Ground => palette.ground,
            Paint::Hairline => palette.hairline,
            Paint::Fixed(c) => c,
        }
        .to_f32()
    }

    pub fn font_family(&self) -> &str {
        &self.config.appearance.font_family
    }

    pub fn text_scale(&self) -> f32 {
        self.config.appearance.text_scale
    }

    /// Multiplier on icon strokes for this scheme (`appearance.icon_stroke_light` / `_dark`).
    pub fn icon_stroke(&self) -> f32 {
        let a = &self.config.appearance;
        let v = if self.dark { a.icon_stroke_dark } else { a.icon_stroke_light };
        if v.is_finite() { v.clamp(MIN_ICON_STROKE, MAX_ICON_STROKE) } else { 1.0 }
    }

    fn material(&self) -> &GlassMaterial {
        if self.dark { &self.config.glass.dark } else { &self.config.glass.light }
    }

    /// A held knob: clear (no blur or tint), magnifying across its whole height, with a bright
    /// rim and a soft shadow beneath (Liquid Glass switch, iPadOS 26).
    fn lens(&self, scale: f32) -> GlassParams {
        let m = self.material();
        let glass = &self.config.glass;
        let bezel = LENS_BEZEL * scale;
        let tint = if glass.reduce_transparency {
            let rgb = self.reduced_lens_color();
            [rgb[0], rgb[1], rgb[2], 1.0]
        } else {
            [1.0, 1.0, 1.0, LENS_TINT]
        };
        GlassParams {
            refraction: if glass.reduce_transparency {
                0.0
            } else {
                m.refraction.max(0.5) * bezel * LENS_REFRACTION
            },
            bezel_width: bezel,
            dispersion: m.dispersion * LENS_DISPERSION,
            merge_smoothness: 0.0,
            frost: 0.0,
            blur_radius: 0.0,
            tint,
            rim: m.rim * LENS_RIM,
            shadow: m.shadow * LENS_SHADOW,
            light_dir: Vec2::new(-0.55, -0.85),
        }
    }

    /// The opaque colour of a lens (hover highlight, held knob or pill) under Reduce Transparency:
    /// the glass tint nudged towards the label colour, so it stands out from the surface it
    /// lies on yet keeps label text and icons legible in both themes (never plain white).
    pub fn reduced_lens_color(&self) -> [f32; 3] {
        let base = self.material().tint.to_f32();
        let label = self.color(Paint::Label);
        [0, 1, 2].map(|i| base[i] * (1.0 - LENS_OPAQUE_MIX) + label[i] * LENS_OPAQUE_MIX)
    }

    /// Flat mode (`glass.style = "flat"`): nothing is drawn as glass, merged or animated like
    /// glass. This is the one switch the painter, the popover code and the physics read.
    pub fn flat(&self) -> bool {
        self.config.glass.is_flat()
    }

    /// What `Background::Glass(level)` is painted as in flat mode: a solid themed card (the
    /// scheme's surface colour; a hint translucent only for thin cards, never blurred) with a
    /// hairline border and a soft shadow. `nested` (glass inside glass: a card on the sidebar,
    /// a field in the bar) is an inset in the ground colour without a shadow, so it stands out
    /// from the card it lies on in both schemes.
    pub fn flat_paint(&self, level: GlassLevel, nested: bool) -> FlatPaint {
        let i = match level {
            GlassLevel::Thin | GlassLevel::Lens => 0,
            GlassLevel::Regular => 1,
            GlassLevel::Thick => 2,
        };
        let hairline = self.color(Paint::Hairline);
        if level == GlassLevel::Lens {
            // A highlight, not a surface: a tint of the label colour that keeps contrast.
            let label = self.color(Paint::Label);
            return FlatPaint {
                fill: [label[0], label[1], label[2], FLAT_HIGHLIGHT_ALPHA],
                border: [0.0; 4],
                shadow: [0.0; 4],
                shadow_dy: 0.0,
                shadow_blur: 0.0,
            };
        }
        if nested {
            let ground = self.color(Paint::Ground);
            return FlatPaint {
                fill: [ground[0], ground[1], ground[2], 1.0],
                border: [hairline[0], hairline[1], hairline[2], hairline[3] * 0.6],
                shadow: [0.0; 4],
                shadow_dy: 0.0,
                shadow_blur: 0.0,
            };
        }
        let surface = self.color(Paint::Surface);
        let (dy, blur, strength) = FLAT_SHADOW[i];
        FlatPaint {
            fill: [surface[0], surface[1], surface[2], FLAT_OPACITY[i]],
            border: hairline,
            shadow: [0.0, 0.0, 0.0, strength[usize::from(self.dark)]],
            shadow_dy: dy,
            shadow_blur: blur,
        }
    }

    /// A pill raised from a tinted strip (the open tab): the surface colour with a hairline, no shadow.
    pub fn flat_raised(&self) -> FlatPaint {
        let surface = self.color(Paint::Surface);
        FlatPaint {
            fill: [surface[0], surface[1], surface[2], 1.0],
            border: self.color(Paint::Hairline),
            shadow: [0.0; 4],
            shadow_dy: 0.0,
            shadow_blur: 0.0,
        }
    }

    /// A tinted card (a primary button) in flat mode: the colour, no outline, no shadow.
    pub fn flat_tinted(&self, tint: [f32; 4]) -> FlatPaint {
        FlatPaint {
            fill: [tint[0], tint[1], tint[2], FLAT_TINTED_OPACITY],
            border: [0.0; 4],
            shadow: [0.0; 4],
            shadow_dy: 0.0,
            shadow_blur: 0.0,
        }
    }

    /// The tint strength of `level` (see [`Theme::glass`]).
    pub fn glass_opacity(&self, level: GlassLevel) -> f32 {
        if self.config.glass.reduce_transparency {
            REDUCED_TRANSPARENCY_OPACITY
        } else {
            pick_level(self.material().opacity, level)
        }
    }

    /// The tint strength of a glass button (see `Glass::button_opacity`).
    pub fn glass_button_opacity(&self) -> f32 {
        if self.config.glass.reduce_transparency {
            REDUCED_TRANSPARENCY_OPACITY
        } else {
            self.config.glass.button_opacity.clamp(0.0, 1.0)
        }
    }

    /// Glass parameters for `level`, in physical pixels at `scale`.
    pub fn glass(&self, level: GlassLevel, scale: f32) -> GlassParams {
        let glass = &self.config.glass;
        let m = self.material();
        if level == GlassLevel::Lens {
            return self.lens(scale);
        }
        let pick = |l: axion_config::Levels| pick_level(l, level);
        // Tint towards the accent a little, as the design asks.
        let base = m.tint.to_f32();
        let accent = self.config.appearance.accent.to_f32();
        let k = self.config.appearance.accent_tint.clamp(0.0, 1.0);
        let tint = [0, 1, 2].map(|i| base[i] * (1.0 - k) + accent[i] * k);
        let opacity = self.glass_opacity(level);
        let bezel = glass.bezel_width * scale;
        GlassParams {
            refraction: if glass.reduce_transparency { 0.0 } else { m.refraction * bezel },
            bezel_width: bezel,
            dispersion: m.dispersion,
            merge_smoothness: glass.merge_distance * scale,
            frost: 1.0,
            blur_radius: pick(m.blur) * scale,
            tint: [tint[0], tint[1], tint[2], opacity],
            rim: m.rim,
            shadow: m.shadow,
            light_dir: Vec2::new(-0.55, -0.85),
        }
    }
}

#[cfg(test)]
mod flat_tests {
    use super::*;

    fn theme(dark: bool, flat: bool) -> Theme {
        let mut config = Config::default();
        if flat {
            config.glass.style = axion_config::GlassStyle::Flat;
        }
        Theme::new(&config, dark)
    }

    #[test]
    fn only_the_flat_style_is_flat() {
        assert!(!theme(false, false).flat());
        assert!(theme(false, true).flat());
    }

    #[test]
    fn glass_levels_map_to_ever_more_solid_cards_with_more_shadow() {
        for dark in [false, true] {
            let t = theme(dark, true);
            let [thin, regular, thick] =
                [GlassLevel::Thin, GlassLevel::Regular, GlassLevel::Thick].map(|l| t.flat_paint(l, false));
            assert!(thin.fill[3] < regular.fill[3] && regular.fill[3] < thick.fill[3]);
            assert_eq!(thick.fill[3], 1.0);
            assert!(thin.shadow[3] < regular.shadow[3] && regular.shadow[3] < thick.shadow[3]);
            assert!(thin.shadow_blur < thick.shadow_blur);
            // The scheme's own surface and hairline.
            assert_eq!(thick.fill[..3], t.color(Paint::Surface)[..3]);
            assert_eq!(thick.border, t.color(Paint::Hairline));
        }
    }

    #[test]
    fn nested_cards_are_shadowless_insets_in_the_ground_colour() {
        for dark in [false, true] {
            let t = theme(dark, true);
            let inset = t.flat_paint(GlassLevel::Thin, true);
            assert_eq!(inset.fill[..3], t.color(Paint::Ground)[..3]);
            assert_eq!(inset.fill[3], 1.0);
            assert_eq!(inset.shadow[3], 0.0);
            assert_ne!(inset.fill[..3], t.flat_paint(GlassLevel::Thin, false).fill[..3]);
        }
    }

    #[test]
    fn highlights_and_tinted_cards_keep_contrast() {
        let t = theme(false, true);
        let lens = t.flat_paint(GlassLevel::Lens, true);
        assert_eq!(lens.fill[..3], t.color(Paint::Label)[..3]);
        assert!(lens.fill[3] < 0.15, "a faint tint of the label colour, text stays readable");
        assert_eq!(lens.border[3], 0.0);
        let tinted = t.flat_tinted([0.2, 0.4, 1.0, 1.0]);
        assert!(tinted.fill[3] > 0.9 && tinted.shadow[3] == 0.0 && tinted.border[3] == 0.0);
    }
}

#[cfg(test)]
mod icon_stroke_tests {
    use super::*;

    #[test]
    fn each_scheme_uses_its_own_icon_stroke() {
        let mut config = Config::default();
        assert_eq!(Theme::new(&config, true).icon_stroke(), 1.0, "dark stays as designed");
        assert_eq!(Theme::new(&config, false).icon_stroke(), 1.2);
        config.appearance.icon_stroke_light = 1.4;
        config.appearance.icon_stroke_dark = 1.15;
        assert_eq!(Theme::new(&config, false).icon_stroke(), 1.4);
        assert_eq!(Theme::new(&config, true).icon_stroke(), 1.15);
        config.appearance.icon_stroke_light = f32::NAN;
        assert_eq!(Theme::new(&config, false).icon_stroke(), 1.0);
        config.appearance.icon_stroke_light = 9.0;
        assert_eq!(Theme::new(&config, false).icon_stroke(), 2.0);
    }
}
