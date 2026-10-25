//! The settings schema. Defaults come from the design system ("Foundations" board).

use serde::{Deserialize, Serialize};

use crate::Color;

/// All of Axion's settings. Every section and key is optional in `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub appearance: Appearance,
    pub glass: Glass,
    pub colors: Colors,
    pub radii: Radii,
    pub motion: Motion,
    pub layout: Layout,
    pub privacy: Privacy,
    pub shields: Shields,
    pub tabs: Tabs,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Page opened in new windows and tabs.
    pub homepage: String,
    /// Search URL; `{}` is replaced by the URL-encoded query.
    pub search_url: String,
}

impl Default for General {
    fn default() -> Self {
        Self { homepage: "https://duckduckgo.com/".into(), search_url: "https://duckduckgo.com/?q={}".into() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    /// Follow the desktop's light/dark preference.
    #[default]
    Auto,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: ThemeMode,
    /// Accent colour for selection, focus rings and primary buttons. Glass tints towards it.
    pub accent: Color,
    /// How strongly glass tints towards the accent (0–1).
    pub accent_tint: f32,
    /// UI font family; falls back to the system sans-serif when missing.
    pub font_family: String,
    /// Scales every UI text size.
    pub text_scale: f32,
    /// Switch the chrome between light and dark to stay legible over the page behind it.
    pub adapt_to_page: bool,
    /// Give the window around the page the page's own background colour (like Safari).
    pub tint_from_page: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Auto,
            accent: Color::rgb(0x2E6BFF),
            accent_tint: 0.06,
            font_family: "Inter".into(),
            text_scale: 1.0,
            adapt_to_page: true,
            tint_from_page: true,
        }
    }
}

/// Opacity or blur for the three glass thicknesses.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Levels {
    /// Floating controls over content (tab bar, media controls).
    pub thin: f32,
    /// Panels (sidebar, popovers).
    pub regular: f32,
    /// Sheets that need the most separation (settings, tab overview chrome).
    pub thick: f32,
}

/// Glass appearance for one colour scheme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlassMaterial {
    pub tint: Color,
    /// Tint opacity per thickness.
    pub opacity: Levels,
    /// Backdrop blur radius in logical pixels per thickness.
    pub blur: Levels,
    /// Refraction strength (0–1) relative to the bezel width.
    pub refraction: f32,
    /// Chromatic dispersion at the rim (0–1).
    pub dispersion: f32,
    /// Rim highlight strength (0–1).
    pub rim: f32,
    /// Drop shadow strength (0–1).
    pub shadow: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Glass {
    pub light: GlassMaterial,
    pub dark: GlassMaterial,
    /// Width of the curved rim in logical pixels.
    pub bezel_width: f32,
    /// Distance in logical pixels at which neighbouring glass starts to flow together.
    pub merge_distance: f32,
    /// Maximum squash along the direction of motion (0.08 = 8%).
    pub squash: f32,
    /// Extra tint opacity over busy or mid-grey backdrops, to keep labels legible.
    pub adaptive_opacity: f32,
    /// Replace glass with opaque surfaces.
    pub reduce_transparency: bool,
    /// Replace springs and squash with short fades.
    pub reduce_motion: bool,
}

impl Default for Glass {
    fn default() -> Self {
        let blur = Levels { thin: 16.0, regular: 26.0, thick: 40.0 };
        Self {
            light: GlassMaterial {
                tint: Color::rgb(0xFFFFFF),
                opacity: Levels { thin: 0.30, regular: 0.48, thick: 0.66 },
                blur,
                refraction: 0.55,
                dispersion: 0.12,
                rim: 0.95,
                shadow: 0.20,
            },
            dark: GlassMaterial {
                tint: Color::rgb(0x16181E),
                opacity: Levels { thin: 0.36, regular: 0.50, thick: 0.68 },
                blur,
                refraction: 0.55,
                dispersion: 0.10,
                rim: 0.32,
                shadow: 0.45,
            },
            bezel_width: 22.0,
            merge_distance: 20.0,
            squash: 0.08,
            adaptive_opacity: 0.12,
            reduce_transparency: false,
            reduce_motion: false,
        }
    }
}

