//! The settings schema. Defaults come from the design system ("Foundations" board).

use serde::{Deserialize, Serialize};

use crate::Color;

/// All of Axion's settings. Every section and key is optional in `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub home: Home,
    pub appearance: Appearance,
    pub glass: Glass,
    pub colors: Colors,
    pub radii: Radii,
    pub motion: Motion,
    pub layout: Layout,
    pub privacy: Privacy,
    pub permissions: Permissions,
    pub shields: Shields,
    pub tabs: Tabs,
    pub media: Media,
    pub hardware: Hardware,
    pub playlist: Playlist,
    pub reader: Reader,
    pub translation: Translation,
    pub downloads: Downloads,
    pub data: Data,
    pub passwords: Passwords,
    pub accounts: Accounts,
    pub import: Import,
    pub find: Find,
    pub zoom: Zoom,
    pub command_palette: CommandPalette,
    pub print: Print,
    pub screenshot: Screenshot,
    pub devtools: DevTools,
    pub clipboard: Clipboard,
    pub toast: Toast,
    pub sidebar: Sidebar,
    pub window: WindowBehavior,
    pub ui: Ui,
    pub updates: Updates,
    pub extensions: Extensions,
}

/// Importing bookmarks and history from other browsers (Settings › Data & Downloads).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Import {
    /// At most this many history entries are imported per browser profile, the most recently
    /// visited first.
    pub max_history_items: u32,
    /// At most this many bookmarks are imported per profile or file.
    pub max_bookmarks: u32,
    /// Browser databases larger than this are not imported (MB; they are copied first, so
    /// this much temporary space is needed).
    pub max_database_mb: u32,
    /// Bookmark files (HTML, JSON) larger than this are refused (MB).
    pub max_file_mb: u32,
}

impl Default for Import {
    fn default() -> Self {
        Self { max_history_items: 20_000, max_bookmarks: 50_000, max_database_mb: 512, max_file_mb: 64 }
    }
}

/// Account profile pictures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Accounts {
    /// A chosen picture is cropped to a square and scaled to this many pixels per side
    /// (stored as PNG in the account's directory).
    pub avatar_size_px: u32,
    /// Larger picture files are refused (MB).
    pub avatar_max_mb: u32,
}

impl Default for Accounts {
    fn default() -> Self {
        Self { avatar_size_px: 256, avatar_max_mb: 20 }
    }
}

/// Axion's own start page, served at this address.
pub const START_PAGE_URL: &str = "axion://home/";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Page opened in new windows and tabs (`axion://home/`: Axion's start page).
    pub homepage: String,
    /// Search URL; `{}` is replaced by the URL-encoded query.
    pub search_url: String,
    /// Language of Axion's own interface (applied when Axion starts).
    pub language: UiLanguage,
    /// The first-run introduction has been finished or skipped (false on a fresh install: it
    /// shows once; Settings › General › Show Welcome Tour runs it again). Accounts that
    /// existed before the tour was added count as done.
    pub onboarding_done: bool,
    /// Linux: add Axion to the application menu at start (a per-user `axion.desktop` and the
    /// icon set in `$XDG_DATA_HOME`), which is what gives the window its icon on Wayland.
    /// Off: Axion leaves the desktop entry alone (and removes nothing).
    pub desktop_integration: bool,
}

/// Which language the interface is shown in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    /// The system's (`LC_ALL`/`LC_MESSAGES`/`LANG`), English when unsupported.
    #[default]
    System,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "de")]
    German,
}

/// The homepage Axion used before it had its own start page; configs that still carry it (or
/// none at all) get the start page.
const OLD_DEFAULT_HOMEPAGE: &str = "https://duckduckgo.com/";

impl General {
    /// The homepage in effect: the user's own URL, or Axion Home when none is set.
    pub fn effective_homepage(&self) -> &str {
        let home = self.homepage.trim();
        if home.is_empty() { START_PAGE_URL } else { home }
    }

    /// Text for the settings field: empty while the default applies, so its placeholder shows.
    pub fn homepage_field_text(&self) -> &str {
        if self.effective_homepage() == START_PAGE_URL { "" } else { self.homepage.trim() }
    }

    /// Stores what the user typed; blank returns to the default (Axion Home).
    pub fn set_homepage(&mut self, typed: &str) {
        let typed = typed.trim();
        self.homepage = if typed.is_empty() { START_PAGE_URL.into() } else { typed.into() };
    }

    /// An empty homepage or the old default becomes Axion's start page.
    pub fn upgrade_homepage(&mut self) {
        if self.homepage.trim().is_empty() || self.homepage == OLD_DEFAULT_HOMEPAGE {
            self.homepage = START_PAGE_URL.into();
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            homepage: START_PAGE_URL.into(),
            search_url: "https://duckduckgo.com/?q={}".into(),
            language: UiLanguage::System,
            onboarding_done: false,
            desktop_integration: true,
        }
    }
}

#[cfg(test)]
mod homepage_tests {
    use super::*;

    #[test]
    fn effective_homepage_resolution() {
        let mut g = General::default();
        assert_eq!(g.effective_homepage(), START_PAGE_URL);
        assert_eq!(g.homepage_field_text(), "");
        g.set_homepage("  https://start.example/ ");
        assert_eq!(g.effective_homepage(), "https://start.example/");
        assert_eq!(g.homepage_field_text(), "https://start.example/");
        g.set_homepage("   ");
        assert_eq!(g, General::default());
        g.homepage = String::new();
        assert_eq!(g.effective_homepage(), START_PAGE_URL);
    }
}

#[cfg(test)]
mod devtools_tests {
    use super::*;

    #[test]
    fn developer_tools_default_on_and_switchable() {
        assert!(Config::default().devtools.enabled);
        let config: Config = toml::from_str("[devtools]\nenabled = false\n").unwrap();
        assert!(!config.devtools.enabled);
    }
}

/// 12- or 24-hour clock on the start page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClockFormat {
    /// From the locale (`LC_TIME` / `LANG`).
    #[default]
    Auto,
    #[serde(rename = "12h")]
    H12,
    #[serde(rename = "24h")]
    H24,
}

/// The start page (`axion://home/`): which widgets it shows. Toggled from the gear on the page
/// itself or in Settings › General.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Home {
    pub show_clock: bool,
    pub clock_format: ClockFormat,
    /// "Good morning, <account name>" under the clock.
    pub show_greeting: bool,
    pub show_search: bool,
    /// Most visited sites (and pinned ones) as tiles.
    pub show_top_sites: bool,
    /// How many tiles at most: pinned sites first, then the most visited domains.
    pub top_sites_count: u32,
    /// Only visits within this many days count towards the ranking.
    pub top_sites_window_days: u32,
    /// Tiles per row and rows per page; more tiles than fit page with dots.
    pub top_sites_columns: u32,
    pub top_sites_rows: u32,
    /// Removed (`top_sites_count` limits the tiles now); still accepted in old files.
    #[serde(skip_serializing)]
    top_sites_pages: Option<u32>,
    /// Trackers & ads blocked, bandwidth and time saved.
    pub show_stats: bool,
    /// The selected background image: a file name in the account's backgrounds directory
    /// (`<profile>/home/backgrounds/`, where added images are copied). Empty: none, the
    /// background is the chrome's own neutral ground (see `glass_ground`).
    pub background: String,
    /// Without a background image the start page takes the chrome's neutral ground colour
    /// (`colors.*.ground`, the tone around the page card and behind the bars), in light and
    /// dark. Off: plain white or black with the theme. Private tabs stay dark either way.
    pub glass_ground: bool,
    /// At most this many backgrounds are kept (adding more is refused).
    pub max_backgrounds: u32,
    /// Larger files are refused as backgrounds (MB). Live wallpapers (videos) are big.
    pub max_background_mb: u32,
    /// Animated GIF/WebP/APNG and video backgrounds play (muted, looped). Off (or with Reduce
    /// Motion on and `live_wallpapers_ignore_reduce_motion` off, or while the tab is hidden) they show their first frame. Private tabs
    /// never show a background.
    pub live_wallpapers: bool,
    /// Live wallpapers keep playing when Reduce Motion (`glass.reduce_motion`) is on. Off, they
    /// follow Reduce Motion and show their first frame.
    pub live_wallpapers_ignore_reduce_motion: bool,
    /// Darkens the background image for legibility (0–1).
    pub background_dim: f32,
    /// Blurs the background image (CSS px).
    pub background_blur: f32,
    /// An open start page refreshes its Shields numbers at most this often.
    pub stats_refresh_seconds: f32,
    /// Older setting (one background image, shown or not), replaced by `background`; accepted
    /// so old files load, and read once to migrate the old image.
    #[serde(skip_serializing)]
    show_background_image: Option<bool>,
}

impl Home {
    /// Whether an old config showed its single background image (see `background`).
    pub fn legacy_background_shown(&self) -> bool {
        self.show_background_image.unwrap_or(false)
    }
}

