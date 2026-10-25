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

/// Tint strength of every glass level under Reduce Transparency (nearly opaque).
const REDUCED_TRANSPARENCY_OPACITY: f32 = 0.94;

fn pick_level(l: axion_config::Levels, level: GlassLevel) -> f32 {
    match level {
        GlassLevel::Thin | GlassLevel::Lens => l.thin,
        GlassLevel::Regular => l.regular,
        GlassLevel::Thick => l.thick,
    }
}

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

    fn material(&self) -> &GlassMaterial {
        if self.dark { &self.config.glass.dark } else { &self.config.glass.light }
    }

    /// A held knob: clear (no blur or tint), magnifying across its whole height, with a bright
    /// rim and a soft shadow beneath (Liquid Glass switch, iPadOS 26).
    fn lens(&self, scale: f32) -> GlassParams {
        let m = self.material();
        let glass = &self.config.glass;
        let bezel = LENS_BEZEL * scale;
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
            tint: [1.0, 1.0, 1.0, if glass.reduce_transparency { 0.9 } else { LENS_TINT }],
            rim: m.rim * LENS_RIM,
            shadow: m.shadow * LENS_SHADOW,
            light_dir: Vec2::new(-0.55, -0.85),
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