/// Neutral colours for one colour scheme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    /// Behind everything (empty window, start page).
    pub ground: Color,
    /// Opaque surfaces, e.g. the address field inside the tab bar.
    pub surface: Color,
    /// Primary text and icons.
    pub label: Color,
    /// Secondary text.
    pub secondary: Color,
    /// Separators.
    pub hairline: Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub light: Palette,
    pub dark: Palette,
    /// Shields icon when protection is active.
    pub shield: Color,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            light: Palette {
                ground: Color::rgb(0xF3F4F7),
                surface: Color::rgb(0xFFFFFF),
                label: Color::rgb(0x111318),
                secondary: Color::rgb(0x4A5060),
                hairline: Color::rgb(0xD9DCE3),
            },
            dark: Palette {
                ground: Color::rgb(0x0E0F13),
                surface: Color::rgb(0x1A1C22),
                label: Color::rgb(0xF5F6F8),
                secondary: Color::rgb(0xA8AEBB),
                hairline: Color::rgb(0x2C2F38),
            },
            shield: Color::rgb(0x1F9D57),
        }
    }
}

/// Corner radii in logical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Radii {
    /// Settings, tab overview.
    pub sheet: f32,
    /// Sidebar, Picture in Picture window.
    pub panel: f32,
    /// Popovers, tab thumbnails.
    pub card: f32,
    /// Fields, segmented controls.
    pub control: f32,
    /// Badges, favicons.
    pub small: f32,
}

impl Default for Radii {
    fn default() -> Self {
        Self { sheet: 28.0, panel: 24.0, card: 20.0, control: 14.0, small: 8.0 }
    }
}

/// A spring, as in SwiftUI: `response` is the period in seconds, `damping` the damping ratio.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringSpec {
    pub response: f32,
    pub damping: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Motion {
    /// Tabs, toggles, buttons.
    pub snappy: SpringSpec,
    /// Sidebar, sheets, settings.
    pub smooth: SpringSpec,
    /// Glass merge and split, popover drip.
    pub liquid: SpringSpec,
    /// Overview zoom, scroll hand-off.
    pub fling: SpringSpec,
    /// Multiplies every spring response (0.5 = twice as fast).
    pub speed: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            snappy: SpringSpec { response: 0.28, damping: 0.90 },
            smooth: SpringSpec { response: 0.45, damping: 1.00 },
            liquid: SpringSpec { response: 0.50, damping: 0.62 },
            fling: SpringSpec { response: 0.60, damping: 0.85 },
            speed: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarMode {
    /// Docked in wide windows, overlay otherwise.
    #[default]
    Auto,
    Docked,
    Overlay,
}

/// Window chrome geometry in logical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// Gap between window edges, the bar and the sidebar.
    pub margin: f32,
    pub bar_height: f32,
    pub sidebar_width: f32,
    pub sidebar: SidebarMode,
    pub sidebar_visible: bool,
    /// From this window width up, the sidebar docks (with `sidebar = "auto"`).
    pub wide_from: f32,
    /// Below this window width, tabs collapse into a single compact capsule.
    pub narrow_below: f32,
    /// Width of an inactive tab pill.
    pub tab_width: f32,
    /// Corner radius of the web page area.
    pub page_corner_radius: f32,
    /// Gap between the page and the window's edges (and a floating sidebar's side).
    pub page_margin: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            margin: 16.0,
            bar_height: 56.0,
            sidebar_width: 316.0,
            sidebar: SidebarMode::Auto,
            sidebar_visible: true,
            wide_from: 1280.0,
            narrow_below: 800.0,
            tab_width: 150.0,
            page_corner_radius: 10.0,
            page_margin: 8.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    /// Lock private tabs behind the password (set in Settings › Privacy).
    pub lock_private_tabs: bool,
    /// Lock after the window has been in the background this long (seconds).
    pub lock_after_seconds: f32,
}