impl Default for Home {
    fn default() -> Self {
        Self {
            show_clock: true,
            clock_format: ClockFormat::Auto,
            show_greeting: true,
            show_search: true,
            show_top_sites: true,
            top_sites_count: 5,
            top_sites_window_days: 90,
            top_sites_columns: 6,
            top_sites_rows: 2,
            top_sites_pages: None,
            show_stats: true,
            background: String::new(),
            glass_ground: true,
            max_backgrounds: 24,
            max_background_mb: 200,
            live_wallpapers: true,
            live_wallpapers_ignore_reduce_motion: true,
            background_dim: 0.2,
            background_blur: 0.0,
            stats_refresh_seconds: 2.0,
            show_background_image: None,
        }
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

/// What happens to a site icon that would vanish on the chrome (a black glyph on dark chrome,
/// a white one on light chrome).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaviconContrast {
    /// Icons are shown as the site made them.
    Off,
    /// A small neutral tile in the opposite tone goes behind the icon.
    Backing,
    /// Single-colour glyphs are inverted (white on dark chrome, like Safari and Arc); coloured
    /// icons that vanish get the tile.
    #[default]
    Invert,
}

/// How the window tint is picked from the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TintMode {
    /// The page's colour is never used: the glass sits on Apple's system greys (light: off-white
    /// `#F2F2F7`, dark: `#1C1C1E`), whatever the website looks like. The default; see `Colors`.
    #[default]
    Neutral,
    /// The page's darkest (dark pages) or lightest (light pages) prominent colour.
    Extreme,
    /// The colour most of the page shows, evened out into a calm light or dark tone.
    Dominant,
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
    /// Switch the chrome between light and dark to match the page's brightness (opt-in; off:
    /// the chrome follows `theme` only, so it never depends on the website).
    pub adapt_to_page: bool,
    /// What to do with site icons that are hard to see on the chrome (see `FaviconContrast`).
    pub favicon_contrast: FaviconContrast,
    /// Icon stroke weight in the light chrome, as a multiple of the design's base stroke
    /// (1.75 in a 24-unit box). Dark-on-light icons look thinner than light-on-dark ones, so
    /// light defaults a little heavier. Clamped to 0.5-2.0 when used. Only the stroke changes;
    /// icons are never grid-snapped.
    pub icon_stroke_light: f32,
    /// Icon stroke weight in the dark chrome (multiple of the base stroke, 1.0 = the design).
    pub icon_stroke_dark: f32,
    /// Give the window around the page a colour of the page (like Safari's "colour in tab
    /// bar"), measured once per page (opt-in; `tint_mode` must not be `neutral` for it to
    /// show: turning this on in Settings switches `neutral` to `extreme`).
    pub tint_from_page: bool,
    /// The page's colour is measured this long after it finished loading (ms), so late
    /// styles and images are in.
    pub page_color_delay_ms: f32,
    /// How the tint is picked: `neutral` (default: never from the page), `extreme` (darkest/lightest prominent colour of the page's top
    /// region, on the side of the page's overall brightness) or `dominant` (calmed main colour).
    pub tint_mode: TintMode,
    /// `extreme` mode: the top part of the page (0-1 of its height) the colour is taken from.
    pub tint_top_region: f32,
    /// `extreme` mode: a colour is prominent when it covers at least this share (0-1) of that region.
    pub tint_min_coverage: f32,
    /// `extreme` mode: use the page's `theme-color` when it has one, instead of the measured colour.
    pub tint_prefer_theme_color: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Auto,
            accent: Color::rgb(0x2E6BFF),
            accent_tint: 0.06,
            font_family: "Inter".into(),
            text_scale: 1.0,
            adapt_to_page: false,
            favicon_contrast: FaviconContrast::Invert,
            icon_stroke_light: 1.2,
            icon_stroke_dark: 1.0,
            tint_from_page: false,
            page_color_delay_ms: 350.0,
            tint_mode: TintMode::Neutral,
            tint_top_region: 0.25,
            tint_min_coverage: 0.03,
            tint_prefer_theme_color: false,
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

/// How the chrome is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlassStyle {
    /// Liquid Glass: refraction, blur, merging drops and the glass physics.
    #[default]
    Liquid,
    /// Flat: solid cards with a border and a soft shadow. No refraction, blur, merging, drips,
    /// stretch or bounce, and the compositor skips the glass passes entirely (the performance
    /// mode for weak GPUs). Everything else works exactly the same.
    Flat,
}

/// The damping ratio flat mode raises every spring to: critically damped, so nothing bounces.
pub const FLAT_DAMPING: f32 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Glass {
    /// Liquid Glass or the flat, glass-free look (see [`GlassStyle`]). Live-switchable.
    pub style: GlassStyle,
    pub light: GlassMaterial,
    pub dark: GlassMaterial,
    /// Width of the curved rim in logical pixels.
    pub bezel_width: f32,
    /// Distance in logical pixels at which neighbouring glass starts to flow together.
    pub merge_distance: f32,
    /// Maximum squash along the direction of motion (0.08 = 8%).
    pub squash: f32,
    /// Smooth-union radius in logical pixels of the broad glass bridge that joins a popover
    /// or menu to the bar or button it flows out of: how wide the concave fillets at the
    /// junction's ends are (at most `merge_distance` has no extra effect).
    pub popover_bridge_fillet: f32,
    /// Opacity (0–1) of the soft plate under a menu's rows: menus carry text, so their glass is
    /// made denser than thick glass in the middle while the edges still refract.
    pub menu_plate_opacity: f32,
    /// Tint opacity (0–1) of every glass *button* (a glass widget with an action), whatever its
    /// thickness: buttons are pure refractive glass, never a grey plate. Text-bearing panels keep
    /// their level's opacity.
    pub button_opacity: f32,
    /// Tint opacity (0–1) of tinted (primary, destructive) glass buttons: how much of the accent
    /// colour the glass takes. The rest is the backdrop, refracted, with the rim light on top.
    pub tinted_opacity: f32,
    /// Rim light of tinted glass buttons relative to plain glass (1 = the same): coloured glass
    /// reads as glass by its bright, refractive edge.
    pub tinted_rim: f32,
    /// Extra tint opacity over busy or mid-grey backdrops, to keep labels legible.
    pub adaptive_opacity: f32,
    /// Replace glass with opaque surfaces.
    pub reduce_transparency: bool,
    /// Replace springs and squash with short fades.
    pub reduce_motion: bool,
}

impl Glass {
    /// Flat mode: no glass is drawn, merged or animated.
    pub fn is_flat(&self) -> bool {
        self.style == GlassStyle::Flat
    }

    /// Distance at which neighbouring glass flows together (0 in flat mode: nothing merges).
    pub fn merge_distance(&self) -> f32 {
        if self.is_flat() { 0.0 } else { self.merge_distance }
    }

    /// Maximum squash of moving glass (0 under Reduce Motion and in flat mode).
    pub fn squash(&self) -> f32 {
        if self.reduce_motion || self.is_flat() { 0.0 } else { self.squash.max(0.0) }
    }

    /// Smooth-union radius of a popover's bridge (0 in flat mode: no bridges).
    pub fn popover_bridge_fillet(&self) -> f32 {
        if self.is_flat() { 0.0 } else { self.popover_bridge_fillet.max(0.0) }
    }

    /// A spring's damping ratio as used: flat mode never bounces, so it is at least critical.
    pub fn damping(&self, ratio: f32) -> f32 {
        if self.is_flat() { ratio.max(FLAT_DAMPING) } else { ratio }
    }
}

impl Default for Glass {
    fn default() -> Self {
        let blur = Levels { thin: 16.0, regular: 26.0, thick: 40.0 };
        Self {
            style: GlassStyle::Liquid,
            light: GlassMaterial {
                tint: Color::rgb(LIGHT_GLASS_TINT),
                opacity: Levels { thin: 0.30, regular: 0.48, thick: 0.66 },
                blur,
                refraction: 0.55,
                dispersion: 0.12,
                rim: 0.95,
                shadow: 0.20,
            },
            dark: GlassMaterial {
                tint: Color::rgb(DARK_GLASS_TINT),
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
            popover_bridge_fillet: 30.0,
            menu_plate_opacity: 0.6,
            button_opacity: 0.04,
            tinted_opacity: 0.65,
            tinted_rim: 1.6,
            adaptive_opacity: 0.12,
            reduce_transparency: false,
            reduce_motion: false,
        }
    }
}

/// Apple's system greys, the neutral chrome of `appearance.tint_mode = "neutral"`. Apple
/// uses neither pure white nor pure black as a background, and neither do we.
/// Light: `systemGroupedBackground` for the ground (window behind the page card, bars,
/// popovers, flat-mode and Reduce-Transparency surfaces), a hair lighter for raised surfaces
/// (fields, cards) and for the glass tint.
pub const LIGHT_GROUND: u32 = 0xF2F2F7;
pub const LIGHT_SURFACE: u32 = 0xFAFAFC;
pub const LIGHT_GLASS_TINT: u32 = 0xF9F9FB;
/// Dark: `systemBackground`-dark grey for the ground, `secondarySystemBackground` for raised
/// surfaces and the glass tint, `tertiarySystemBackground` for separators.
pub const DARK_GROUND: u32 = 0x1C1C1E;
pub const DARK_SURFACE: u32 = 0x2C2C2E;
pub const DARK_GLASS_TINT: u32 = 0x2C2C2E;
pub const DARK_TERTIARY: u32 = 0x3A3A3C;

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
    /// Shields icon when protection is active (unset: the accent colour).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield: Option<Color>,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            light: Palette {
                ground: Color::rgb(LIGHT_GROUND),
                surface: Color::rgb(LIGHT_SURFACE),
                label: Color::rgb(0x111318),
                secondary: Color::rgb(0x4A5060),
                hairline: Color::rgb(0xD9DCE3),
            },
            dark: Palette {
                ground: Color::rgb(DARK_GROUND),
                surface: Color::rgb(DARK_SURFACE),
                label: Color::rgb(0xF5F6F8),
                secondary: Color::rgb(0xB9BDC8),
                hairline: Color::rgb(DARK_TERTIARY),
            },
            shield: None,
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
    /// A closing popover or sheet draining back into the glass it came from: its far edge
    /// and sides (the body retracting toward its anchor). Underdamped, so the body shoots
    /// into the glass's rim and recoils out once before it is absorbed.
    pub drain: SpringSpec,
    /// … and its near edge (the neck sinking into the anchor's glass, ahead of the body).
    pub drain_neck: SpringSpec,
    /// The last drop, after the recoil, flowing all the way into the anchor's glass.
    pub drain_absorb: SpringSpec,
    /// Size of the drop left at the anchor's rim when the recoil ends (logical px).
    pub drain_drop: f32,
    /// Overview zoom, scroll hand-off.
    pub fling: SpringSpec,
    /// A closing tab shrinking and fading away (tab bar, sidebar, overview). The tabs beside it
    /// slide into the gap on `snappy`.
    pub tab_close: SpringSpec,
    /// Multiplies every spring response (0.5 = twice as fast).
    pub speed: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            snappy: SpringSpec { response: 0.28, damping: 0.90 },
            smooth: SpringSpec { response: 0.45, damping: 1.00 },
            liquid: SpringSpec { response: 0.50, damping: 0.62 },
            drain: SpringSpec { response: 0.42, damping: 0.55 },
            drain_neck: SpringSpec { response: 0.34, damping: 1.00 },
            drain_absorb: SpringSpec { response: 0.20, damping: 1.00 },
            drain_drop: 18.0,
            fling: SpringSpec { response: 0.60, damping: 0.85 },
            tab_close: SpringSpec { response: 0.34, damping: 0.95 },
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

