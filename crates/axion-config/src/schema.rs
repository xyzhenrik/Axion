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
    pub shields: Shields,
    pub tabs: Tabs,
    pub media: Media,
    pub downloads: Downloads,
    pub data: Data,
    pub passwords: Passwords,
    pub find: Find,
    pub clipboard: Clipboard,
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
}

/// The homepage Axion used before it had its own start page; configs that still carry it (or
/// none at all) get the start page.
const OLD_DEFAULT_HOMEPAGE: &str = "https://duckduckgo.com/";

impl General {
    /// An empty homepage or the old default becomes Axion's start page.
    pub fn upgrade_homepage(&mut self) {
        if self.homepage.trim().is_empty() || self.homepage == OLD_DEFAULT_HOMEPAGE {
            self.homepage = START_PAGE_URL.into();
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self { homepage: START_PAGE_URL.into(), search_url: "https://duckduckgo.com/?q={}".into() }
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
    /// Tiles per row, rows per page, and pages (with dots to switch) at most.
    pub top_sites_columns: u32,
    pub top_sites_rows: u32,
    pub top_sites_pages: u32,
    /// Trackers & ads blocked, bandwidth and time saved.
    pub show_stats: bool,
    /// The selected background image: a file name in the account's backgrounds directory
    /// (`<profile>/home/backgrounds/`, where added images are copied). Empty: none, the
    /// background is plain white or black with the theme.
    pub background: String,
    /// At most this many backgrounds are kept (adding more is refused).
    pub max_backgrounds: u32,
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
            top_sites_columns: 6,
            top_sites_rows: 2,
            top_sites_pages: 3,
            show_stats: true,
            background: String::new(),
            max_backgrounds: 24,
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
    /// Give the window around the page the page's own background colour (like Safari): its
    /// `theme-color`, else the colour most of the page shows, measured once per page.
    pub tint_from_page: bool,
    /// The page's colour is measured this long after it finished loading (ms), so late
    /// styles and images are in.
    pub page_color_delay_ms: f32,
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
            page_color_delay_ms: 350.0,
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
    /// Smooth-union radius in logical pixels of the broad glass bridge that joins a popover
    /// or menu to the bar or button it flows out of: how wide the concave fillets at the
    /// junction's ends are (at most `merge_distance` has no extra effect).
    pub popover_bridge_fillet: f32,
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
            popover_bridge_fillet: 30.0,
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
    /// Shields icon when protection is active (unset: the accent colour).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield: Option<Color>,
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
    /// Always float the sidebar over the page as a rounded glass card (it never pushes the
    /// page), whatever the window size.
    pub float_sidebar: bool,
    /// Corner radius of a floating sidebar card.
    pub sidebar_corner_radius: f32,
    /// From this window width up, the sidebar docks (with `sidebar = "auto"`).
    pub wide_from: f32,
    /// Below this window width, tabs collapse into a single compact capsule.
    pub narrow_below: f32,
    /// Preferred width of a tab in the separate tab bar and of the other tabs' segments in the
    /// compact one; all tabs share it, and they shrink equally when they don't fit.
    pub tab_width: f32,
    /// Tabs shrink no further than this (logical px); beyond it they overflow.
    pub tab_min_width: f32,
    /// Width of a pinned tab (favicon only).
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
    /// Address bar row, and the tabs in a row below it.
    Separate,
    /// One row: the tabs, the current one holding the address.
    #[default]
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
            float_sidebar: false,
            sidebar_corner_radius: 26.0,
            wide_from: 1280.0,
            narrow_below: 800.0,
            tab_width: 260.0,
            tab_min_width: 72.0,
            pinned_tab_width: 44.0,
            bookmarks_bar: BookmarksBar::Always,
            page_corner_radius: 10.0,
            page_margin: 8.0,
            tab_bar: TabBarStyle::Compact,
            home_button: true,
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
    /// Older setting (locking again after time in the background), no longer used: an unlock
    /// lasts for the session. Accepted so old files load.
    #[serde(skip_serializing)]
    lock_after_seconds: Option<f32>,
}

impl Default for Privacy {
    fn default() -> Self {
        Self { lock_private_tabs: true, lock_after_seconds: None }
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
}

impl Default for Passwords {
    fn default() -> Self {
        Self {
            offer_to_save: true,
            autofill: true,
            auto_lock_minutes: None,
            clear_clipboard_seconds: 60.0,
            generator_length: 20,
            generator_symbols: true,
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
    /// Close tabs not looked at for this many days (0: never).
    pub close_after_days: u32,
    /// How often memory use is measured and the tab rules are applied.
    pub check_interval_seconds: f32,
    /// A press on a tab turns into dragging it (to reorder, or to move it between normal and
    /// private tabs) once the pointer moved this far (logical px); shorter moves are clicks.
    pub drag_threshold: f32,
    /// Pages draw at most this many frames per second; 0 follows the display's refresh rate.
    pub max_frame_rate: u32,
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
            close_after_days: 0,
            check_interval_seconds: 5.0,
            drag_threshold: 6.0,
            max_frame_rate: 0,
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
    /// Decode video on the GPU (VA-API).
    pub hardware_decoding: bool,
    /// Show what plays in the desktop's media controls and take the media keys (MPRIS).
    pub desktop_integration: bool,
    /// The Picture in Picture window keeps the video's aspect when it is resized (so the
    /// video fills it without black bars).
    pub pip_keep_aspect: bool,
    /// How long after the last resize step of the PiP window it snaps to the video's aspect
    /// (window managers resize in many small steps).
    pub pip_aspect_settle_seconds: f32,
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
            own_controls: None,
            replace_site_players: None,
            hide_controls_after_seconds: 2.5,
            skip_seconds: 10.0,
            volume_step: 0.1,
            playback_speeds: vec![1.0, 1.25, 1.5, 2.0, 0.5, 0.75],
            hardware_decoding: true,
            desktop_integration: true,
            pip_keep_aspect: true,
            pip_aspect_settle_seconds: 0.3,
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
    fn removed_layout_keys_still_load() {
        let c: Config = toml::from_str("[layout]\nhide_bar_on_scroll = true").unwrap();
        assert_eq!(c.layout.tab_bar, TabBarStyle::Compact);
        assert!(!toml::to_string(&c).unwrap().contains("hide_bar_on_scroll"));
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