impl Default for Privacy {
    fn default() -> Self {
        Self { lock_private_tabs: true, lock_after_seconds: 60.0 }
    }
}

/// How hard Shields blocks on a site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShieldLevel {
    Off,
    /// Third-party ads and trackers, ad popups and ad redirects.
    #[default]
    Standard,
    /// Also first-party ads, every cross-site redirect without a click, and `utm_` parameters.
    Aggressive,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shields {
    /// Level for sites without their own setting (set per site in the Shields popover).
    pub default_level: ShieldLevel,
    /// Hide ad placeholders left on the page (cosmetic filters).
    pub hide_ad_elements: bool,
    /// Block tabs and windows a page opens without a click or key press.
    pub block_popups: bool,
    /// Block redirects the page makes without a click to sites on the block lists.
    pub block_redirects: bool,
    /// Remove click-tracking parameters (fbclid, gclid, …) from links.
    pub strip_tracking_parameters: bool,
    pub block_third_party_cookies: bool,
    /// Filter lists in Adblock Plus / uBlock Origin syntax.
    pub lists: Vec<String>,
    /// Refresh the lists after this many hours.
    pub update_interval_hours: f32,
}

impl Default for Shields {
    fn default() -> Self {
        Self {
            default_level: ShieldLevel::Standard,
            hide_ad_elements: true,
            block_popups: true,
            block_redirects: true,
            strip_tracking_parameters: true,
            block_third_party_cookies: true,
            lists: vec![
                "https://easylist.to/easylist/easylist.txt".into(),
                "https://easylist.to/easylist/easyprivacy.txt".into(),
                "https://ublockorigin.github.io/uAssets/filters/filters.txt".into(),
                "https://ublockorigin.github.io/uAssets/filters/privacy.txt".into(),
                "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext"
                    .into(),
            ],
            update_interval_hours: 96.0,
        }
    }
}

/// Tab lifecycle and memory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tabs {
    /// Freeze tabs (pause their scripts and timers) after this long in the background (0: never).
    pub freeze_after_minutes: f32,
    /// Unload tabs (keep only URL and a snapshot) after this long in the background (0: never).
    pub discard_after_minutes: f32,
    /// Unload background tabs while the browser uses more memory than the budget.
    pub limit_memory: bool,
    /// Memory budget for the whole browser in MB; 0 means 40 % of the computer's memory.
    pub memory_budget_mb: u32,
    /// Tabs playing audio are never frozen or unloaded.
    pub keep_media_awake: bool,
    /// Reopen the last session's tabs at start (they load when first shown).
    pub restore_session: bool,
    /// How often memory use is measured and the tab rules are applied.
    pub check_interval_seconds: f32,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            freeze_after_minutes: 10.0,
            discard_after_minutes: 120.0,
            limit_memory: true,
            memory_budget_mb: 0,
            keep_media_awake: true,
            restore_session: true,
            check_interval_seconds: 5.0,
        }
    }
}

impl Config {
    /// The full default configuration as TOML, for documentation and the settings UI.
    pub fn default_toml() -> String {
        toml::to_string_pretty(&Self::default()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_default() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c: Config = toml::from_str(
            r##"
            [appearance]
            accent = "#E2457A"
            [glass]
            squash = 0.0
            "##,
        )
        .unwrap();
        assert_eq!(c.appearance.accent, Color::rgb(0xE2457A));
        assert_eq!(c.appearance.font_family, "Inter");
        assert_eq!(c.glass.squash, 0.0);
        assert_eq!(c.glass.bezel_width, Glass::default().bezel_width);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("[appearance]\naccnt = \"#000000\"").is_err());
    }

    #[test]
    fn default_round_trips() {
        let text = Config::default_toml();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, Config::default());
    }
}