/// How the page follows a docked sidebar that slides in or out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSlideResize {
    /// The page is re-laid out continuously while the panel moves (a new size every few
    /// pixels); Chromium's frames lag a frame or two behind the rectangle.
    #[default]
    Live,
    /// The page is resized once, as the slide starts, and stands still under or beside the
    /// panel until the slide settles.
    Final,
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
    /// Whether the page resizes with a docked sidebar while it slides, or once it has settled.
    pub sidebar_slide_resize: SidebarSlideResize,
    /// With `sidebar_slide_resize = "live"`: the page is resized again once the panel moved
    /// this many logical pixels (each size costs Chromium a layout and a frame).
    pub sidebar_slide_resize_step: f32,
    /// With `sidebar_slide_resize = "live"`: seconds the page is resized ahead of the panel,
    /// so Chromium's frame of a size arrives as the panel gets there (the first estimate of
    /// Chromium's delay, then measured).
    pub sidebar_slide_lead: f32,
    /// While the window is being resized: seconds the shown page may take to deliver its
    /// frame of the size it was last given before it is given the window's newest size anyway
    /// (sizes in between are skipped; each costs Chromium a layout and a frame). 0 resizes
    /// the page with every step of the window.
    pub window_resize_wait: f32,
    /// Seconds after the window last changed size before the tabs in the background follow
    /// it (they are not shown meanwhile; resizing them at every step made a resize lag).
    pub background_resize_delay: f32,
    /// Always float the sidebar over the page as a rounded glass card (it never pushes the
    /// page), whatever the window size.
    pub float_sidebar: bool,
    /// Corner radius of the sidebar card (docked or floating).
    pub sidebar_corner_radius: f32,
    /// From this window width up, the sidebar docks (with `sidebar = "auto"`).
    pub wide_from: f32,
    /// Below this window width, tabs collapse into a single compact capsule.
    pub narrow_below: f32,
    /// Preferred width of a tab in the separate tab bar; all tabs share it, and they shrink
    /// equally when they don't fit.
    pub tab_width: f32,
    /// Compact tab bar: the widest a tab segment gets (all segments, the open one with the
    /// address included, are equally wide); 0 = they fill the capsule.
    pub compact_tab_max_width: f32,
    /// Compact tab bar: while the address is being edited, the open segment grows to this width
    /// (at most the capsule's room); the others shrink equally to make space.
    pub compact_address_width: f32,
    /// Below this width a tab drops its title (and close button) and shows only its favicon.
    pub tab_min_width: f32,
    /// Tabs shrink no further than this (icon only, logical px); beyond it they overflow.
    pub tab_icon_width: f32,
    /// Width of a pinned tab (favicon only); 0 = a square as high as the tab row.
    pub pinned_tab_width: f32,
    /// When the bookmarks bar under the toolbar is shown.
    pub bookmarks_bar: BookmarksBar,
    /// Corner radius of the web page area.
    pub page_corner_radius: f32,
    /// Gap between the page and the window's edges (and a floating sidebar's side).
    pub page_margin: f32,
    /// Tabs in their own row under the address bar, or in one compact row with it.
    pub tab_bar: TabBarStyle,
    /// A home button (opens `general.homepage`) next to back and forward.
    pub home_button: bool,
    /// F11 fullscreen: resting the pointer at the window's top edge brings the toolbar, tab
    /// row and bookmarks bar back over the page (it keeps its size) until the pointer leaves
    /// them. Off: they only come back when fullscreen ends.
    pub fullscreen_reveal_chrome: bool,
    /// Seconds the pointer must rest at the top edge before the bars come back.
    pub fullscreen_reveal_delay: f32,
    /// How close to the top edge (logical px) the pointer counts as at it.
    pub fullscreen_reveal_edge: f32,
    /// Removed (the bars no longer hide while scrolling); still accepted in old files.
    #[serde(skip_serializing)]
    hide_bar_on_scroll: Option<bool>,
}

/// When the bookmarks bar is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BookmarksBar {
    #[default]
    Always,
    /// Only on the home page (new tabs).
    Home,
    Never,
}

/// Safari's two tab bar layouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabBarStyle {
    /// Address bar row, and the tabs in a row below it (the default).
    #[default]
    Separate,
    /// One row: the tabs, the current one holding the address.
    Compact,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            margin: 16.0,
            bar_height: 46.0,
            sidebar_width: 316.0,
            sidebar: SidebarMode::Auto,
            sidebar_visible: true,
            sidebar_slide_resize: SidebarSlideResize::Live,
            sidebar_slide_resize_step: 2.0,
            sidebar_slide_lead: 0.05,
            window_resize_wait: 0.1,
            background_resize_delay: 0.3,
            float_sidebar: false,
            sidebar_corner_radius: 26.0,
            wide_from: 1280.0,
            narrow_below: 800.0,
            tab_width: 260.0,
            compact_tab_max_width: 0.0,
            compact_address_width: 480.0,
            tab_min_width: 72.0,
            tab_icon_width: 36.0,
            pinned_tab_width: 0.0,
            bookmarks_bar: BookmarksBar::Always,
            page_corner_radius: 10.0,
            page_margin: 8.0,
            tab_bar: TabBarStyle::Separate,
            home_button: true,
            fullscreen_reveal_chrome: true,
            fullscreen_reveal_delay: 0.35,
            fullscreen_reveal_edge: 3.0,
            hide_bar_on_scroll: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    /// Lock private tabs behind the account password (set in Settings › Privacy). Entering
    /// the password once unlocks them until Axion quits or the account menu's Lock is used.
    pub lock_private_tabs: bool,
    /// Open the password card (focused) by itself when locked private browsing comes up.
    pub prompt_unlock_on_open: bool,
    /// Older setting (locking again after time in the background), no longer used: an unlock
    /// lasts for the session. Accepted so old files load.
    #[serde(skip_serializing)]
    lock_after_seconds: Option<f32>,
}

impl Default for Privacy {
    fn default() -> Self {
        Self { lock_private_tabs: true, prompt_unlock_on_open: true, lock_after_seconds: None }
    }
}

/// Chrome extensions (Manifest V3, installed from the Chrome Web Store, an unpacked folder or a
/// `.crx` file).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Extensions {
    /// Run Chromium's extension system and show the extensions button in the toolbar. Off: no
    /// extension loads and the button is hidden. Applies after a restart. Default: on.
    pub enabled: bool,
    /// "Add to Chrome" on the Chrome Web Store installs extensions (after a confirmation in
    /// Axion's own dialog). Off: the store opens as a normal website and cannot install.
    /// Needs `enabled`. Default: on.
    pub web_store: bool,
    /// Check Web Store extensions for newer versions in the background (no other site is
    /// contacted; an update runs after the next restart). Default: on.
    pub auto_update: bool,
    /// Turn on Chromium's "Allow User Scripts" switch for extensions that ask for the
    /// `userScripts` permission (Tampermonkey, Violentmonkey: they need it to run scripts).
    /// Default: on.
    pub allow_user_scripts: bool,
    /// Remove the `webRequest` and `declarativeNetRequest` permissions from installed
    /// extensions. Today they make Chromium stall every navigation to a website in Axion's
    /// off-screen browser, so extensions that hold them (and most blockers do) would break
    /// browsing. Userscript managers, content scripts and the other APIs are not affected.
    /// Turn off to test once the engine handles them. Default: on.
    pub block_request_apis: bool,
    /// Hours between two update checks. At least 1. Default: 24.
    pub update_interval_hours: u32,
    /// Seconds a Web Store request (a download or an update check) may take. Default: 60.
    pub timeout_seconds: u32,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            enabled: true,
            web_store: true,
            auto_update: true,
            allow_user_scripts: true,
            block_request_apis: true,
            update_interval_hours: 24,
            timeout_seconds: 60,
        }
    }
}

/// What happens when a site asks for a permission nobody has decided for it yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDefault {
    /// Show the prompt.
    #[default]
    Ask,
    /// Refuse without asking.
    Block,
}

impl PermissionDefault {
    pub fn blocks(self) -> bool {
        self == PermissionDefault::Block
    }
}

/// Site permissions: what sites may ask for, and what Axion does when they do. Decisions for
/// single sites are remembered in the profile (Settings › Privacy › Site Permissions), not here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Permissions {
    pub microphone: PermissionDefault,
    pub camera: PermissionDefault,
    /// Sharing a screen or window (calls, Discord streams). Allowing is never remembered:
    /// every share asks.
    pub screen_sharing: PermissionDefault,
    pub notifications: PermissionDefault,
    pub location: PermissionDefault,
    /// Reading the clipboard (writing to it needs no permission).
    pub clipboard: PermissionDefault,
    /// Everything else sites can ask for: MIDI devices, the local network, window placement,
    /// installed fonts, idle detection, pointer and keyboard lock, sensors, several downloads.
    pub other: PermissionDefault,
    /// "Stop" in the capture popover ends the page's tracks; if the page still records this
    /// long after, the page is reloaded (milliseconds).
    pub stop_reload_after_ms: u32,
    /// Who chooses what a page shares when it calls `getDisplayMedia()`.
    pub screen_picker: ScreenPicker,
    /// Most screens and windows the picker lists (screens first).
    pub screen_picker_max_sources: u32,
    /// Width of the picker's thumbnails (logical pixels); their height follows at 16:9.
    pub screen_picker_thumbnail_width: u32,
}

/// Who chooses what a shared screen or window is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenPicker {
    /// Axion's own picker on X11; the desktop's (screen cast portal) on Wayland.
    #[default]
    Auto,
    /// Axion's own picker wherever it can list sources (X11 and XWayland).
    Own,
    /// Chromium and the desktop decide (the portal's dialog, else the whole screen).
    System,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            microphone: PermissionDefault::Ask,
            camera: PermissionDefault::Ask,
            screen_sharing: PermissionDefault::Ask,
            notifications: PermissionDefault::Ask,
            location: PermissionDefault::Ask,
            clipboard: PermissionDefault::Ask,
            other: PermissionDefault::Ask,
            stop_reload_after_ms: 1500,
            screen_picker: ScreenPicker::Auto,
            screen_picker_max_sources: 12,
            screen_picker_thumbnail_width: 160,
        }
    }
}

/// When finished downloads leave the Downloads list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveDownloads {
    /// Only with "Clear List" or a row's remove button.
    #[default]
    Manually,
    AfterOneDay,
    /// When Axion quits.
    OnQuit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Downloads {
    /// Folder new downloads go to; empty means the desktop's Downloads folder.
    pub directory: String,
    /// Ask where to save each file instead of saving it straight to `directory`.
    pub ask_where_to_save: bool,
    /// Open "safe" files (images, PDFs, text, audio, video) once they finished downloading.
    pub open_safe_files: bool,
    pub remove_list_items: RemoveDownloads,
    /// How many finished downloads the list keeps at most.
    pub list_limit: u32,
}

impl Default for Downloads {
    fn default() -> Self {
        Self {
            directory: String::new(),
            ask_where_to_save: true,
            open_safe_files: false,
            remove_list_items: RemoveDownloads::Manually,
            list_limit: 500,
        }
    }
}

/// How far back "Clear Browsing Data" goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearRange {
    LastHour,
    Today,
    LastWeek,
    #[default]
    AllTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Data {
    /// Delete all cookies and site data when Axion quits.
    pub clear_cookies_on_quit: bool,
    /// What "Clear Browsing Data" selects (remembered between uses).
    pub clear_history: bool,
    pub clear_cookies: bool,
    pub clear_cache: bool,
    pub clear_downloads: bool,
    pub clear_range: ClearRange,
    /// How long (milliseconds) the window waits for the history database when another part of
    /// Axion (an import) holds it, before the entry is skipped. The window freezes meanwhile.
    pub db_busy_timeout_ms: u32,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            clear_cookies_on_quit: false,
            clear_history: true,
            clear_cookies: true,
            clear_cache: true,
            clear_downloads: true,
            clear_range: ClearRange::AllTime,
            db_busy_timeout_ms: 500,
        }
    }
}

/// The password manager (Settings › Passwords). The master password itself is never stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Passwords {
    /// Offer to save a password after signing in to a site.
    pub offer_to_save: bool,
    /// Offer to fill saved logins into sign-in forms.
    pub autofill: bool,
    /// Older setting (locking after inactivity), no longer used: the account stays unlocked
    /// for the session. Accepted so old files load.
    #[serde(skip_serializing)]
    auto_lock_minutes: Option<f32>,
    /// Empty the clipboard this long after copying a password or card number (0: never).
    pub clear_clipboard_seconds: f32,
    /// Length of generated passwords.
    pub generator_length: u32,
    /// Generated passwords include symbols.
    pub generator_symbols: bool,
    /// Groups of registrable domains that share one sign-in: a login saved for one is offered
    /// on the others (google.com and youtube.com). Subdomains of a member belong to it.
    /// Domains of one registrable domain (accounts.google.com, google.com) always match.
    pub related_domains: Vec<Vec<String>>,
    /// How long the user name typed on the first step of a two-step sign-in is remembered, to
    /// pair it with the password asked on the next page (0: don't pair).
    pub two_step_memory_seconds: f32,
    /// How long a declined "Save password?" / "Change Password?" is remembered for that exact
    /// credential (same site, user name and password; in memory only). A different password
    /// always prompts again.
    pub decline_memory_seconds: f32,
    /// A login filled from the vault counts as "just used" for this long: a different password
    /// submitted for the same account then waits for `login_check_seconds` before it is offered.
    pub fill_recent_seconds: f32,
    /// After such a submit the page gets this long to show the sign-in form again; if it does,
    /// the login most likely failed (a typo) and no "Change Password?" is offered.
    pub login_check_seconds: f32,
    /// Fetch the icons of saved logins' sites that aren't cached yet, for the Passwords list.
    pub fetch_missing_icons: bool,
    /// At most this many icons are fetched each time the list opens.
    pub icon_fetch_limit: u32,
}

impl Default for Passwords {
    fn default() -> Self {
        let group = |domains: &[&str]| domains.iter().map(|d| (*d).to_owned()).collect::<Vec<_>>();
        Self {
            offer_to_save: true,
            autofill: true,
            auto_lock_minutes: None,
            clear_clipboard_seconds: 60.0,
            generator_length: 20,
            generator_symbols: true,
            related_domains: vec![
                group(&["google.com", "youtube.com", "gmail.com"]),
                group(&[
                    "microsoft.com",
                    "live.com",
                    "microsoftonline.com",
                    "office.com",
                    "outlook.com",
                    "xbox.com",
                    "bing.com",
                    "skype.com",
                ]),
                group(&["apple.com", "icloud.com"]),
                group(&["facebook.com", "instagram.com", "messenger.com", "meta.com"]),
                group(&["amazon.com", "amazon.de", "amazon.co.uk", "amazon.fr", "amazon.it", "amazon.es"]),
                group(&["adobe.com", "adobelogin.com"]),
                group(&["twitter.com", "x.com"]),
                group(&["yahoo.com", "aol.com"]),
            ],
            two_step_memory_seconds: 300.0,
            decline_memory_seconds: 1800.0,
            fill_recent_seconds: 120.0,
            login_check_seconds: 4.0,
            fetch_missing_icons: true,
            icon_fetch_limit: 40,
        }
    }
}

/// Paper sizes of the print dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperSize {
    #[default]
    A4,
    A3,
    A5,
    Letter,
    Legal,
}

/// Page margins of the print dialog (their widths are `print.margin_*_inches`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintMargins {
    None,
    Narrow,
    #[default]
    Default,
    Wide,
}

/// Print and Save as PDF.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Print {
    /// Folder the PDF goes to when not asking where; empty: the download folder.
    pub directory: String,
    /// The dialog's last choices (they are kept for next time).
    pub paper: PaperSize,
    pub landscape: bool,
    pub margins: PrintMargins,
    /// Print background colours and images.
    pub backgrounds: bool,
    /// Title, date, address and page numbers in the page margins.
    pub header_footer: bool,
    /// Scale of the content in percent.
    pub scale_percent: u32,
    /// Margin widths of the three presets (inches).
    pub margin_narrow_inches: f32,
    pub margin_default_inches: f32,
    pub margin_wide_inches: f32,
    /// Open the PDF in Axion once it is saved.
    pub open_after_saving: bool,
}

impl Default for Print {
    fn default() -> Self {
        Self {
            directory: String::new(),
            paper: PaperSize::A4,
            landscape: false,
            margins: PrintMargins::Default,
            backgrounds: false,
            header_footer: false,
            scale_percent: 100,
            margin_narrow_inches: 0.2,
            margin_default_inches: 0.4,
            margin_wide_inches: 1.0,
            open_after_saving: false,
        }
    }
}

/// File formats of screenshots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotFormat {
    #[default]
    Png,
    Jpeg,
}

/// What happens to a screenshot once it is taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotAfter {
    /// Show it with the choice to copy or save.
    #[default]
    Ask,
    Copy,
    Save,
}

/// The screenshot tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Screenshot {
    /// Folder for saved screenshots when not asking where; empty: the download folder.
    pub directory: String,
    pub format: ShotFormat,
    /// JPEG quality, 1–100.
    pub jpeg_quality: u8,
    pub after_capture: ShotAfter,
    /// A full page screenshot stops at this many CSS pixels of height.
    pub full_page_max_height: u32,
    /// How long a full page screenshot waits after each scroll step for the page to draw (ms).
    pub scroll_settle_ms: u32,
    /// How dark the page gets around the selection (0–1).
    pub dim_opacity: f32,
    /// The smallest selection that counts (logical px); smaller drags are ignored.
    pub min_selection: f32,
}

impl Default for Screenshot {
    fn default() -> Self {
        Self {
            directory: String::new(),
            format: ShotFormat::Png,
            jpeg_quality: 90,
            after_capture: ShotAfter::Ask,
            full_page_max_height: 16_000,
            scroll_settle_ms: 250,
            dim_opacity: 0.45,
            min_selection: 8.0,
        }
    }
}

/// Find on Page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Find {
    /// Match upper and lower case exactly (the find bar's "Aa" toggle).
    pub match_case: bool,
}

/// Page zoom (Ctrl +/-/0), remembered per site.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Zoom {
    /// Zoom of sites that have none of their own (percent).
    pub default_percent: f64,
    /// One zoom step, in Chromium zoom levels (each level is x1.2; 0.5 steps through 100, 110, 120, 131 ...).
    pub step: f64,
    /// Smallest and largest zoom (percent).
    pub min_percent: f64,
    pub max_percent: f64,
    /// Remember a site's zoom (in the profile's site settings) and apply it to every page of
    /// that site. Off: zoom belongs to the tab and resets when another site loads.
    pub remember_per_site: bool,
    /// Show the zoom (with a reset on click) in the address field while a page is not at its
    /// default zoom.
    pub show_indicator: bool,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            default_percent: 100.0,
            step: 0.5,
            min_percent: 25.0,
            max_percent: 500.0,
            remember_per_site: true,
            show_indicator: true,
        }
    }
}

/// The command palette (Ctrl+K).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandPalette {
    /// Rows listed at once (the list scrolls by the selection).
    pub max_rows: u32,
    /// Show each command's shortcut.
    pub show_shortcuts: bool,
    /// The palette is this wide (logical px; it shrinks with the window).
    pub width: f32,
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self { max_rows: 8, show_shortcuts: true, width: 560.0 }
    }
}

/// Chromium's developer tools (unchanged: Axion does not build its own). They open in a
/// window of their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DevTools {
    /// Allows the developer tools (F12, Inspect, View Source). Off, none of it is reachable.
    pub enabled: bool,
}

impl Default for DevTools {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Remembering the window and the panels between runs, and how quitting behaves. The remembered
/// values themselves (position, size, which panels are open) live in the profile's
/// `window.toml`, not here: they change all the time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowBehavior {
    /// Reopen the window where and how big it was (maximized, fullscreen and monitor included).
    pub restore_geometry: bool,
    /// Reopen the sidebar and its sections as they were.
    pub restore_panels: bool,
    /// Reopen Picture in Picture as big as the user last made it (the same area, at the new
    /// video's shape).
    pub restore_pip_size: bool,
    /// The window's state is written this many milliseconds after it last changed.
    pub save_delay_ms: u32,
    /// A remembered position is used only while at least this much of the window (logical px
    /// each way) is still on a connected monitor; else the window opens on the default monitor.
    pub min_visible_px: u32,
    /// Quitting waits this long (milliseconds) for Chromium to shut down; then the process
    /// ends at once (everything of ours is saved before that). 0 never forces the exit.
    pub shutdown_timeout_ms: u32,
    /// How long (milliseconds) quitting waits for the pages to close before it stops
    /// waiting for them.
    pub close_pages_timeout_ms: u32,
}

impl Default for WindowBehavior {
    fn default() -> Self {
        Self {
            restore_geometry: true,
            restore_panels: true,
            restore_pip_size: true,
            save_delay_ms: 800,
            min_visible_px: 120,
            shutdown_timeout_ms: 6000,
            close_pages_timeout_ms: 2500,
        }
    }
}

/// The sidebar's panels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sidebar {
    /// The History panel opens with every day but today folded away (a search shows matches
    /// unfolded either way).
    pub history_collapse_older: bool,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self { history_collapse_older: true }
    }
}

/// General interface behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    /// How long the pointer must rest on a button before its tooltip appears (milliseconds).
    pub tooltip_delay_ms: u32,
    /// The page context menu's "Search … for “text”" row shows at most this many characters of
    /// the selection (cut with an ellipsis).
    pub page_context_menu_label_chars: u32,
    /// The most characters of a selection the context menu searches for.
    pub page_context_menu_query_chars: u32,
    /// The context menu's "Share…" row for selected text (opens a new e-mail with the text and
    /// the page's address).
    pub page_context_menu_share: bool,
    /// "Copy Image" scales images larger than this down (pixels on the long side).
    pub copy_image_max_px: u32,
    /// The first-run introduction's card is at most this wide (logical px); it shrinks with
    /// the window.
    pub onboarding_max_width: f32,
    /// ...and at most this high.
    pub onboarding_max_height: f32,
    /// How strongly the window behind the introduction is veiled with the theme's ground colour (0-1); the steps sit directly on it, with no panel of their own.
    pub onboarding_dim: f32,
    /// A frame that would look exactly like the one on screen is not composited and presented
    /// again: while nothing moves the GPU rests (the window still checks for changes once per
    /// display refresh). Off: every frame is drawn.
    pub skip_unchanged_frames: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            tooltip_delay_ms: 700,
            page_context_menu_label_chars: 14,
            page_context_menu_query_chars: 400,
            page_context_menu_share: true,
            copy_image_max_px: 8192,
            onboarding_max_width: 880.0,
            onboarding_max_height: 640.0,
            onboarding_dim: 0.84,
            skip_unchanged_frames: true,
        }
    }
}

/// The repository Axion's releases are published in, as `owner/name` (GitHub, or any forge
/// with a GitHub-compatible releases API via `updates.api_url`). **Fill this in once the
/// project's repository exists**; while it and `updates.manifest_url` are both empty, the
/// updater does nothing at all (no network access, no notices).
pub const DEFAULT_REPOSITORY: &str = "xyzhenrik/Axion";

/// Which releases the updater offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Only final releases.
    #[default]
    Stable,
    /// Also pre-releases (alpha, beta, rc).
    Prerelease,
}

/// Looking for newer Axion versions (Settings › About). Inert while no source is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Updates {
    /// Master switch for update checks (a no-op while no source is set).
    pub enabled: bool,
    /// `owner/name` of the releases' repository (a full `https://github.com/owner/name`
    /// address works too). Empty (the default, nothing stored): [`DEFAULT_REPOSITORY`]
    /// applies; a value here overrides it. Read it through [`Updates::effective_repository`].
    pub repository: String,
    /// A forge without a GitHub-style API: the address of a JSON manifest
    /// (`{"version": "1.2.3", "notes": "…", "url": "release page", "assets": [{"name": "…",
    /// "url": "…", "sha256": "…"}]}`). Wins over `repository` when both are set.
    pub manifest_url: String,
    /// The GitHub-compatible API of the repository's host.
    pub api_url: String,
    pub channel: UpdateChannel,
    /// Automatic checks happen at most this often (hours).
    pub check_interval_hours: u32,
    /// Check shortly after Axion starts, when the interval has passed.
    pub auto_check_on_start: bool,
    /// The automatic check runs this long after Axion started (seconds), out of the way of
    /// loading the first page.
    pub startup_delay_seconds: u32,
    /// "Remind Me Later" silences the automatic notice for this long (hours).
    pub remind_later_hours: u32,
    /// Give up on a check after this many seconds.
    pub timeout_seconds: u32,
    /// Larger downloads (the AppImage) are refused (MB).
    pub max_download_mb: u32,
    /// When the last check ran (Unix seconds; 0: never). Written by Axion.
    pub last_checked: u64,
    /// A version the user chose to skip: no automatic notice for it. Written by Axion.
    pub skipped_version: String,
    /// The version the toast already announced (once per version). Written by Axion.
    pub notified_version: String,
    /// No automatic notice before this time (Unix seconds). Written by Axion.
    pub remind_until: u64,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            enabled: true,
            repository: String::new(),
            manifest_url: String::new(),
            api_url: "https://api.github.com".into(),
            channel: UpdateChannel::Stable,
            check_interval_hours: 24,
            auto_check_on_start: true,
            startup_delay_seconds: 8,
            remind_later_hours: 24,
            timeout_seconds: 10,
            max_download_mb: 1024,
            last_checked: 0,
            skipped_version: String::new(),
            notified_version: String::new(),
            remind_until: 0,
        }
    }
}

impl Updates {
    /// The repository in effect: the user's own, or [`DEFAULT_REPOSITORY`] when none is typed.
    pub fn effective_repository(&self) -> &str {
        self.effective_repository_or(DEFAULT_REPOSITORY)
    }

    /// [`Self::effective_repository`] with another default (tests, and a build whose default
    /// is empty).
    pub fn effective_repository_or<'a>(&'a self, default: &'a str) -> &'a str {
        let typed = self.repository.trim();
        if typed.is_empty() { default.trim() } else { typed }
    }

    /// Text for the settings field: empty while the default applies, so its placeholder shows.
    pub fn repository_field_text(&self) -> &str {
        let typed = self.repository.trim();
        if typed == DEFAULT_REPOSITORY.trim() { "" } else { typed }
    }

    /// Stores what the user typed. Blank, or the default itself, returns to the default and
    /// stores nothing.
    pub fn set_repository(&mut self, typed: &str) {
        let typed = typed.trim();
        self.repository = if typed == DEFAULT_REPOSITORY.trim() { String::new() } else { typed.into() };
    }

    /// Whether a repository (typed or default) or manifest address is configured.
    pub fn has_source(&self) -> bool {
        self.has_source_or(DEFAULT_REPOSITORY)
    }

    /// [`Self::has_source`] with another default repository.
    pub fn has_source_or(&self, default: &str) -> bool {
        !self.effective_repository_or(default).is_empty() || !self.manifest_url.trim().is_empty()
    }
}

/// Short glass notifications ("Link copied").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Toast {
    /// How long a toast stays before it springs away (seconds).
    pub duration_seconds: f32,
    /// How long a toast with a button ("Axion 0.2.0 is available" – Update) stays (seconds).
    pub action_duration_seconds: f32,
    /// Gap kept between a toast and a popover, menu or sheet it would otherwise overlap (px).
    pub clearance: f32,
}

impl Default for Toast {
    fn default() -> Self {
        Self { duration_seconds: 2.0, action_duration_seconds: 8.0, clearance: 12.0 }
    }
}

/// Pasting into pages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clipboard {
    /// Larger images and files on the clipboard are left out of a paste (MB). Only used where
    /// Axion hands the clipboard to pages itself (Wayland).
    pub max_paste_mb: u32,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self { max_paste_mb: 64 }
    }
}

/// How Chromium hands finished page frames to Axion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageFrames {
    /// Share them as GPU textures where that is known to work (NVIDIA's driver on Wayland),
    /// copy them through memory elsewhere.
    #[default]
    Auto,
    /// Always share GPU memory. On AMD's RADV driver this can crash the GPU driver.
    Gpu,
    /// Always copy frames through system memory.
    Software,
}

/// Which graphics card Axion draws with (and Chromium is pinned to).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuChoice {
    /// The discrete card if there is one (`AXION_GPU` can override).
    #[default]
    Auto,
    /// The integrated card (power saving).
    Low,
    /// The most powerful card (performance).
    High,
}

/// Hardware acceleration. Everything here is read when Axion starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hardware {
    /// Master switch. Off: Chromium renders pages, video and canvas in software and pages are
    /// painted through system memory. Axion's own interface still needs the GPU (wgpu).
    pub enabled: bool,
    /// How page frames reach Axion (`AXION_OSR=software|gpu` overrides it).
    pub page_frames: PageFrames,
    /// Which graphics card to use (`AXION_GPU=low` overrides it).
    pub gpu: GpuChoice,
    /// GPU rasterization (also what makes WebGL and canvas smooth) and zero-copy uploads.
    pub gpu_rasterization: bool,
}

impl Default for Hardware {
    fn default() -> Self {
        Self { enabled: true, page_frames: PageFrames::Auto, gpu: GpuChoice::Auto, gpu_rasterization: true }
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
    /// Tabs opened from links start in the background.
    pub open_links_in_background: bool,
    /// A middle click on a tab (in the tab bar, the sidebar or the tab overview) closes it.
    pub middle_click_closes_tab: bool,
    /// A tab opened from a tab of a tab group (a link, New Tab to the Right) joins that group.
    pub new_tab_joins_group: bool,
    /// Tab groups show as a coloured chip with their name in the compact tab bar and the
    /// separate tab row (off: only a colour marker on their tabs).
    pub group_chips_in_bar: bool,
    /// Tabs that play sound show a speaker (click to mute) in the tab bar and the sidebar.
    pub show_audio_indicator: bool,
    /// "Mute Site" is remembered: new tabs of a muted site start muted.
    pub remember_site_mute: bool,
    /// How many closed tabs the tab search (and Reopen Closed Tab) remembers.
    pub closed_tabs_remembered: u32,
    /// How many history entries the tab search lists at most.
    pub tab_search_history_results: u32,
    /// Close tabs not looked at for this many days (0: never).
    pub close_after_days: u32,
    /// How often memory use is measured and the tab rules are applied.
    pub check_interval_seconds: f32,
    /// A press on a tab turns into dragging it (to reorder, or to move it between normal and
    /// private tabs) once the pointer moved this far (logical px); shorter moves are clicks.
    pub drag_threshold: f32,
    /// A tab that is closed shrinks and fades away while the tabs beside it slide into its place
    /// (tab bar, sidebar and tab overview). Off: tabs vanish at once. Reduce Motion and flat
    /// mode keep only a short fade.
    pub close_animation: bool,
    /// Closing tabs with the pointer in the tab bar: the remaining tabs keep their widths until
    /// the pointer leaves the bar, so the next close button stays under the pointer.
    pub keep_widths_while_closing: bool,
    /// Dragging a tab out of the window into another app drops its link there (a text field gets
    /// the address).
    pub drag_out_links: bool,
    /// A dragged tab counts as leaving the window once the pointer is this far outside it
    /// (logical px).
    pub drag_out_distance: f32,
    /// A tab dragged out of the window carries a small card of the page (a preview with the
    /// site icon and title) beside the pointer, as drag image.
    pub drag_out_image: bool,
    /// Width of that card (logical px).
    pub drag_out_image_width: f32,
    /// How opaque the card is (0 invisible .. 1 solid).
    pub drag_out_image_opacity: f32,
    /// Pages draw at most this many frames per second; 0 follows the display's refresh rate.
    pub max_frame_rate: u32,
    /// Following the display (`max_frame_rate = 0`), pages draw at most this many frames per
    /// second (0: as many as the display shows). Each page frame costs Chromium a frame of its
    /// own GPU work and Axion a copy: a busy chat-app-like page (DOM updates, WebGL, canvas,
    /// video) kept Chromium's processes at 0.75 CPU cores at 60, 1.3 at 120 and 1.55 at 240
    /// frames per second, for motion that hardly looks smoother.
    pub max_frame_rate_cap: u32,
    /// GPU-shared page frames are copied this many milliseconds after Chromium hands them over
    /// (0: at once). Chromium reports a frame once its drawing is *submitted*, and the driver
    /// gives no fence to wait for, so a copy made at once can still hold the buffer's previous
    /// content (typed letters vanish for a frame, or until the next one comes). Longer is safer
    /// on a busy GPU, shorter adds less delay to every frame.
    pub gpu_frame_settle_ms: f32,
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
            open_links_in_background: true,
            middle_click_closes_tab: true,
            new_tab_joins_group: true,
            group_chips_in_bar: true,
            show_audio_indicator: true,
            remember_site_mute: true,
            closed_tabs_remembered: 25,
            tab_search_history_results: 8,
            close_after_days: 0,
            check_interval_seconds: 5.0,
            drag_threshold: 6.0,
            close_animation: true,
            keep_widths_while_closing: true,
            drag_out_links: true,
            drag_out_distance: 12.0,
            drag_out_image: true,
            drag_out_image_width: 200.0,
            drag_out_image_opacity: 0.9,
            max_frame_rate: 0,
            max_frame_rate_cap: 120,
            gpu_frame_settle_ms: 6.0,
        }
    }
}

/// The media player.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Media {
    /// Axion's player takes over when a video goes fullscreen (on every site except the
    /// trusted ones below). Inline, pages always keep their own controls.
    #[serde(alias = "own_fullscreen_controls")]
    pub fullscreen_player: bool,
    /// Sites whose own player is kept, also in fullscreen. Just the site's name, like
    /// "youtube.com" — its subdomains (www., m., …) count too. Edit in Settings › Media.
    pub trusted_player_sites: Vec<String>,
    /// Sites whose `<video>`/`<audio>` elements are not media to play or list: calls and chats
    /// (Discord voice/video, …) where the elements carry live call streams. The bridge does not
    /// see them, so no player, no Now Playing entry, no Playlist entry. The site's name, like
    /// "discord.com"; subdomains count too. Streams the page gets from the camera, microphone
    /// or a peer connection (`srcObject`) are skipped on every site.
    pub ignored_media_sites: Vec<String>,
    /// Older settings (Axion's controls inline), no longer used; accepted so old files load.
    #[serde(skip_serializing)]
    own_controls: Option<bool>,
    #[serde(skip_serializing)]
    replace_site_players: Option<bool>,
    /// Hide the controls of a playing video after this long without pointer movement.
    pub hide_controls_after_seconds: f32,
    /// How far the skip buttons and arrow keys jump.
    pub skip_seconds: f32,
    /// Volume step of the arrow keys (0–1).
    pub volume_step: f32,
    /// The speed button cycles through these.
    pub playback_speeds: Vec<f32>,
    /// Decode video on the GPU (VA-API). Needs `hardware.enabled`; shown in Settings › Media and
    /// Settings › Hardware (applies after a restart).
    pub hardware_decoding: bool,
    /// Show what plays in the desktop's media controls and take the media keys (MPRIS).
    pub desktop_integration: bool,
    /// The Picture in Picture window can only be resized with the video's aspect, so the whole
    /// video is always visible (no black bars, nothing cropped). Off: free resizing.
    #[serde(alias = "pip_keep_aspect")]
    pub pip_lock_aspect: bool,
    /// Older settings (the PiP window used to snap back to the aspect after a delay), no longer
    /// used: the window now follows the pointer continuously. Accepted so old files load.
    #[serde(skip_serializing)]
    pip_aspect_settle_seconds: Option<f32>,
    #[serde(skip_serializing)]
    pip_aspect_live_seconds: Option<f32>,
    /// A PiP resize the window manager started on its own (no edge of ours grabbed, e.g.
    /// Alt + right drag) counts as over when no new size arrived for this long (seconds).
    pub pip_resize_idle_seconds: f32,
    /// Width (logical px) of a fresh PiP window (the video fits a square of this side) when no
    /// size is remembered.
    pub pip_default_width: f32,
    /// Widest the PiP window can be made (logical px).
    pub pip_max_width: f32,
    /// The PiP window never grows beyond this share of its monitor (0.1-1), either way.
    pub pip_max_screen_fraction: f32,
    /// Size of the PiP window's controls (buttons, icons and the progress bar), as a multiple
    /// of the base size. Shrinks by itself in a small window so everything still fits.
    pub pip_control_scale: f32,
    /// Width (logical px) of the PiP window's edge band that resizes it. Controls stay clear
    /// of it, so the progress bar can be grabbed right at the bottom.
    pub pip_resize_border: f32,
    /// The smallest size (logical px) of any PiP control, however small the window is.
    pub pip_min_target: f32,
    /// Narrowest the PiP window can be made (logical px).
    pub pip_min_width: f32,
    /// Thickness (logical px) of the progress bar's track in the fullscreen player, the
    /// Playlist player and Picture in Picture (at rest; it grows while it is dragged).
    pub progress_thickness: f32,
    /// Height (logical px) of the area that grabs the progress bar: the track alone would be
    /// too thin to hit comfortably. At least 24.
    pub progress_hit_height: f32,
    /// Side (logical px) of the artwork tile of the sidebar's Now Playing card.
    pub now_playing_artwork_size: f32,
    /// Gap (logical px) between the Now Playing card and the account button below it (and the
    /// account menu, which pushes the card up as it opens).
    pub now_playing_gap: f32,
    /// Fetch the artwork the page offers (media session artwork, poster, preview image) for the
    /// Now Playing card; otherwise (and for private tabs) it shows the site's icon.
    pub now_playing_artwork: bool,
    /// Scroll a title that is too long for the Now Playing card (otherwise it is cut off).
    pub now_playing_marquee: bool,
    /// Scroll speed of that title (logical px per second).
    pub now_playing_marquee_speed: f32,
    /// Show a turning ring on the players while a video loads or buffers.
    pub loading_indicator: bool,
    /// Seconds for one turn of that ring.
    pub loading_spin_seconds: f32,
}

impl Media {
    /// Adds a trusted site (a host like "example.com"; "https://www.example.com/x" works too).
    /// Returns false if it is empty or already listed.
    pub fn add_trusted_site(&mut self, input: &str) -> bool {
        let site = trusted_site_name(input);
        if site.is_empty() || self.trusted_player_sites.contains(&site) {
            return false;
        }
        self.trusted_player_sites.push(site);
        true
    }
}

/// "https://www.Example.com/watch?v=1" → "example.com".
fn trusted_site_name(input: &str) -> String {
    let s = input.trim().to_ascii_lowercase();
    let s = s.split_once("://").map_or(s.as_str(), |(_, rest)| rest);
    let host = s.split(['/', '?', '#', ':']).next().unwrap_or_default();
    host.strip_prefix("www.").unwrap_or(host).to_owned()
}

impl Default for Media {
    fn default() -> Self {
        Self {
            fullscreen_player: true,
            trusted_player_sites: [
                "youtube.com",
                "youtube-nocookie.com",
                "twitch.tv",
                "vimeo.com",
                "dailymotion.com",
                "netflix.com",
                "disneyplus.com",
                "primevideo.com",
                "crunchyroll.com",
                "spotify.com",
                "soundcloud.com",
            ]
            .map(String::from)
            .to_vec(),
            ignored_media_sites: ["discord.com", "discordapp.com", "discord.gg", "discord.media"]
                .map(String::from)
                .to_vec(),
            own_controls: None,
            replace_site_players: None,
            hide_controls_after_seconds: 2.5,
            skip_seconds: 10.0,
            volume_step: 0.1,
            playback_speeds: vec![1.0, 1.25, 1.5, 2.0, 0.5, 0.75],
            hardware_decoding: true,
            desktop_integration: true,
            pip_lock_aspect: true,
            pip_aspect_settle_seconds: None,
            pip_aspect_live_seconds: None,
            pip_resize_idle_seconds: 0.5,
            pip_default_width: 480.0,
            pip_max_width: 1280.0,
            pip_max_screen_fraction: 0.9,
            pip_control_scale: 1.3,
            pip_resize_border: 6.0,
            pip_min_target: 36.0,
            pip_min_width: 272.0,
            progress_thickness: 8.0,
            progress_hit_height: 28.0,
            now_playing_artwork_size: 52.0,
            now_playing_gap: 10.0,
            now_playing_artwork: true,
            now_playing_marquee: true,
            now_playing_marquee_speed: 28.0,
            loading_indicator: true,
            loading_spin_seconds: 0.9,
        }
    }
}

/// Axion Playlist: videos saved for later, optionally downloaded for offline use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Playlist {
    /// Show the Add to Playlist button in the address field on pages with a video.
    pub quick_action: bool,
    /// Download a video for offline use as soon as it is added (when it is a plain file).
    pub auto_download: bool,
    /// Folder for downloaded videos; empty means `<profile>/playlist` (the account's own data).
    /// Changing it only affects new downloads.
    pub directory: String,
    /// Removing an item also deletes the file it downloaded.
    pub delete_files_on_remove: bool,
    /// The most items the Playlist keeps; adding more is refused with a message.
    pub max_items: u32,
    /// Titles are cut to this many characters.
    pub max_title_chars: u32,
    /// Thumbnails are scaled down to at most this many pixels on the long side.
    pub thumbnail_max_px: u32,
    /// How long Axion waits for a thumbnail from the page before giving up (seconds).
    pub thumbnail_wait_seconds: f32,
    /// An item counts as watched once this much of it (0–1) has played.
    pub watched_fraction: f32,
    /// The playback position is saved at most this often while playing (seconds).
    pub position_save_seconds: f32,
    /// Positions before this (seconds) are not worth resuming from.
    pub min_resume_seconds: f32,
    /// Resuming starts this many seconds before where playback stopped.
    pub resume_rewind_seconds: f32,
    /// Thumbnails decoded onto the GPU per frame while the page fills in.
    pub thumbnails_per_frame: u32,
    /// Name of the list new videos go to (shown as the Playlist page's title). The old
    /// default "Play Later" counts as "Playlist".
    pub list_name: String,
    /// The sleep timer's choices (minutes); "Off" and "End of video" are always offered.
    pub sleep_timer_minutes: Vec<u32>,
    /// A video that was on its page for this long (seconds) gets its picture taken for the
    /// thumbnail when none comes with the page.
    pub thumbnail_frame_after_seconds: f32,
    /// Obsolete: the Playlist plays only the video source, never the page (kept so old
    /// config files still load).
    pub forward_clicks: bool,
    /// An item saved without its video source gets it found by loading its page hidden; this
    /// is how long that may take before the player says it couldn't (seconds).
    pub resolve_timeout_seconds: f32,
    /// A page whose videos look unplayable outside it (assembled by the page, DASH) is given
    /// up on after this long (seconds), since the network may still show a source.
    pub resolve_settle_seconds: f32,
    /// While finding a source, the page's player is clicked this often (seconds): site
    /// players load their video only after a click.
    pub resolve_click_seconds: f32,
    /// How many media responses (manifests, video files) a page's network traffic is
    /// remembered by, for capturing the source behind `blob:` players.
    pub sniff_limit: u32,
    /// Opening the Playlist page starts the topmost video (unless one is playing already).
    pub autoplay_on_open: bool,
    /// "Continue watching": the most recently played unfinished video stays on top of the
    /// list, whatever the sort says.
    pub continue_on_top: bool,
    /// When a video ends, the next one in the list starts.
    pub autoadvance: bool,
    /// How the list is ordered (below the continue-watching item).
    pub sort: PlaylistSort,
    /// The volume (0–1) every Playlist video plays at: the fullscreen player, the Playlist
    /// page's player and Picture in Picture share it, and every volume control there sets it.
    pub volume: f32,
    /// Playlist videos are muted (the volume above is kept for when they are not).
    pub muted: bool,
}

/// The font family of Reader view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderFont {
    /// A book face.
    #[default]
    Serif,
    /// The system's UI face.
    Sans,
    /// A fixed-width face.
    Mono,
}

/// The colours of Reader view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderTheme {
    /// Light or dark with Axion's own theme.
    #[default]
    Auto,
    Light,
    Sepia,
    Dark,
}

/// Reader view: the article of a page on a clean sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Reader {
    /// Offer Reader View in the page menu (on pages that have an article). The key kept its
    /// name from when this was a button in the address field.
    pub button: bool,
    pub font: ReaderFont,
    /// Text size in CSS pixels.
    pub font_size: f32,
    /// Line height as a multiple of the text size.
    pub line_height: f32,
    /// Width of the text column in CSS pixels.
    pub width: f32,
    pub theme: ReaderTheme,
    /// The Aa menu's choices are remembered for each site (otherwise they are the choices for
    /// every site, saved here).
    pub remember_per_site: bool,
    /// A page counts as readable when its main text has at least this many characters.
    pub min_chars: u32,
    /// How long after a page loaded it is checked for an article (milliseconds); pages that
    /// build their text with scripts need a moment.
    pub detect_delay_ms: u32,
    /// The Aa menu's text size range and step (CSS pixels).
    pub min_font_size: f32,
    pub max_font_size: f32,
    pub font_step: f32,
    /// The Aa menu's column width range and step (CSS pixels).
    pub min_width: f32,
    pub max_width: f32,
    pub width_step: f32,
}

impl Default for Reader {
    fn default() -> Self {
        Self {
            button: true,
            font: ReaderFont::Serif,
            font_size: 20.0,
            line_height: 1.6,
            width: 680.0,
            theme: ReaderTheme::Auto,
            remember_per_site: true,
            min_chars: 600,
            detect_delay_ms: 700,
            min_font_size: 14.0,
            max_font_size: 36.0,
            font_step: 2.0,
            min_width: 480.0,
            max_width: 1000.0,
            width_step: 80.0,
        }
    }
}

/// Where translations are computed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationBackend {
    /// On this computer, with Mozilla's Bergamot models (downloaded once, with your consent).
    #[default]
    Local,
    /// A LibreTranslate-compatible server you run or trust (`translation.endpoint_url`):
    /// page text is sent there.
    Endpoint,
}

/// Page translation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Translation {
    /// Offer to translate pages written in another language than `target_language`.
    pub offer: bool,
    /// The offer opens by itself once such a page has loaded (otherwise only the Translate
    /// button appears in the address field).
    pub prompt: bool,
    /// ISO 639-1 code pages are translated into; empty: the interface language.
    pub target_language: String,
    pub backend: TranslationBackend,
    /// LibreTranslate-compatible server (`https://host/translate`), used when `backend` is
    /// `endpoint`. Empty: none. Page text is sent to it.
    pub endpoint_url: String,
    /// The server's API key (empty: none).
    pub endpoint_api_key: String,
    /// Sites (and their subdomains) that are never offered for translation.
    pub never_sites: Vec<String>,
    /// Languages (ISO 639-1) that are never offered for translation, besides the target.
    pub known_languages: Vec<String>,
    /// Where the model list comes from (Mozilla Remote Settings).
    pub models_records_url: String,
    /// Where model files are downloaded from.
    pub models_attachments_url: String,
    /// Translated in one go: at most this many characters and text pieces.
    pub batch_chars: u32,
    pub batch_pieces: u32,
    /// Text that appears on a translated page later (feeds, comments) is translated too.
    pub translate_changes: bool,
    /// A page without a declared language is guessed from at least this many characters.
    pub detect_min_chars: u32,
    /// How long an endpoint may take for one request (seconds).
    pub endpoint_timeout_seconds: u32,
    /// The hidden page that runs the local engine is closed after this many seconds without
    /// work, which frees the memory of the models.
    pub engine_idle_seconds: u32,
}

impl Default for Translation {
    fn default() -> Self {
        Self {
            offer: true,
            prompt: true,
            target_language: String::new(),
            backend: TranslationBackend::Local,
            endpoint_url: String::new(),
            endpoint_api_key: String::new(),
            never_sites: Vec::new(),
            known_languages: Vec::new(),
            models_records_url: "https://firefox.settings.services.mozilla.com/v1/buckets/main/collections"
                .to_owned(),
            models_attachments_url: "https://firefox-settings-attachments.cdn.mozilla.net/".to_owned(),
            batch_chars: 4000,
            batch_pieces: 60,
            translate_changes: true,
            detect_min_chars: 120,
            endpoint_timeout_seconds: 30,
            engine_idle_seconds: 120,
        }
    }
}

/// How the Playlist page orders its videos. More modes can be added later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistSort {
    /// Latest added first.
    #[default]
    Newest,
    /// Oldest added first.
    Oldest,
    /// The order the user arranged with move up / down in Edit mode.
    Manual,
}

impl Default for Playlist {
    fn default() -> Self {
        Self {
            quick_action: true,
            auto_download: false,
            directory: String::new(),
            delete_files_on_remove: true,
            max_items: 1000,
            max_title_chars: 200,
            thumbnail_max_px: 480,
            thumbnail_wait_seconds: 8.0,
            watched_fraction: 0.9,
            position_save_seconds: 5.0,
            min_resume_seconds: 5.0,
            resume_rewind_seconds: 2.0,
            thumbnails_per_frame: 3,
            list_name: "Playlist".to_owned(),
            sleep_timer_minutes: vec![15, 30, 45, 60],
            thumbnail_frame_after_seconds: 1.0,
            forward_clicks: true,
            resolve_timeout_seconds: 25.0,
            resolve_settle_seconds: 6.0,
            resolve_click_seconds: 2.0,
            sniff_limit: 40,
            autoplay_on_open: true,
            continue_on_top: true,
            autoadvance: true,
            sort: PlaylistSort::Newest,
            volume: 1.0,
            muted: false,
        }
    }
}

/// Flat mode: the gap around the toolbar's rows (logical px), at most.
pub const FLAT_MARGIN: f32 = 8.0;

impl Config {
    /// The layout as used: in flat mode the chrome is one conventional toolbar surface with the
    /// page filling the area under (and beside) it: small margins, no page margin, square page.
    /// Liquid mode uses `layout` as configured.
    pub fn effective_layout(&self) -> Layout {
        let mut layout = self.layout.clone();
        if self.glass.is_flat() {
            layout.margin = layout.margin.min(FLAT_MARGIN);
            layout.page_margin = 0.0;
            layout.page_corner_radius = 0.0;
        }
        layout
    }

    /// The full default configuration as TOML, for documentation and the settings UI.
    pub fn default_toml() -> String {
        toml::to_string_pretty(&Self::default()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_chrome_is_neutral_by_default() {
        let c = Config::default();
        let a = &c.appearance;
        assert_eq!(a.tint_mode, TintMode::Neutral);
        assert!(!a.tint_from_page && !a.adapt_to_page);
        assert_eq!(a.theme, ThemeMode::Auto);
        // Apple's system greys, never pure white or black.
        assert_eq!(c.colors.light.ground, Color::rgb(0xF2F2F7));
        assert_eq!(c.colors.dark.ground, Color::rgb(0x1C1C1E));
        assert_eq!(c.colors.dark.surface, Color::rgb(0x2C2C2E));
        for tint in [c.glass.light.tint, c.glass.dark.tint, c.colors.light.surface] {
            assert!(tint != Color::rgb(0xFFFFFF) && tint != Color::rgb(0x000000));
        }
        assert_eq!(c.glass.dark.tint, Color::rgb(DARK_GLASS_TINT));
    }

    use super::*;

    #[test]
    fn empty_file_is_default() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn removed_layout_keys_still_load() {
        let c: Config = toml::from_str("[layout]\nhide_bar_on_scroll = true").unwrap();
        assert_eq!(c.layout.tab_bar, TabBarStyle::Separate);
        assert!(!toml::to_string(&c).unwrap().contains("hide_bar_on_scroll"));
    }

    #[test]
    fn top_sites_defaults_and_old_pages_key() {
        let d = Config::default().home;
        assert_eq!((d.top_sites_count, d.top_sites_window_days), (5, 90));
        let c: Config = toml::from_str("[home]\ntop_sites_pages = 3").unwrap();
        assert_eq!(c.home.top_sites_count, 5);
        assert!(!toml::to_string(&c).unwrap().contains("top_sites_pages"));
    }

    #[test]
    fn old_background_toggle_still_loads() {
        let c: Config = toml::from_str("[home]\nshow_background_image = true").unwrap();
        assert!(c.home.legacy_background_shown());
        assert!(c.home.background.is_empty());
        assert!(!toml::to_string(&c).unwrap().contains("show_background_image"));
        assert!(!Config::default().home.legacy_background_shown());
    }

    #[test]
    fn media_settings_old_and_new() {
        // Files from before the fullscreen-only player still load.
        let c: Config = toml::from_str(
            "[media]\nown_controls = false\nreplace_site_players = true\nown_fullscreen_controls = false",
        )
        .unwrap();
        assert!(!c.media.fullscreen_player);
        let mut media = Media::default();
        assert!(media.add_trusted_site("https://www.Example.com/watch?v=1"));
        assert!(media.trusted_player_sites.contains(&"example.com".to_owned()));
        assert!(!media.add_trusted_site("example.com"));
        assert!(!media.add_trusted_site("  "));
        assert!(!toml::to_string(&media).unwrap().contains("own_controls"));
    }

    #[test]
    fn pip_old_snap_keys_still_load() {
        let c: Config =
            toml::from_str("[media]\npip_aspect_settle_seconds = 0.3\npip_aspect_live_seconds = 0.06")
                .unwrap();
        assert_eq!(c.media.pip_max_width, Media::default().pip_max_width);
        assert!(!toml::to_string(&c.media).unwrap().contains("pip_aspect_"));
        assert!(Config::default().window.restore_pip_size);
    }

    #[test]
    fn playlist_defaults_and_keys() {
        let p = Playlist::default();
        assert!(p.quick_action && !p.auto_download && p.delete_files_on_remove);
        assert_eq!(p.list_name, "Playlist");
        let c: Config = toml::from_str("[playlist]\nauto_download = true").unwrap();
        assert!(c.playlist.auto_download);
        assert_eq!(c.playlist.max_items, p.max_items);
    }

    #[test]
    fn playlist_sort_and_continue_keys_round_trip() {
        let p = Playlist::default();
        assert!(p.autoplay_on_open && p.continue_on_top && p.autoadvance);
        assert_eq!(p.sort, PlaylistSort::Newest);
        let mut c = Config::default();
        c.playlist.sort = PlaylistSort::Oldest;
        let text = toml::to_string(&c).unwrap();
        assert!(text.contains("sort = \"oldest\""));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.playlist.sort, PlaylistSort::Oldest);
        let c: Config = toml::from_str("[playlist]\nsort = \"manual\"\ncontinue_on_top = false").unwrap();
        assert_eq!(c.playlist.sort, PlaylistSort::Manual);
        assert!(!c.playlist.continue_on_top);
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

#[cfg(test)]
mod private_prompt_tests {
    use super::*;

    #[test]
    fn the_unlock_card_opens_by_itself_by_default() {
        assert!(Privacy::default().prompt_unlock_on_open);
    }
}

#[cfg(test)]
mod glass_style_tests {
    use super::*;

    #[test]
    fn liquid_is_the_default_and_the_key_round_trips() {
        assert_eq!(Config::default().glass.style, GlassStyle::Liquid);
        assert!(!Config::default().glass.is_flat());
        let mut c = Config::default();
        c.glass.style = GlassStyle::Flat;
        let text = toml::to_string(&c).unwrap();
        assert!(text.contains("style = \"flat\""), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.glass.style, GlassStyle::Flat);
        let c: Config = toml::from_str("[glass]\nstyle = \"liquid\"").unwrap();
        assert_eq!(c.glass.style, GlassStyle::Liquid);
        assert!(toml::from_str::<Config>("[glass]\nstyle = \"jelly\"").is_err());
    }

    #[test]
    fn flat_mode_switches_off_merging_squash_bridges_and_bounce() {
        let mut glass = Glass::default();
        assert_eq!(glass.merge_distance(), glass.merge_distance);
        assert_eq!(glass.squash(), glass.squash);
        assert_eq!(glass.popover_bridge_fillet(), glass.popover_bridge_fillet);
        assert_eq!(glass.damping(0.6), 0.6, "liquid keeps its bounce");
        glass.style = GlassStyle::Flat;
        assert_eq!(glass.merge_distance(), 0.0);
        assert_eq!(glass.squash(), 0.0);
        assert_eq!(glass.popover_bridge_fillet(), 0.0);
        assert_eq!(glass.damping(0.6), FLAT_DAMPING);
        assert_eq!(glass.damping(1.4), 1.4, "already over-damped springs stay as they are");
    }

    #[test]
    fn flat_mode_lays_the_chrome_out_as_one_toolbar_with_a_full_bleed_page() {
        let mut c = Config::default();
        assert_eq!(c.effective_layout(), c.layout, "liquid uses the layout as configured");
        c.glass.style = GlassStyle::Flat;
        let l = c.effective_layout();
        assert!(l.margin <= FLAT_MARGIN);
        assert_eq!((l.page_margin, l.page_corner_radius), (0.0, 0.0));
        assert_eq!(l.sidebar_width, c.layout.sidebar_width, "everything else is as configured");
    }

    #[test]
    fn reduce_motion_also_stops_the_squash() {
        let glass = Glass { reduce_motion: true, ..Glass::default() };
        assert_eq!(glass.squash(), 0.0);
    }
}
