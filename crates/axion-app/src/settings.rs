//! The settings sheet (design: "Settings"): a large glass sheet over the dimmed browser with
//! categories on the left. Every change is written to `config.toml` at once.

use std::path::PathBuf;

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{
    ACCENT_SWATCHES, Action as KeyAction, BookmarksBar, ClearRange, Color, Config, GlassStyle, GpuChoice,
    Keymap, PageFrames, PermissionDefault, PlaylistSort, ReaderFont, ReaderTheme, RemoveDownloads,
    ShieldLevel, TabBarStyle, ThemeMode, TintMode, TranslationBackend, UiLanguage,
};
use axion_i18n::{tr, tr_fmt, tr_mark, tr_n};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;
use zeroize::Zeroizing;

use axion_engine::PermissionKind;

use crate::home::Image;
use crate::morph::{Origin, SheetMorph};
use crate::passwords::MIN_MASTER_LENGTH;
use crate::permissions::Group;
use crate::update::{AboutInfo, AboutState, InstallMode};

mod import;
mod search;

pub use import::{ImportAction, ImportRequest, ImportSource, ImportStatus, ImportView};

const ACTION_BASE: u32 = 8000;
const SHEET_MAX: Vec2 = Vec2::new(920.0, 660.0);
const SHEET_MARGIN: f32 = 32.0;
const SHEET_RADIUS: f32 = 30.0;
const NAV_WIDTH: f32 = 230.0;
const ROW_HEIGHT: f32 = 46.0;
/// How many lines a row's label and a note may wrap onto before they end in an ellipsis.
const LABEL_LINES: usize = 2;
const NOTE_LINES: usize = 4;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// Width of the value label beside a slider.
const VALUE_WIDTH: f32 = 150.0;
const SLIDER_WIDTH: f32 = 180.0;
/// Grouped list (Safari's settings): rounded cards of rows with hairline separators.
const GROUP_RADIUS: f32 = 14.0;
const GROUP_INSET: f32 = 16.0;
const GROUP_GAP: f32 = 22.0;
const BACKDROP_DIM: f32 = 0.28;
/// The dot on the About entry (and the account button) while an update is waiting.
const UPDATE_DOT: Color = Color::rgb(0xFF3B30);
/// The release notes box: its height, and the app icon's size and corner radius.
const NOTES_HEIGHT: f32 = 168.0;
const ABOUT_ICON: f32 = 64.0;
const ABOUT_ICON_RADIUS: f32 = 15.0;
/// A release note line wraps onto at most this many lines.
const NOTES_LINE_WRAP: usize = 8;

pub const SEARCH_ENGINES: [(&str, &str); 5] = [
    ("DuckDuckGo", "https://duckduckgo.com/?q={}"),
    ("Google", "https://www.google.com/search?q={}"),
    ("Brave", "https://search.brave.com/search?q={}"),
    ("Startpage", "https://www.startpage.com/sp/search?query={}"),
    ("Ecosia", "https://www.ecosia.org/search?q={}"),
];

/// Accent colours from the design system.
const ACCENTS: [Color; 6] = ACCENT_SWATCHES;

/// The start page's backgrounds Settings can list (each has a select and a remove action).
pub const BACKGROUND_SLOTS: usize = 30;
/// A background card in the picker (16:10, like a window) and its selection ring.
const BACKGROUND_CARD: Vec2 = Vec2::new(112.0, 70.0);
const BACKGROUND_CARD_RADIUS: f32 = 11.0;
const BACKGROUND_RING: f32 = 3.0;
/// The large preview of the selected background above the cards.
pub const BACKGROUND_PREVIEW: Vec2 = Vec2::new(336.0, 210.0);
const BACKGROUND_PREVIEW_RADIUS: f32 = 16.0;

/// A background as the picker shows it.
#[derive(Clone)]
pub struct BackgroundCard {
    /// Its file name in the backgrounds directory.
    pub name: String,
    /// Its thumbnail (none while it is made, or for formats without one).
    pub image: Option<Image>,
    pub loading: bool,
}
/// The avatar on the Account page.
const AVATAR_SIZE: f32 = 52.0;
/// The small preview next to the photo buttons.
const PHOTO_PREVIEW: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Account,
    General,
    Appearance,
    LiquidGlass,
    Tabs,
    Media,
    Reading,
    Keyboard,
    Shields,
    Privacy,
    Data,
    Passwords,
    Hardware,
    About,
}

impl Category {
    pub const ALL: [Category; 14] = [
        Category::Account,
        Category::General,
        Category::Appearance,
        Category::LiquidGlass,
        Category::Tabs,
        Category::Media,
        Category::Reading,
        Category::Keyboard,
        Category::Shields,
        Category::Privacy,
        Category::Data,
        Category::Passwords,
        Category::Hardware,
        Category::About,
    ];

    /// The page's name, translated.
    fn title(self) -> &'static str {
        tr(self.source_title())
    }

    /// The page's English name (what search also matches).
    fn source_title(self) -> &'static str {
        match self {
            Category::Account => tr_mark("Account"),
            Category::General => tr_mark("General"),
            Category::Appearance => tr_mark("Appearance"),
            Category::LiquidGlass => tr_mark("Liquid Glass"),
            Category::Tabs => tr_mark("Tabs & Memory"),
            Category::Media => tr_mark("Media"),
            Category::Reading => tr_mark("Reading & Translation"),
            Category::Keyboard => tr_mark("Keyboard"),
            Category::Shields => tr_mark("Shields"),
            Category::Privacy => tr_mark("Privacy"),
            Category::Data => tr_mark("Data & Downloads"),
            Category::Passwords => tr_mark("Passwords"),
            Category::Hardware => tr_mark("Hardware"),
            Category::About => tr_mark("About & Updates"),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Category::Account => Icon::Person,
            Category::General => Icon::Settings,
            Category::Appearance => Icon::Sidebar,
            Category::LiquidGlass => Icon::Overview,
            Category::Tabs => Icon::TabGroup,
            Category::Media => Icon::Media,
            Category::Reading => Icon::TextSize,
            Category::Keyboard => Icon::Menu,
            Category::Shields => Icon::Shield,
            Category::Privacy => Icon::Lock,
            Category::Data => Icon::Download,
            Category::Passwords => Icon::Key,
            Category::Hardware => Icon::Gauge,
            Category::About => Icon::Info,
        }
    }
}

/// A setting edited with a toggle or slider; sliders map 0–1 onto the setting's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    HomeClock,
    HomeGreeting,
    HomeSearch,
    HomeTopSites,
    HomeStats,
    HomeLive,
    HomeGlassGround,
    HomeLiveIgnoreReduceMotion,
    DevTools,
    HomeDim,
    HomeBlur,
    TintFromPage,
    AdaptToPage,
    TextScale,
    PageCorner,
    FloatSidebar,
    ReduceTransparency,
    ReduceMotion,
    Refraction,
    Blur,
    Dispersion,
    Squash,
    DragStretch,
    MergeDistance,
    LockPrivateTabs,
    HideAdElements,
    BlockPopups,
    BlockRedirects,
    StripTracking,
    ThirdPartyCookies,
    FreezeAfter,
    DiscardAfter,
    LimitMemory,
    MemoryBudget,
    KeepMediaAwake,
    RestoreSession,
    OpenInBackground,
    CloseAnimation,
    KeepTabWidths,
    MiddleClickClosesTab,
    NewTabJoinsGroup,
    GroupChipsInBar,
    ShowAudioIndicator,
    RememberSiteMute,
    DragOutLinks,
    DragOutImage,
    HomeButton,
    OwnFullscreenControls,
    HideControlsAfter,
    HardwareDecoding,
    HardwareEnabled,
    GpuRasterization,
    DesktopIntegration,
    AppMenuEntry,
    AskWhereToSave,
    OpenSafeFiles,
    ClearCookiesOnQuit,
    ClearHistoryItem,
    ClearCookiesItem,
    ClearCacheItem,
    ClearDownloadsItem,
    OfferToSavePasswords,
    ExtensionsEnabled,
    ExtensionsWebStore,
    ExtensionsAutoUpdate,
    AutofillPasswords,
    ClearClipboardAfter,
    PlaylistQuickAction,
    PlaylistAutoDownload,
    PlaylistDeleteFiles,
    ReaderButton,
    ReaderRememberPerSite,
    TranslateOffer,
    TranslatePrompt,
    TranslateChanges,
    AutoCheckUpdates,
    UpdatePrereleases,
}

impl Setting {
    /// The setting's label, translated.
    fn label(self) -> &'static str {
        tr(self.source_label())
    }

    /// The setting's English label (what search also matches).
    fn source_label(self) -> &'static str {
        match self {
            Setting::HomeClock => tr_mark("Clock"),
            Setting::HomeGreeting => tr_mark("Greeting"),
            Setting::HomeSearch => tr_mark("Search field"),
            Setting::HomeTopSites => tr_mark("Top sites"),
            Setting::HomeStats => tr_mark("Shields statistics"),
            Setting::HomeGlassGround => tr_mark("Chrome colour as default background"),
            Setting::HomeLive => tr_mark("Live wallpapers (animated images and videos)"),
            Setting::HomeLiveIgnoreReduceMotion => tr_mark("Play live wallpapers with Reduce Motion"),
            Setting::DevTools => tr_mark("Developer tools"),
            Setting::HomeDim => tr_mark("Dim background image"),
            Setting::HomeBlur => tr_mark("Blur background image"),
            Setting::TintFromPage => tr_mark("Colour in tab bar"),
            Setting::AdaptToPage => tr_mark("Light or dark chrome to match the page"),
            Setting::TextScale => tr_mark("Text size"),
            Setting::PageCorner => tr_mark("Page corner radius"),
            Setting::FloatSidebar => tr_mark("Float sidebar over the page"),
            Setting::ReduceTransparency => tr_mark("Reduce transparency"),
            Setting::ReduceMotion => tr_mark("Reduce motion"),
            Setting::Refraction => tr_mark("Refraction"),
            Setting::Blur => tr_mark("Blur"),
            Setting::Dispersion => tr_mark("Colour dispersion"),
            Setting::Squash => tr_mark("Squash when moving"),
            Setting::DragStretch => tr_mark("Drag stretch (maximum)"),
            Setting::MergeDistance => tr_mark("Merge distance"),
            Setting::LockPrivateTabs => tr_mark("Lock private tabs with the account password"),
            Setting::HideAdElements => tr_mark("Hide empty ad spaces"),
            Setting::BlockPopups => tr_mark("Block popups opened without a click"),
            Setting::BlockRedirects => tr_mark("Block redirects to ad and tracker sites"),
            Setting::StripTracking => tr_mark("Remove tracking parameters from links"),
            Setting::ThirdPartyCookies => tr_mark("Block third-party cookies"),
            Setting::FreezeAfter => tr_mark("Freeze background tabs after"),
            Setting::DiscardAfter => tr_mark("Unload background tabs after"),
            Setting::LimitMemory => tr_mark("Unload tabs when memory runs over the budget"),
            Setting::MemoryBudget => tr_mark("Memory budget"),
            Setting::KeepMediaAwake => tr_mark("Keep tabs playing audio awake"),
            Setting::RestoreSession => tr_mark("Reopen last session's tabs at start"),
            Setting::OpenInBackground => tr_mark("Open new tabs in background"),
            Setting::CloseAnimation => tr_mark("Animate closing tabs"),
            Setting::KeepTabWidths => tr_mark("Keep tab widths while closing"),
            Setting::MiddleClickClosesTab => tr_mark("Middle click closes tabs"),
            Setting::NewTabJoinsGroup => tr_mark("New tabs join the group they open from"),
            Setting::GroupChipsInBar => tr_mark("Show tab group names in the tab bar"),
            Setting::ShowAudioIndicator => tr_mark("Show a speaker on tabs playing sound"),
            Setting::RememberSiteMute => tr_mark("Remember muted sites"),
            Setting::DragOutLinks => tr_mark("Drag tabs out as links"),
            Setting::DragOutImage => tr_mark("Show a preview while dragging a tab out"),
            Setting::HomeButton => tr_mark("Show home button"),
            Setting::OwnFullscreenControls => tr_mark("Axion's player in fullscreen"),
            Setting::HideControlsAfter => tr_mark("Hide controls while playing after"),
            Setting::HardwareDecoding => tr_mark("Hardware video decoding (after restart)"),
            Setting::HardwareEnabled => tr_mark("Use hardware acceleration"),
            Setting::GpuRasterization => tr_mark("Smooth WebGL, canvas and GPU rasterization"),
            Setting::DesktopIntegration => tr_mark("Desktop media controls and media keys (after restart)"),
            Setting::AppMenuEntry => tr_mark("Add Axion to the application menu"),
            Setting::AskWhereToSave => tr_mark("Ask where to save each file"),
            Setting::OpenSafeFiles => tr_mark("Open “safe” files after downloading"),
            Setting::ClearCookiesOnQuit => tr_mark("Clear cookies and site data when Axion quits"),
            Setting::ClearHistoryItem => tr_mark("Browsing history"),
            Setting::ClearCookiesItem => tr_mark("Cookies and other site data"),
            Setting::ClearCacheItem => tr_mark("Cached images and files"),
            Setting::ClearDownloadsItem => tr_mark("Downloads list"),
            Setting::OfferToSavePasswords => tr_mark("Offer to save passwords"),
            Setting::ExtensionsEnabled => tr_mark("Chrome extensions (after restart)"),
            Setting::ExtensionsWebStore => tr_mark("Install from the Chrome Web Store (after restart)"),
            Setting::ExtensionsAutoUpdate => tr_mark("Update Web Store extensions automatically"),
            Setting::AutofillPasswords => tr_mark("Offer to fill saved passwords"),
            Setting::ClearClipboardAfter => tr_mark("Clear clipboard after copying"),
            Setting::PlaylistQuickAction => tr_mark("Playlist button in the address field"),
            Setting::PlaylistAutoDownload => tr_mark("Download videos when adding them to the Playlist"),
            Setting::PlaylistDeleteFiles => tr_mark("Delete downloaded files when removing videos"),
            Setting::ReaderButton => tr_mark("Reader View in the page menu"),
            Setting::ReaderRememberPerSite => tr_mark("Remember Reader View’s look for each site"),
            Setting::TranslateOffer => tr_mark("Offer to translate pages in other languages"),
            Setting::TranslatePrompt => tr_mark("Open the offer by itself"),
            Setting::TranslateChanges => tr_mark("Translate text that appears later"),
            Setting::AutoCheckUpdates => tr_mark("Check for updates automatically"),
            Setting::UpdatePrereleases => tr_mark("Include pre-releases"),
        }
    }

    fn is_toggle(self) -> bool {
        matches!(
            self,
            Setting::HomeClock
                | Setting::HomeButton
                | Setting::HomeGreeting
                | Setting::HomeSearch
                | Setting::HomeTopSites
                | Setting::HomeStats
                | Setting::HomeLive
                | Setting::HomeGlassGround
                | Setting::HomeLiveIgnoreReduceMotion
                | Setting::DevTools
                | Setting::TintFromPage
                | Setting::FloatSidebar
                | Setting::AdaptToPage
                | Setting::ReduceTransparency
                | Setting::ReduceMotion
                | Setting::LockPrivateTabs
                | Setting::HideAdElements
                | Setting::BlockPopups
                | Setting::BlockRedirects
                | Setting::StripTracking
                | Setting::ThirdPartyCookies
                | Setting::LimitMemory
                | Setting::KeepMediaAwake
                | Setting::RestoreSession
                | Setting::OpenInBackground
                | Setting::CloseAnimation
                | Setting::KeepTabWidths
                | Setting::MiddleClickClosesTab
                | Setting::NewTabJoinsGroup
                | Setting::GroupChipsInBar
                | Setting::ShowAudioIndicator
                | Setting::RememberSiteMute
                | Setting::DragOutLinks
                | Setting::DragOutImage
                | Setting::OwnFullscreenControls
                | Setting::HardwareDecoding
                | Setting::HardwareEnabled
                | Setting::GpuRasterization
                | Setting::DesktopIntegration
                | Setting::AppMenuEntry
                | Setting::AskWhereToSave
                | Setting::OpenSafeFiles
                | Setting::ClearCookiesOnQuit
                | Setting::ClearHistoryItem
                | Setting::ClearCookiesItem
                | Setting::ClearCacheItem
                | Setting::ClearDownloadsItem
                | Setting::OfferToSavePasswords
                | Setting::ExtensionsEnabled
                | Setting::ExtensionsWebStore
                | Setting::ExtensionsAutoUpdate
                | Setting::AutofillPasswords
                | Setting::PlaylistQuickAction
                | Setting::PlaylistAutoDownload
                | Setting::PlaylistDeleteFiles
                | Setting::ReaderButton
                | Setting::ReaderRememberPerSite
                | Setting::TranslateOffer
                | Setting::TranslatePrompt
                | Setting::TranslateChanges
                | Setting::AutoCheckUpdates
                | Setting::UpdatePrereleases
        )
    }

    /// Slider range (min, max).
    fn range(self) -> (f32, f32) {
        match self {
            Setting::HomeDim => (0.0, 0.8),
            Setting::HomeBlur => (0.0, 30.0),
            Setting::TextScale => (0.85, 1.3),
            Setting::PageCorner => (0.0, 24.0),
            Setting::Refraction => (0.0, 1.0),
            // Blur is a factor on the default radii of all three thicknesses.
            Setting::Blur => (0.0, 2.0),
            Setting::Dispersion => (0.0, 0.4),
            Setting::Squash => (0.0, 0.2),
            Setting::DragStretch => (0.0, 0.2),
            Setting::MergeDistance => (0.0, 40.0),
            Setting::FreezeAfter => (0.0, 60.0),
            Setting::HideControlsAfter => (1.0, 10.0),
            Setting::DiscardAfter => (0.0, 480.0),
            Setting::MemoryBudget => (0.0, memory_slider_max_mb()),
            Setting::ClearClipboardAfter => (0.0, 300.0),
            _ => (0.0, 1.0),
        }
    }

    /// The slider's current value for people: "Never", "30 min", "2 h", "100 %", …
    fn display(self, c: &Config) -> String {
        let percent = |v: f32| format!("{:.0} %", v * 100.0);
        let minutes = |m: f32| match m {
            m if m <= 0.0 => tr("Never").to_owned(),
            m if m < 60.0 => tr_fmt!("{minutes} min", minutes = format!("{m:.0}")),
            m => tr_fmt!("{hours} h", hours = trim_number(m / 60.0)),
        };
        let defaults = Config::default();
        match self {
            Setting::HomeDim => percent(c.home.background_dim),
            Setting::HomeBlur => match c.home.background_blur {
                b if b <= 0.0 => tr("Off").to_owned(),
                b => format!("{b:.0} px"),
            },
            Setting::TextScale => percent(c.appearance.text_scale),
            Setting::PageCorner => format!("{:.0} px", c.layout.page_corner_radius),
            Setting::Refraction => percent(c.glass.light.refraction),
            Setting::Blur => {
                percent(c.glass.light.blur.regular / defaults.glass.light.blur.regular.max(0.001))
            }
            Setting::Dispersion => percent(c.glass.light.dispersion),
            Setting::Squash => percent(c.glass.squash),
            Setting::DragStretch => percent(c.glass.drag_stretch_max),
            Setting::MergeDistance => format!("{:.0} px", c.glass.merge_distance),
            Setting::FreezeAfter => minutes(c.tabs.freeze_after_minutes),
            Setting::DiscardAfter => minutes(c.tabs.discard_after_minutes),
            Setting::MemoryBudget => match c.tabs.memory_budget_mb {
                0 => {
                    let mut auto = c.clone();
                    auto.tabs.limit_memory = true;
                    crate::lifecycle::memory_budget(&auto).map_or(tr("Automatic").to_owned(), |b| {
                        tr_fmt!("Automatic ({size} GB)", size = format!("{:.0}", b as f64 / GIB))
                    })
                }
                mb => format_bytes(u64::from(mb) * 1024 * 1024),
            },
            Setting::HideControlsAfter => {
                tr_fmt!("{seconds} s", seconds = trim_number(c.media.hide_controls_after_seconds))
            }
            Setting::ClearClipboardAfter => match c.passwords.clear_clipboard_seconds {
                s if s <= 0.0 => tr("Never").to_owned(),
                s if s < 60.0 => tr_fmt!("{seconds} s", seconds = format!("{s:.0}")),
                s => tr_fmt!("{minutes} min", minutes = trim_number(s / 60.0)),
            },
            _ => String::new(),
        }
    }

    fn get(self, c: &Config) -> f32 {
        let defaults = Config::default();
        let raw = match self {
            Setting::HomeClock => return c.home.show_clock as u8 as f32,
            Setting::HomeGreeting => return c.home.show_greeting as u8 as f32,
            Setting::HomeSearch => return c.home.show_search as u8 as f32,
            Setting::HomeTopSites => return c.home.show_top_sites as u8 as f32,
            Setting::HomeStats => return c.home.show_stats as u8 as f32,
            Setting::HomeGlassGround => return c.home.glass_ground as u8 as f32,
            Setting::HomeLive => return c.home.live_wallpapers as u8 as f32,
            Setting::HomeLiveIgnoreReduceMotion => {
                return c.home.live_wallpapers_ignore_reduce_motion as u8 as f32;
            }
            Setting::DevTools => return c.devtools.enabled as u8 as f32,
            Setting::HomeDim => c.home.background_dim,
            Setting::HomeBlur => c.home.background_blur,
            Setting::TintFromPage => return c.appearance.tint_from_page as u8 as f32,
            Setting::FloatSidebar => return c.layout.float_sidebar as u8 as f32,
            Setting::AdaptToPage => return c.appearance.adapt_to_page as u8 as f32,
            Setting::ReduceTransparency => return c.glass.reduce_transparency as u8 as f32,
            Setting::ReduceMotion => return c.glass.reduce_motion as u8 as f32,
            Setting::LockPrivateTabs => return c.privacy.lock_private_tabs as u8 as f32,
            Setting::HideAdElements => return c.shields.hide_ad_elements as u8 as f32,
            Setting::BlockPopups => return c.shields.block_popups as u8 as f32,
            Setting::BlockRedirects => return c.shields.block_redirects as u8 as f32,
            Setting::StripTracking => return c.shields.strip_tracking_parameters as u8 as f32,
            Setting::ThirdPartyCookies => return c.shields.block_third_party_cookies as u8 as f32,
            Setting::LimitMemory => return c.tabs.limit_memory as u8 as f32,
            Setting::KeepMediaAwake => return c.tabs.keep_media_awake as u8 as f32,
            Setting::RestoreSession => return c.tabs.restore_session as u8 as f32,
            Setting::OpenInBackground => return c.tabs.open_links_in_background as u8 as f32,
            Setting::CloseAnimation => return c.tabs.close_animation as u8 as f32,
            Setting::KeepTabWidths => return c.tabs.keep_widths_while_closing as u8 as f32,
            Setting::MiddleClickClosesTab => return c.tabs.middle_click_closes_tab as u8 as f32,
            Setting::NewTabJoinsGroup => return c.tabs.new_tab_joins_group as u8 as f32,
            Setting::GroupChipsInBar => return c.tabs.group_chips_in_bar as u8 as f32,
            Setting::ShowAudioIndicator => return c.tabs.show_audio_indicator as u8 as f32,
            Setting::RememberSiteMute => return c.tabs.remember_site_mute as u8 as f32,
            Setting::DragOutLinks => return c.tabs.drag_out_links as u8 as f32,
            Setting::DragOutImage => return c.tabs.drag_out_image as u8 as f32,
            Setting::HomeButton => return c.layout.home_button as u8 as f32,
            Setting::OwnFullscreenControls => return c.media.fullscreen_player as u8 as f32,
            Setting::HardwareDecoding => return c.media.hardware_decoding as u8 as f32,
            Setting::HardwareEnabled => return c.hardware.enabled as u8 as f32,
            Setting::GpuRasterization => return c.hardware.gpu_rasterization as u8 as f32,
            Setting::DesktopIntegration => return c.media.desktop_integration as u8 as f32,
            Setting::AppMenuEntry => return c.general.desktop_integration as u8 as f32,
            Setting::AskWhereToSave => return c.downloads.ask_where_to_save as u8 as f32,
            Setting::OpenSafeFiles => return c.downloads.open_safe_files as u8 as f32,
            Setting::ClearCookiesOnQuit => return c.data.clear_cookies_on_quit as u8 as f32,
            Setting::ClearHistoryItem => return c.data.clear_history as u8 as f32,
            Setting::ClearCookiesItem => return c.data.clear_cookies as u8 as f32,
            Setting::ClearCacheItem => return c.data.clear_cache as u8 as f32,
            Setting::ClearDownloadsItem => return c.data.clear_downloads as u8 as f32,
            Setting::OfferToSavePasswords => return c.passwords.offer_to_save as u8 as f32,
            Setting::ExtensionsEnabled => return c.extensions.enabled as u8 as f32,
            Setting::ExtensionsWebStore => return c.extensions.web_store as u8 as f32,
            Setting::ExtensionsAutoUpdate => return c.extensions.auto_update as u8 as f32,
            Setting::AutofillPasswords => return c.passwords.autofill as u8 as f32,
            Setting::PlaylistQuickAction => return c.playlist.quick_action as u8 as f32,
            Setting::PlaylistAutoDownload => return c.playlist.auto_download as u8 as f32,
            Setting::PlaylistDeleteFiles => return c.playlist.delete_files_on_remove as u8 as f32,
            Setting::ReaderButton => return c.reader.button as u8 as f32,
            Setting::ReaderRememberPerSite => return c.reader.remember_per_site as u8 as f32,
            Setting::TranslateOffer => return c.translation.offer as u8 as f32,
            Setting::TranslatePrompt => return c.translation.prompt as u8 as f32,
            Setting::TranslateChanges => return c.translation.translate_changes as u8 as f32,
            Setting::AutoCheckUpdates => return c.updates.auto_check_on_start as u8 as f32,
            Setting::UpdatePrereleases => {
                return (c.updates.channel == axion_config::UpdateChannel::Prerelease) as u8 as f32;
            }
            Setting::ClearClipboardAfter => c.passwords.clear_clipboard_seconds,
            Setting::HideControlsAfter => c.media.hide_controls_after_seconds,
            Setting::FreezeAfter => c.tabs.freeze_after_minutes,
            Setting::DiscardAfter => c.tabs.discard_after_minutes,
            Setting::MemoryBudget => c.tabs.memory_budget_mb as f32,
            Setting::TextScale => c.appearance.text_scale,
            Setting::PageCorner => c.layout.page_corner_radius,
            Setting::Refraction => c.glass.light.refraction,
            Setting::Blur => c.glass.light.blur.regular / defaults.glass.light.blur.regular,
            Setting::Dispersion => c.glass.light.dispersion,
            Setting::Squash => c.glass.squash,
            Setting::DragStretch => c.glass.drag_stretch_max,
            Setting::MergeDistance => c.glass.merge_distance,
        };
        let (min, max) = self.range();
        ((raw - min) / (max - min)).clamp(0.0, 1.0)
    }

    fn set(self, c: &mut Config, value: f32) {
        let (min, max) = self.range();
        let v = min + (max - min) * value;
        let on = value > 0.5;
        let defaults = Config::default();
        match self {
            Setting::HomeClock => c.home.show_clock = on,
            Setting::HomeGreeting => c.home.show_greeting = on,
            Setting::HomeSearch => c.home.show_search = on,
            Setting::HomeTopSites => c.home.show_top_sites = on,
            Setting::HomeStats => c.home.show_stats = on,
            Setting::HomeGlassGround => c.home.glass_ground = on,
            Setting::HomeLive => c.home.live_wallpapers = on,
            Setting::HomeLiveIgnoreReduceMotion => c.home.live_wallpapers_ignore_reduce_motion = on,
            Setting::DevTools => c.devtools.enabled = on,
            Setting::HomeDim => c.home.background_dim = (v * 20.0).round() / 20.0,
            Setting::HomeBlur => c.home.background_blur = v.round(),
            Setting::TintFromPage => {
                c.appearance.tint_from_page = on;
                if on && c.appearance.tint_mode == TintMode::Neutral {
                    c.appearance.tint_mode = TintMode::Extreme;
                }
            }
            Setting::FloatSidebar => c.layout.float_sidebar = on,
            Setting::AdaptToPage => c.appearance.adapt_to_page = on,
            Setting::ReduceTransparency => c.glass.reduce_transparency = on,
            Setting::ReduceMotion => c.glass.reduce_motion = on,
            Setting::LockPrivateTabs => c.privacy.lock_private_tabs = on,
            Setting::HideAdElements => c.shields.hide_ad_elements = on,
            Setting::BlockPopups => c.shields.block_popups = on,
            Setting::BlockRedirects => c.shields.block_redirects = on,
            Setting::StripTracking => c.shields.strip_tracking_parameters = on,
            Setting::ThirdPartyCookies => c.shields.block_third_party_cookies = on,
            Setting::LimitMemory => c.tabs.limit_memory = on,
            Setting::KeepMediaAwake => c.tabs.keep_media_awake = on,
            Setting::RestoreSession => c.tabs.restore_session = on,
            Setting::OpenInBackground => c.tabs.open_links_in_background = on,
            Setting::CloseAnimation => c.tabs.close_animation = on,
            Setting::KeepTabWidths => c.tabs.keep_widths_while_closing = on,
            Setting::MiddleClickClosesTab => c.tabs.middle_click_closes_tab = on,
            Setting::NewTabJoinsGroup => c.tabs.new_tab_joins_group = on,
            Setting::GroupChipsInBar => c.tabs.group_chips_in_bar = on,
            Setting::ShowAudioIndicator => c.tabs.show_audio_indicator = on,
            Setting::RememberSiteMute => c.tabs.remember_site_mute = on,
            Setting::DragOutLinks => c.tabs.drag_out_links = on,
            Setting::DragOutImage => c.tabs.drag_out_image = on,
            Setting::HomeButton => c.layout.home_button = on,
            Setting::OwnFullscreenControls => c.media.fullscreen_player = on,
            Setting::HardwareDecoding => c.media.hardware_decoding = on,
            Setting::HardwareEnabled => c.hardware.enabled = on,
            Setting::GpuRasterization => c.hardware.gpu_rasterization = on,
            Setting::DesktopIntegration => c.media.desktop_integration = on,
            Setting::AppMenuEntry => c.general.desktop_integration = on,
            Setting::AskWhereToSave => c.downloads.ask_where_to_save = on,
            Setting::OpenSafeFiles => c.downloads.open_safe_files = on,
            Setting::ClearCookiesOnQuit => c.data.clear_cookies_on_quit = on,
            Setting::ClearHistoryItem => c.data.clear_history = on,
            Setting::ClearCookiesItem => c.data.clear_cookies = on,
            Setting::ClearCacheItem => c.data.clear_cache = on,
            Setting::ClearDownloadsItem => c.data.clear_downloads = on,
            Setting::OfferToSavePasswords => c.passwords.offer_to_save = on,
            Setting::ExtensionsEnabled => c.extensions.enabled = on,
            Setting::ExtensionsWebStore => c.extensions.web_store = on,
            Setting::ExtensionsAutoUpdate => c.extensions.auto_update = on,
            Setting::AutofillPasswords => c.passwords.autofill = on,
            Setting::PlaylistQuickAction => c.playlist.quick_action = on,
            Setting::PlaylistAutoDownload => c.playlist.auto_download = on,
            Setting::PlaylistDeleteFiles => c.playlist.delete_files_on_remove = on,
            Setting::ReaderButton => c.reader.button = on,
            Setting::ReaderRememberPerSite => c.reader.remember_per_site = on,
            Setting::TranslateOffer => c.translation.offer = on,
            Setting::TranslatePrompt => c.translation.prompt = on,
            Setting::TranslateChanges => c.translation.translate_changes = on,
            Setting::AutoCheckUpdates => c.updates.auto_check_on_start = on,
            Setting::UpdatePrereleases => {
                c.updates.channel = if on {
                    axion_config::UpdateChannel::Prerelease
                } else {
                    axion_config::UpdateChannel::Stable
                };
            }
            Setting::ClearClipboardAfter => c.passwords.clear_clipboard_seconds = (v / 5.0).round() * 5.0,
            Setting::HideControlsAfter => c.media.hide_controls_after_seconds = (v * 2.0).round() / 2.0,
            Setting::FreezeAfter => c.tabs.freeze_after_minutes = v.round(),
            Setting::DiscardAfter => c.tabs.discard_after_minutes = (v / 5.0).round() * 5.0,
            Setting::MemoryBudget => c.tabs.memory_budget_mb = ((v / 256.0).round() * 256.0) as u32,
            Setting::TextScale => c.appearance.text_scale = (v * 20.0).round() / 20.0,
            Setting::PageCorner => c.layout.page_corner_radius = v.round(),
            Setting::Refraction => {
                c.glass.light.refraction = v;
                c.glass.dark.refraction = v;
            }
            Setting::Blur => {
                for (m, d) in
                    [(&mut c.glass.light, &defaults.glass.light), (&mut c.glass.dark, &defaults.glass.dark)]
                {
                    m.blur.thin = (d.blur.thin * v).round();
                    m.blur.regular = (d.blur.regular * v).round();
                    m.blur.thick = (d.blur.thick * v).round();
                }
            }
            Setting::Dispersion => {
                c.glass.light.dispersion = v;
                c.glass.dark.dispersion =
                    v * defaults.glass.dark.dispersion / defaults.glass.light.dispersion;
            }
            Setting::Squash => c.glass.squash = v,
            Setting::DragStretch => c.glass.drag_stretch_max = v,
            Setting::MergeDistance => c.glass.merge_distance = v.round(),
        }
    }
}

/// Clicks inside the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Close,
    Category(Category),
    Theme(ThemeMode),
    /// Liquid Glass or the flat, glass-free look.
    GlassStyle(GlassStyle),
    /// Index into `ICON_WEIGHTS`.
    IconWeight(usize),
    SearchEngine(usize),
    Accent(usize),
    ClearHistory,
    /// Sets or changes the account password from the fields on the page.
    SetPassword,
    SaveAccountName,
    /// Index into the accent swatches.
    AccountColor(usize),
    /// Pick a profile photo with the desktop's file chooser.
    ChooseAccountPhoto,
    RemoveAccountPhoto,
    DeleteAccount,
    ShieldLevel(ShieldLevel),
    UpdateFilterLists,
    TabBar(TabBarStyle),
    BookmarksBar(BookmarksBar),
    /// `appearance.tint_mode` while the page colour is on.
    TintMode(TintMode),
    PlaylistSort(PlaylistSort),
    ReaderFont(ReaderFont),
    ReaderTheme(ReaderTheme),
    TranslationBackend(TranslationBackend),
    /// Index into [`TRANSLATE_TARGETS`].
    TranslateTarget(usize),
    /// Deletes the downloaded translation models.
    RemoveTranslationModels,
    Language(UiLanguage),
    /// Runs the first-run introduction again.
    ShowWelcomeTour,
    CloseAfter(u32),
    AddTrustedSite,
    /// Index into `media.trusted_player_sites`.
    RemoveTrustedSite(usize),
    ChooseDownloadFolder,
    /// Pick the Playlist's download folder, or go back to the default one.
    ChoosePlaylistFolder,
    ResetPlaylistFolder,
    RemoveDownloads(RemoveDownloads),
    ClearRange(ClearRange),
    ClearBrowsingData,
    RemoveAllSiteData,
    /// Index into the listed sites with data.
    RemoveSiteData(usize),
    OpenPasswords,
    ImportPasswords,
    RestorePasswords,
    ExportPasswords,
    /// The start page's background: none, one of the listed ones (index), or remove one.
    NoBackground,
    Background(usize),
    RemoveBackground(usize),
    /// Add a background with the desktop's file chooser, or from the path field.
    ChooseBackground,
    AddBackgroundPath,
    /// About & Updates.
    CheckForUpdates,
    InstallUpdate,
    SkipUpdate,
    RemindLater,
    OpenReleasePage,
    /// Store the repository typed into the field.
    SaveRepository,
    RelaunchNow,
    /// Hardware page: how page frames reach Axion, and which graphics card.
    PageFrames(PageFrames),
    GpuCard(GpuChoice),
    CopyDiagnostics,
    /// Relaunch to apply the Hardware settings.
    RestartAxion,
    /// Privacy › Site Permissions: a group's default (index into `Group::ALL`), Ask or Block.
    PermissionDefault(usize, PermissionDefault),
    /// Forget one remembered site decision (index into the listed ones), or all of them.
    ForgetPermission(usize),
    ResetPermissions,
    /// Data & Downloads: importing from other browsers.
    Import(ImportAction),
}

/// Privacy › Site Permissions: the defaults' segments, "Reset All", then one forget button per
/// listed decision (ids between the trusted sites' and the Hardware page's).
const PERMISSION_DEFAULT_BASE: u32 = 600;
const RESET_PERMISSIONS: u32 = 619;
const PERMISSION_SITE_BASE: u32 = 620;
const PERMISSION_SITE_LIMIT: usize = (HARDWARE_BASE - PERMISSION_SITE_BASE) as usize;
const PERMISSION_DEFAULTS: [(&str, PermissionDefault); 2] =
    [(tr_mark("Ask"), PermissionDefault::Ask), (tr_mark("Block"), PermissionDefault::Block)];

/// The Hardware page's actions (ids between the trusted sites' and the pages').
const HARDWARE_BASE: u32 = 880;

/// The About page's actions: the ids after the background picker's.
const UPDATE_BASE: u32 = 990;
const UPDATE_ACTIONS: u32 = 7;

/// The background picker's actions (after the categories' ids).
const NO_BACKGROUND: u32 = 920;
const CHOOSE_BACKGROUND: u32 = 921;
const ADD_BACKGROUND_PATH: u32 = 922;
const BACKGROUND_BASE: u32 = 930;
const REMOVE_BACKGROUND_BASE: u32 = BACKGROUND_BASE + BACKGROUND_SLOTS as u32;

/// Remove buttons of the trusted player sites (one id per row, up to `CATEGORY_BASE`).
const TRUSTED_SITE_BASE: u32 = 200;
/// The pages in the list on the left.
const CATEGORY_BASE: u32 = 900;
/// Reader view and translation controls.
const READING_BASE: u32 = 1100;
const READER_FONTS: [(&str, ReaderFont); 3] = [
    (tr_mark("Serif"), ReaderFont::Serif),
    (tr_mark("Sans"), ReaderFont::Sans),
    (tr_mark("Mono"), ReaderFont::Mono),
];
const READER_THEMES: [(&str, ReaderTheme); 4] = [
    (tr_mark("Auto"), ReaderTheme::Auto),
    (tr_mark("Light"), ReaderTheme::Light),
    (tr_mark("Sepia"), ReaderTheme::Sepia),
    (tr_mark("Dark"), ReaderTheme::Dark),
];
const TRANSLATION_BACKENDS: [(&str, TranslationBackend); 2] = [
    (tr_mark("On this computer"), TranslationBackend::Local),
    (tr_mark("Your server"), TranslationBackend::Endpoint),
];
/// Languages pages are translated into: the code (empty: the interface language) and its name.
const TRANSLATE_TARGETS: [(&str, &str); 5] = [
    ("", tr_mark("Interface")),
    ("en", tr_mark("English")),
    ("de", tr_mark("German")),
    ("fr", tr_mark("French")),
    ("es", tr_mark("Spanish")),
];

/// Remove buttons of the sites with data, and how many sites are listed.
const SITE_DATA_BASE: u32 = 100;
const SITE_DATA_LIMIT: usize = 100;

const REMOVE_DOWNLOADS: [(&str, RemoveDownloads); 3] = [
    (tr_mark("Manually"), RemoveDownloads::Manually),
    (tr_mark("After One Day"), RemoveDownloads::AfterOneDay),
    (tr_mark("When Axion Quits"), RemoveDownloads::OnQuit),
];

const CLEAR_RANGES: [(&str, ClearRange); 4] = [
    (tr_mark("Last Hour"), ClearRange::LastHour),
    (tr_mark("Today"), ClearRange::Today),
    (tr_mark("Last 7 Days"), ClearRange::LastWeek),
    (tr_mark("All Time"), ClearRange::AllTime),
];

const DESTRUCTIVE: Color = Color::rgb(0xD0362B);

const CLOSE_AFTER: [(&str, u32); 4] = [
    (tr_mark("Manually"), 0),
    (tr_mark("After a Day"), 1),
    (tr_mark("After a Week"), 7),
    (tr_mark("After a Month"), 30),
];

const TINT_MODES: [(&str, TintMode); 2] =
    [(tr_mark("Extreme"), TintMode::Extreme), (tr_mark("Dominant"), TintMode::Dominant)];

const BOOKMARKS_BAR: [(&str, BookmarksBar); 3] = [
    (tr_mark("Always"), BookmarksBar::Always),
    (tr_mark("On Home Page"), BookmarksBar::Home),
    (tr_mark("Never"), BookmarksBar::Never),
];

/// Icon weight presets: (name, `appearance.icon_stroke_light`, `appearance.icon_stroke_dark`).
const ICON_WEIGHTS: [(&str, f32, f32); 3] =
    [(tr_mark("Regular"), 1.0, 1.0), (tr_mark("Medium"), 1.2, 1.0), (tr_mark("Bold"), 1.4, 1.15)];

/// The preset the config's two multipliers match, if any (custom values match none).
fn icon_weight_preset(c: &Config) -> Option<usize> {
    let close = |a: f32, b: f32| (a - b).abs() < 0.005;
    ICON_WEIGHTS.iter().position(|&(_, light, dark)| {
        close(c.appearance.icon_stroke_light, light) && close(c.appearance.icon_stroke_dark, dark)
    })
}

fn set_icon_weight(config: &mut Config, preset: usize) {
    let (_, light, dark) = ICON_WEIGHTS[preset];
    config.appearance.icon_stroke_light = light;
    config.appearance.icon_stroke_dark = dark;
}

/// The language choices, named in their own language (never translated).
const LANGUAGES: [(&str, UiLanguage); 3] =
    [("System", UiLanguage::System), ("English", UiLanguage::English), ("Deutsch", UiLanguage::German)];

/// The page rendering choices (labels are translated where drawn).
const PAGE_FRAMES: [(&str, PageFrames); 3] = [
    (tr_mark("Automatic"), PageFrames::Auto),
    (tr_mark("Shared GPU memory"), PageFrames::Gpu),
    (tr_mark("Software"), PageFrames::Software),
];

const GPU_CARDS: [(&str, GpuChoice); 3] = [
    (tr_mark("Automatic"), GpuChoice::Auto),
    (tr_mark("Integrated"), GpuChoice::Low),
    (tr_mark("Dedicated"), GpuChoice::High),
];

/// An adapter type (`AdapterLine::kind`) in the current language.
fn adapter_kind(kind: &str) -> &'static str {
    match kind {
        "integrated" => tr("integrated"),
        "discrete" => tr("dedicated"),
        "virtual" => tr("virtual"),
        "software" => tr("software"),
        _ => tr("other"),
    }
}

fn page_frames_index(frames: PageFrames) -> u32 {
    PAGE_FRAMES.iter().position(|&(_, f)| f == frames).unwrap_or(0) as u32
}

fn gpu_card_index(card: GpuChoice) -> u32 {
    GPU_CARDS.iter().position(|&(_, g)| g == card).unwrap_or(0) as u32
}

const SHIELD_LEVELS: [(&str, ShieldLevel); 3] = [
    (tr_mark("Off"), ShieldLevel::Off),
    (tr_mark("Standard"), ShieldLevel::Standard),
    (tr_mark("Aggressive"), ShieldLevel::Aggressive),
];

impl SettingsAction {
    fn id(self) -> ActionId {
        ActionId(
            ACTION_BASE
                + match self {
                    SettingsAction::Close => 0,
                    SettingsAction::Category(c) => {
                        CATEGORY_BASE + Category::ALL.iter().position(|&x| x == c).unwrap_or(0) as u32
                    }
                    SettingsAction::Theme(ThemeMode::Auto) => 20,
                    SettingsAction::Theme(ThemeMode::Light) => 21,
                    SettingsAction::Theme(ThemeMode::Dark) => 22,
                    SettingsAction::Language(language) => {
                        23 + LANGUAGES.iter().position(|&(_, l)| l == language).unwrap_or(0) as u32
                    }
                    SettingsAction::GlassStyle(GlassStyle::Liquid) => 26,
                    SettingsAction::GlassStyle(GlassStyle::Flat) => 27,
                    SettingsAction::ShowWelcomeTour => 28,
                    SettingsAction::SearchEngine(i) => 30 + i as u32,
                    SettingsAction::Accent(i) => 40 + i as u32,
                    SettingsAction::ClearHistory => 50,
                    SettingsAction::SetPassword => 51,
                    SettingsAction::SaveAccountName => 52,
                    SettingsAction::DeleteAccount => 53,
                    SettingsAction::AccountColor(i) => 54 + i as u32,
                    SettingsAction::IconWeight(i) => 66 + i as u32,
                    SettingsAction::ChooseAccountPhoto => 64,
                    SettingsAction::RemoveAccountPhoto => 65,
                    SettingsAction::ShieldLevel(level) => {
                        60 + SHIELD_LEVELS.iter().position(|(_, l)| *l == level).unwrap_or(0) as u32
                    }
                    SettingsAction::UpdateFilterLists => 63,
                    SettingsAction::TabBar(TabBarStyle::Separate) => 70,
                    SettingsAction::TabBar(TabBarStyle::Compact) => 71,
                    SettingsAction::CloseAfter(days) => {
                        72 + CLOSE_AFTER.iter().position(|(_, d)| *d == days).unwrap_or(0) as u32
                    }
                    SettingsAction::BookmarksBar(mode) => {
                        76 + BOOKMARKS_BAR.iter().position(|(_, m)| *m == mode).unwrap_or(0) as u32
                    }
                    SettingsAction::TintMode(mode) => {
                        if mode == TintMode::Dominant {
                            79
                        } else {
                            69
                        }
                    }
                    SettingsAction::PlaylistSort(sort) => {
                        81 + crate::playlist::SORTS.iter().position(|(s, ..)| *s == sort).unwrap_or(0) as u32
                    }
                    SettingsAction::ReaderFont(f) => {
                        READING_BASE + READER_FONTS.iter().position(|(_, x)| *x == f).unwrap_or(0) as u32
                    }
                    SettingsAction::ReaderTheme(t) => {
                        READING_BASE + 3 + READER_THEMES.iter().position(|(_, x)| *x == t).unwrap_or(0) as u32
                    }
                    SettingsAction::TranslationBackend(TranslationBackend::Local) => READING_BASE + 7,
                    SettingsAction::TranslationBackend(TranslationBackend::Endpoint) => READING_BASE + 8,
                    SettingsAction::TranslateTarget(i) => READING_BASE + 10 + i as u32,
                    SettingsAction::RemoveTranslationModels => READING_BASE + 30,
                    SettingsAction::AddTrustedSite => 80,
                    SettingsAction::OpenPasswords => 85,
                    SettingsAction::ImportPasswords => 86,
                    SettingsAction::ExportPasswords => 87,
                    SettingsAction::RestorePasswords => 88,
                    SettingsAction::RemoveTrustedSite(i) => TRUSTED_SITE_BASE + i as u32,
                    SettingsAction::PermissionDefault(group, value) => {
                        PERMISSION_DEFAULT_BASE
                            + 2 * group as u32
                            + PERMISSION_DEFAULTS.iter().position(|(_, v)| *v == value).unwrap_or(0) as u32
                    }
                    SettingsAction::ResetPermissions => RESET_PERMISSIONS,
                    SettingsAction::ForgetPermission(i) => PERMISSION_SITE_BASE + i as u32,
                    SettingsAction::ChooseDownloadFolder => 90,
                    SettingsAction::ResetPlaylistFolder => 84,
                    SettingsAction::ChoosePlaylistFolder => 89,
                    SettingsAction::RemoveDownloads(r) => {
                        91 + REMOVE_DOWNLOADS.iter().position(|(_, x)| *x == r).unwrap_or(0) as u32
                    }
                    SettingsAction::ClearRange(r) => {
                        94 + CLEAR_RANGES.iter().position(|(_, x)| *x == r).unwrap_or(0) as u32
                    }
                    SettingsAction::ClearBrowsingData => 98,
                    SettingsAction::RemoveAllSiteData => 99,
                    SettingsAction::RemoveSiteData(i) => SITE_DATA_BASE + i as u32,
                    SettingsAction::NoBackground => NO_BACKGROUND,
                    SettingsAction::ChooseBackground => CHOOSE_BACKGROUND,
                    SettingsAction::AddBackgroundPath => ADD_BACKGROUND_PATH,
                    SettingsAction::Background(i) => BACKGROUND_BASE + i as u32,
                    SettingsAction::RemoveBackground(i) => REMOVE_BACKGROUND_BASE + i as u32,
                    SettingsAction::CheckForUpdates => UPDATE_BASE,
                    SettingsAction::InstallUpdate => UPDATE_BASE + 1,
                    SettingsAction::SkipUpdate => UPDATE_BASE + 2,
                    SettingsAction::RemindLater => UPDATE_BASE + 3,
                    SettingsAction::OpenReleasePage => UPDATE_BASE + 4,
                    SettingsAction::SaveRepository => UPDATE_BASE + 5,
                    SettingsAction::RelaunchNow => UPDATE_BASE + 6,
                    SettingsAction::Import(action) => action.id(),
                    SettingsAction::PageFrames(f) => HARDWARE_BASE + page_frames_index(f),
                    SettingsAction::GpuCard(g) => HARDWARE_BASE + 3 + gpu_card_index(g),
                    SettingsAction::CopyDiagnostics => HARDWARE_BASE + 6,
                    SettingsAction::RestartAxion => HARDWARE_BASE + 7,
                },
        )
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        let n = id.0.checked_sub(ACTION_BASE)?;
        Some(match n {
            0 => SettingsAction::Close,
            n if (CATEGORY_BASE..CATEGORY_BASE + Category::ALL.len() as u32).contains(&n) => {
                SettingsAction::Category(Category::ALL[(n - CATEGORY_BASE) as usize])
            }
            20 => SettingsAction::Theme(ThemeMode::Auto),
            21 => SettingsAction::Theme(ThemeMode::Light),
            22 => SettingsAction::Theme(ThemeMode::Dark),
            23..=25 => SettingsAction::Language(LANGUAGES[(n - 23) as usize].1),
            26 => SettingsAction::GlassStyle(GlassStyle::Liquid),
            27 => SettingsAction::GlassStyle(GlassStyle::Flat),
            28 => SettingsAction::ShowWelcomeTour,
            30..=39 if ((n - 30) as usize) < SEARCH_ENGINES.len() => {
                SettingsAction::SearchEngine((n - 30) as usize)
            }
            40..=49 if ((n - 40) as usize) < ACCENTS.len() => SettingsAction::Accent((n - 40) as usize),
            50 => SettingsAction::ClearHistory,
            51 => SettingsAction::SetPassword,
            52 => SettingsAction::SaveAccountName,
            53 => SettingsAction::DeleteAccount,
            54..=59 if ((n - 54) as usize) < ACCENTS.len() => SettingsAction::AccountColor((n - 54) as usize),
            60..=62 => SettingsAction::ShieldLevel(SHIELD_LEVELS[(n - 60) as usize].1),
            63 => SettingsAction::UpdateFilterLists,
            64 => SettingsAction::ChooseAccountPhoto,
            66..=68 => SettingsAction::IconWeight((n - 66) as usize),
            65 => SettingsAction::RemoveAccountPhoto,
            70 => SettingsAction::TabBar(TabBarStyle::Separate),
            71 => SettingsAction::TabBar(TabBarStyle::Compact),
            72..=75 => SettingsAction::CloseAfter(CLOSE_AFTER[(n - 72) as usize].1),
            76..=78 => SettingsAction::BookmarksBar(BOOKMARKS_BAR[(n - 76) as usize].1),
            69 => SettingsAction::TintMode(TintMode::Extreme),
            79 => SettingsAction::TintMode(TintMode::Dominant),
            80 => SettingsAction::AddTrustedSite,
            81..=83 => SettingsAction::PlaylistSort(crate::playlist::SORTS[(n - 81) as usize].0),
            n if (READING_BASE..READING_BASE + 3).contains(&n) => {
                SettingsAction::ReaderFont(READER_FONTS[(n - READING_BASE) as usize].1)
            }
            n if (READING_BASE + 3..READING_BASE + 7).contains(&n) => {
                SettingsAction::ReaderTheme(READER_THEMES[(n - READING_BASE - 3) as usize].1)
            }
            n if n == READING_BASE + 7 => SettingsAction::TranslationBackend(TranslationBackend::Local),
            n if n == READING_BASE + 8 => SettingsAction::TranslationBackend(TranslationBackend::Endpoint),
            n if (READING_BASE + 10..READING_BASE + 10 + TRANSLATE_TARGETS.len() as u32).contains(&n) => {
                SettingsAction::TranslateTarget((n - READING_BASE - 10) as usize)
            }
            n if n == READING_BASE + 30 => SettingsAction::RemoveTranslationModels,
            85 => SettingsAction::OpenPasswords,
            86 => SettingsAction::ImportPasswords,
            87 => SettingsAction::ExportPasswords,
            88 => SettingsAction::RestorePasswords,
            84 => SettingsAction::ResetPlaylistFolder,
            89 => SettingsAction::ChoosePlaylistFolder,
            90 => SettingsAction::ChooseDownloadFolder,
            91..=93 => SettingsAction::RemoveDownloads(REMOVE_DOWNLOADS[(n - 91) as usize].1),
            94..=97 => SettingsAction::ClearRange(CLEAR_RANGES[(n - 94) as usize].1),
            98 => SettingsAction::ClearBrowsingData,
            99 => SettingsAction::RemoveAllSiteData,
            n if (SITE_DATA_BASE..SITE_DATA_BASE + SITE_DATA_LIMIT as u32).contains(&n) => {
                SettingsAction::RemoveSiteData((n - SITE_DATA_BASE) as usize)
            }
            n if (HARDWARE_BASE..HARDWARE_BASE + 8).contains(&n) => match n - HARDWARE_BASE {
                i @ 0..=2 => SettingsAction::PageFrames(PAGE_FRAMES[i as usize].1),
                i @ 3..=5 => SettingsAction::GpuCard(GPU_CARDS[i as usize - 3].1),
                6 => SettingsAction::CopyDiagnostics,
                _ => SettingsAction::RestartAxion,
            },
            n if (PERMISSION_DEFAULT_BASE..PERMISSION_DEFAULT_BASE + 2 * Group::ALL.len() as u32)
                .contains(&n) =>
            {
                let i = n - PERMISSION_DEFAULT_BASE;
                SettingsAction::PermissionDefault((i / 2) as usize, PERMISSION_DEFAULTS[(i % 2) as usize].1)
            }
            RESET_PERMISSIONS => SettingsAction::ResetPermissions,
            n if (PERMISSION_SITE_BASE..HARDWARE_BASE).contains(&n) => {
                SettingsAction::ForgetPermission((n - PERMISSION_SITE_BASE) as usize)
            }
            n if (TRUSTED_SITE_BASE..PERMISSION_DEFAULT_BASE).contains(&n) => {
                SettingsAction::RemoveTrustedSite((n - TRUSTED_SITE_BASE) as usize)
            }
            NO_BACKGROUND => SettingsAction::NoBackground,
            CHOOSE_BACKGROUND => SettingsAction::ChooseBackground,
            ADD_BACKGROUND_PATH => SettingsAction::AddBackgroundPath,
            n if (BACKGROUND_BASE..REMOVE_BACKGROUND_BASE).contains(&n) => {
                SettingsAction::Background((n - BACKGROUND_BASE) as usize)
            }
            n if (REMOVE_BACKGROUND_BASE..REMOVE_BACKGROUND_BASE + BACKGROUND_SLOTS as u32).contains(&n) => {
                SettingsAction::RemoveBackground((n - REMOVE_BACKGROUND_BASE) as usize)
            }
            n if (UPDATE_BASE..UPDATE_BASE + UPDATE_ACTIONS).contains(&n) => [
                SettingsAction::CheckForUpdates,
                SettingsAction::InstallUpdate,
                SettingsAction::SkipUpdate,
                SettingsAction::RemindLater,
                SettingsAction::OpenReleasePage,
                SettingsAction::SaveRepository,
                SettingsAction::RelaunchNow,
            ][(n - UPDATE_BASE) as usize],
            n if ImportAction::from_id(n).is_some() => SettingsAction::Import(ImportAction::from_id(n)?),
            _ => return None,
        })
    }
}

/// What the browser should do after a click or edit in the sheet.
pub enum Outcome {
    Nothing,
    /// Close Settings and run the first-run introduction.
    ShowWelcomeTour,
    /// The configuration changed; save and apply it.
    Changed,
    Close,
    ClearHistory,
    /// Delete the downloaded translation models.
    RemoveTranslationModels,
    /// Set the account password (`current: None`: it has none yet) or change it.
    SetAccountPassword {
        current: Option<Zeroizing<String>>,
        new: Zeroizing<String>,
    },
    RenameAccount(String),
    AccountColor(Color),
    /// Pick a profile photo for the current account (the chooser opens).
    ChooseAccountPhoto,
    RemoveAccountPhoto,
    /// Delete the current account (confirmed) and switch to another one.
    DeleteAccount,
    UpdateFilterLists,
    /// The Data & Downloads page opened: measure the cache and list the sites with data.
    DataPage,
    ChooseDownloadFolder,
    ChoosePlaylistFolder,
    /// Clear what `config.data` selects.
    ClearBrowsingData,
    /// Remove one site's cookies and data, or every site's (`None`).
    RemoveSiteData(Option<String>),
    /// Forget one site's remembered permission decision, or every one (`None`).
    ForgetPermissions(Option<(String, PermissionKind)>),
    OpenPasswords,
    /// Import a CSV file from another browser or manager.
    ImportPasswords(PathBuf),
    /// Restore an Axion export made with `password` (on this or another device).
    RestorePasswords {
        path: PathBuf,
        password: Zeroizing<String>,
    },
    /// Write a portable export; `password` must be the account password.
    ExportPasswords {
        path: PathBuf,
        password: Zeroizing<String>,
    },
    /// Pick a new start page background with the desktop's file chooser.
    ChooseBackground,
    /// Copy this image in as a start page background (and select it).
    AddBackground(PathBuf),
    /// Delete the start page background with this file name.
    RemoveBackground(String),
    /// About: check now (opened from the page, so the result shows in it).
    CheckForUpdates,
    /// About: confirm and install the available update (or open its page, for packages).
    InstallUpdate,
    SkipUpdate,
    RemindLater,
    OpenReleasePage,
    RelaunchNow,
    /// Hardware: copy the GPU diagnostics to the clipboard.
    CopyDiagnostics,
    /// Hardware: relaunch Axion to apply the changed settings.
    RestartAxion,
    /// Data & Downloads: import from another browser or a file.
    Import(ImportRequest),
    /// Hardware: ask before "Shared GPU memory" is used on AMD graphics.
    ConfirmGpuFrames,
    /// The repository changed (`config.updates.repository`); save it and look for updates.
    SourceChanged,
}

pub struct SettingsSheet {
    root: NodeId,
    sheet: NodeId,
    /// The sheet flows out of the glass it was opened from and drains back into it.
    morph: SheetMorph,
    nav: Vec<(Category, NodeId)>,
    content: NodeId,
    /// The clipped area `content` scrolls in.
    viewport: NodeId,
    scroll: f32,
    shown_category: Category,
    category: Category,
    controls: Vec<(NodeId, Setting)>,
    /// The value label beside each slider.
    values: Vec<(Setting, NodeId)>,
    /// The card rows are added to; a new one starts with the next row after `section`.
    group: Option<NodeId>,
    homepage: Option<NodeId>,
    /// Keyboard page: its filter field, the list below it, and the filter text it was built with.
    key_search: Option<NodeId>,
    key_list: Option<NodeId>,
    key_query: String,
    /// The "add a trusted player site" field (Media page).
    trusted_site: Option<NodeId>,
    /// The Playlist's list name field (Media page).
    /// The account password fields (current, new, confirm) where shown.
    password_fields: Option<PasswordFields>,
    account_name: Option<NodeId>,
    account: AccountInfo,
    /// The last result of setting the account password (an error, or that it worked).
    account_status: Option<String>,
    /// Why the last profile photo was refused (or nothing).
    photo_status: Option<String>,
    confirm_delete: bool,
    /// The page must be rebuilt after a change (a toggle that shows more rows).
    rebuild: bool,
    /// The import/export file and password fields (Passwords page) and the last
    /// import/export result.
    passwords_file: Option<NodeId>,
    passwords_secret: Option<NodeId>,
    passwords_status: Option<String>,
    /// The browser's memory use, shown on the Tabs page.
    memory_used: Option<u64>,
    confirm_clear: bool,
    /// Data page: the cache's size, the sites with cookies (`None` while unknown) and
    /// which destructive button asks for confirmation.
    cache_size: Option<u64>,
    site_data: Option<Vec<(String, usize)>>,
    confirm_clear_data: bool,
    confirm_remove_all: bool,
    /// Privacy: the remembered permission decisions (origin, kind, allowed) and whether "Reset
    /// All" asks for confirmation.
    site_permissions: Vec<(String, PermissionKind, bool)>,
    confirm_reset_permissions: bool,
    /// Data page: the browsers to import from and what the import group shows.
    import: ImportView,
    open: bool,
    /// The search field above the categories, and what it held when the page was built.
    search: NodeId,
    query: String,
    /// The start page's backgrounds, the selected one's preview (its name and picture), the
    /// preview's picture and dimming nodes, the path field, and the last adding problem.
    backgrounds: Vec<BackgroundCard>,
    background_preview: Option<(String, Image)>,
    preview_node: Option<NodeId>,
    background_path: Option<NodeId>,
    background_status: Option<String>,
    /// The start page's last "could not add" message that `background_status` followed.
    background_error: Option<String>,
    /// About & Updates: what the page shows, the repository field and its last message, the
    /// release notes box (clipped viewport, scrolling content) with its scroll, and the dot
    /// beside the page's entry in the list.
    about: AboutInfo,
    /// What the GPU session uses (Hardware page); `None` before the browser set it.
    hardware: Option<crate::hardware::Status>,
    repository_field: Option<NodeId>,
    repository_status: Option<String>,
    /// What was typed in the repository field when it was refused (put back after a redraw).
    repository_draft: Option<String>,
    notes: Option<(NodeId, NodeId)>,
    notes_scroll: f32,
    about_badge: NodeId,
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn padding(h: f32, v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(h), right: length(h), top: length(v), bottom: length(v) }
}

fn grow_label() -> Style {
    Style {
        flex_grow: 1.0,
        flex_shrink: 1.0,
        min_size: Size { width: length(0.0), height: auto() },
        ..Default::default()
    }
}

impl SettingsSheet {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(SettingsAction::Close.id()),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        let sheet = ui.add(
            root,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_radius(SHEET_RADIUS),
            Style { position: Position::Absolute, flex_direction: FlexDirection::Row, ..Default::default() },
        );
        let nav_column = ui.add(
            sheet,
            Widget::container(),
            Style {
                size: Size { width: length(NAV_WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(2.0) },
                padding: padding(14.0, 22.0),
                ..Default::default()
            },
        );
        let title = ui.add_label(
            nav_column,
            tr("Settings"),
            TextStyle::Title1,
            Style {
                margin: Rect {
                    left: length(12.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(14.0),
                },
                ..Default::default()
            },
        );
        let _ = title;
        let search_box = ui.add(
            nav_column,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(36.0) },
                flex_shrink: 0.0,
                padding: padding(12.0, 0.0),
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(10.0),
                },
                ..row(8.0)
            },
        );
        ui.add(search_box, Widget::icon(Icon::Search, 15.0).with_paint(Paint::Secondary), Style::default());
        let search = ui.add_text_field(
            search_box,
            tr("Search"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let mut nav = Vec::new();
        let mut about_badge = None;
        for category in Category::ALL {
            let item = ui.add(
                nav_column,
                Widget::container().with_action(SettingsAction::Category(category).id()).with_radius(CAPSULE),
                Style {
                    size: Size { width: auto(), height: length(42.0) },
                    padding: padding(12.0, 0.0),
                    ..row(12.0)
                },
            );
            ui.add(item, Widget::icon(category.icon(), 19.0).with_paint(Paint::Accent), Style::default());
            ui.add_label(item, category.title(), TextStyle::Body, grow_label());
            if category == Category::About {
                let dot = ui.add(
                    item,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Fixed(UPDATE_DOT), 1.0))
                        .with_radius(CAPSULE)
                        .without_feedback(),
                    Style {
                        size: Size { width: length(9.0), height: length(9.0) },
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                );
                ui.set_visible(dot, false);
                about_badge = Some(dot);
            }
            nav.push((category, item));
        }
        let about_badge = about_badge.unwrap_or(search);
        // The page scrolls inside a clipped viewport.
        let viewport = ui.add(
            sheet,
            Widget::container().with_clip(),
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0),
                flex_direction: FlexDirection::Column,
                // The page keeps its natural height (taller than the viewport when it scrolls).
                justify_content: JustifyContent::START,
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
        );
        let content = ui.add(
            viewport,
            Widget::container(),
            Style {
                flex_shrink: 0.0,
                size: Size { width: percent(1.0), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(4.0) },
                padding: padding(28.0, 26.0),
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        let morph = SheetMorph::build(sheet, root, SHEET_RADIUS, BACKDROP_DIM);
        Self {
            root,
            sheet,
            morph,
            nav,
            content,
            viewport,
            scroll: 0.0,
            shown_category: Category::General,
            category: Category::General,
            controls: Vec::new(),
            values: Vec::new(),
            homepage: None,
            key_search: None,
            key_list: None,
            key_query: String::new(),
            trusted_site: None,
            password_fields: None,
            account_name: None,
            account: AccountInfo::default(),
            account_status: None,
            photo_status: None,
            confirm_delete: false,
            rebuild: false,
            passwords_file: None,
            passwords_secret: None,
            passwords_status: None,
            memory_used: None,
            group: None,
            confirm_clear: false,
            cache_size: None,
            site_data: None,
            confirm_clear_data: false,
            confirm_remove_all: false,
            site_permissions: Vec::new(),
            confirm_reset_permissions: false,
            import: ImportView::default(),
            open: false,
            search,
            query: String::new(),
            backgrounds: Vec::new(),
            background_preview: None,
            preview_node: None,
            background_path: None,
            background_status: None,
            background_error: None,
            about: AboutInfo::default(),
            hardware: None,
            repository_field: None,
            repository_status: None,
            repository_draft: None,
            notes: None,
            notes_scroll: 0.0,
            about_badge,
        }
    }

    /// What the Hardware page reports.
    pub fn set_hardware(&mut self, status: crate::hardware::Status) {
        self.hardware = Some(status);
    }

    /// What the About & Updates page shows; call [`Self::show`] to redraw it.
    pub fn set_about(&mut self, ui: &mut Ui, about: AboutInfo) {
        let waiting = matches!(about.state, AboutState::Available { .. } | AboutState::Installed { .. })
            && !about.skipped;
        ui.set_visible(self.about_badge, waiting);
        self.about = about;
    }

    pub fn repository_field(&self) -> Option<NodeId> {
        self.repository_field
    }

    /// The start page's backgrounds, listed on the General page.
    pub fn set_backgrounds(&mut self, cards: Vec<BackgroundCard>) {
        self.backgrounds = cards;
    }

    /// The selected background's preview (blurred like the page), shown at once.
    pub fn set_background_preview(&mut self, ui: &mut Ui, preview: Option<(String, Image)>) {
        self.background_preview = preview;
        if let (Some(node), Some((_, image))) = (self.preview_node, &self.background_preview) {
            ui.set_image(node, Some(image.clone()));
            ui.set_image_crop(node, Some(cover_crop(image.1, BACKGROUND_PREVIEW)));
        }
    }

    /// Shows why the last background could not be added (or nothing).
    /// Follows the start page's "could not add" message when it changes (a success clears it).
    pub fn sync_background_status(&mut self, error: Option<String>) {
        if self.background_error != error {
            self.background_status.clone_from(&error);
            self.background_error = error;
        }
    }

    pub fn set_background_status(&mut self, status: Option<String>) {
        self.background_status = status;
    }

    pub fn background_path_field(&self) -> Option<NodeId> {
        self.background_path
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether `p` (logical px) is on the sheet itself (backdrop clicks outside close it).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.sheet).contains(p)
    }

    /// The current account, shown on the Account page (and whether it has a password, which
    /// changes the Privacy page).
    pub fn set_account(&mut self, account: AccountInfo) {
        self.account = account;
    }

    /// Why the last profile photo was refused (or nothing), shown under the photo buttons.
    pub fn set_photo_status(&mut self, status: Option<String>) {
        self.photo_status = status;
    }

    /// The result of setting or changing the account password.
    pub fn set_account_status(&mut self, status: Option<String>) {
        self.account_status = status;
    }

    /// The field whose Enter sets the password (the last of them).
    pub fn password_field(&self) -> Option<NodeId> {
        self.password_fields.as_ref().map(|f| f.confirm)
    }

    pub fn account_name_field(&self) -> Option<NodeId> {
        self.account_name
    }

    /// Whether the page should be rebuilt after the last change (then clears the flag).
    pub fn take_rebuild(&mut self) -> bool {
        std::mem::take(&mut self.rebuild)
    }

    pub fn trusted_site_field(&self) -> Option<NodeId> {
        self.trusted_site
    }

    /// The Keyboard page's shortcut filter field (while that page is shown).
    pub fn key_search_field(&self) -> Option<NodeId> {
        self.key_search
    }

    pub fn homepage_field(&self) -> Option<NodeId> {
        self.homepage
    }

    /// Shows the result of the last password import or export (Passwords page).
    pub fn set_passwords_status(&mut self, status: String) {
        self.passwords_status = Some(status);
    }

    /// Opens out of the glass at `origin` (or `fallback`, the address field, if none is there).
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        keymap: &Keymap,
        window: Vec2,
        origin: Origin,
        fallback: Origin,
    ) {
        self.open = true;
        ui.set_visible(self.root, true);
        let target = self.resize(ui, window);
        self.show(ui, config, keymap);
        self.morph.open(ui, config, origin, fallback, target, true);
    }

    /// Advances the sheet's flow in or out; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        let phase = self.morph.animate(ui, dt);
        if !self.open && phase == crate::drip::Phase::Closed && ui.is_visible(self.root) {
            ui.set_visible(self.root, false);
        }
    }

    pub fn set_memory_used(&mut self, used: Option<u64>) {
        self.memory_used = used;
    }

    pub fn category(&self) -> Category {
        self.category
    }

    pub fn set_cache_size(&mut self, size: Option<u64>) {
        self.cache_size = size;
    }

    /// The sites with cookies and how many each has (`None`: being listed).
    /// The remembered permission decisions Privacy lists.
    pub fn set_site_permissions(&mut self, decisions: Vec<(String, PermissionKind, bool)>) {
        self.site_permissions = decisions;
    }

    pub fn set_site_data(&mut self, sites: Option<Vec<(String, usize)>>) {
        self.site_data = sites;
    }

    /// Scrolls the page by `dy` logical px (wheel: positive scrolls up).
    pub fn scroll_by(&mut self, ui: &mut Ui, dy: f32) {
        let max = (ui.frame(self.content).size.y - ui.frame(self.viewport).size.y).max(0.0);
        let scroll = (self.scroll - dy).clamp(0.0, max);
        self.set_scroll(ui, scroll);
    }

    fn set_scroll(&mut self, ui: &mut Ui, scroll: f32) {
        self.scroll = scroll;
        let mut style = ui.style(self.content);
        style.margin =
            Rect { left: length(0.0), right: length(0.0), top: length(-scroll), bottom: length(0.0) };
        ui.set_style(self.content, style);
    }

    pub fn search_field(&self) -> NodeId {
        self.search
    }

    /// Rebuilds the page as search results when the search text changed. Call every frame.
    pub fn poll_search(&mut self, ui: &mut Ui, config: &Config, keymap: &Keymap) {
        if !self.open {
            return;
        }
        if self.query.is_empty()
            && let (Some(field), Some(list)) = (self.key_search, self.key_list)
        {
            let text = ui.field_text(field).unwrap_or_default().trim().to_owned();
            if text != self.key_query {
                self.key_query = text;
                self.fill_shortcuts(ui, list, keymap);
            }
        }
        let text = ui.field_text(self.search).unwrap_or_default().trim().to_owned();
        if text != self.query {
            self.query = text;
            self.set_scroll(ui, 0.0);
            self.show(ui, config, keymap);
        }
    }

    /// Shows this category next time the sheet opens (or now, if open).
    pub fn set_category(&mut self, category: Category) {
        self.category = category;
    }

    /// Drains back into the glass it came from (or `fallback`, if that is gone).
    pub fn close(&mut self, ui: &mut Ui, config: &Config, fallback: Origin) {
        if !self.open {
            return;
        }
        self.open = false;
        self.query.clear();
        self.key_query.clear();
        ui.set_field_text(self.search, "");
        self.confirm_clear = false;
        self.confirm_clear_data = false;
        self.confirm_remove_all = false;
        self.confirm_delete = false;
        self.account_status = None;
        self.morph.close(ui, config, fallback);
    }

    /// Lays the sheet out for `window`; returns its frame (logical px).
    pub fn resize(&mut self, ui: &mut Ui, window: Vec2) -> Frame {
        let size = (window - Vec2::splat(2.0 * SHEET_MARGIN)).min(SHEET_MAX).max(Vec2::new(480.0, 360.0));
        let origin = ((window - size) / 2.0).max(Vec2::ZERO);
        let mut style = ui.style(self.sheet);
        style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
        style.size = Size { width: length(size.x), height: length(size.y) };
        ui.set_style(self.sheet, style);
        let frame = Frame { origin, size };
        self.morph.retarget(frame);
        frame
    }

    /// Rebuilds the current category's page from `config`.
    pub fn show(&mut self, ui: &mut Ui, config: &Config, keymap: &Keymap) {
        for &(category, node) in &self.nav {
            if let Some(w) = ui.widget_mut(node) {
                w.background = if category == self.category && self.query.is_empty() {
                    Background::Fill(Paint::Accent, 0.14)
                } else {
                    Background::None
                };
            }
        }
        ui.clear_children(self.content);
        self.controls.clear();
        self.values.clear();
        self.group = None;
        if self.category != self.shown_category {
            self.shown_category = self.category;
            self.set_scroll(ui, 0.0);
        }
        self.homepage = None;
        self.key_search = None;
        self.key_list = None;
        self.trusted_site = None;
        self.password_fields = None;
        self.account_name = None;
        self.passwords_file = None;
        self.passwords_secret = None;
        self.preview_node = None;
        self.background_path = None;
        self.repository_field = None;
        self.notes = None;
        if !self.query.is_empty() {
            self.show_results(ui, config);
            return;
        }
        ui.add_label(
            self.content,
            self.category.title(),
            TextStyle::Title1,
            Style {
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(12.0),
                },
                ..Default::default()
            },
        );
        match self.category {
            Category::General => {
                let row_node = self.row(ui, tr("Homepage"));
                let field_box = ui.add(
                    row_node,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Label, 0.06))
                        .with_radius(12.0),
                    Style {
                        size: Size { width: auto(), height: length(38.0) },
                        padding: padding(12.0, 0.0),
                        ..row(0.0)
                    },
                );
                let field = ui.add_text_field(
                    field_box,
                    tr("Axion Home"),
                    TextStyle::Body,
                    Style { size: Size { width: length(320.0), height: length(22.0) }, ..Default::default() },
                );
                ui.set_field_text(field, config.general.homepage_field_text());
                self.homepage = Some(field);
                let row_node = self.row(ui, tr("Search engine"));
                let current = SEARCH_ENGINES.iter().position(|(_, url)| *url == config.general.search_url);
                let options: Vec<(&str, ActionId)> = SEARCH_ENGINES
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| (*name, SettingsAction::SearchEngine(i).id()))
                    .collect();
                segmented(ui, row_node, &options, current);
                // The language names stay as they are (each in its own language).
                let row_node = self.row(ui, tr("Language"));
                let languages: Vec<(&str, ActionId)> = LANGUAGES
                    .iter()
                    .map(|&(name, language)| (name, SettingsAction::Language(language).id()))
                    .collect();
                let current = LANGUAGES.iter().position(|&(_, l)| l == config.general.language);
                segmented(ui, row_node, &languages, current);
                self.note(ui, tr("Applies after restarting Axion."));
                if cfg!(target_os = "linux") {
                    self.control(ui, config, Setting::AppMenuEntry);
                    self.note(
                        ui,
                        tr("Gives Axion its icon in the taskbar and the application menu. Applies after restarting Axion."),
                    );
                }
                let row_node = self.row(ui, tr("Welcome tour"));
                glass_pill(ui, row_node, tr("Show Welcome Tour"), SettingsAction::ShowWelcomeTour);
                self.control(ui, config, Setting::HomeButton);
                self.section(ui, tr("Start page"));
                for setting in [
                    Setting::HomeClock,
                    Setting::HomeGreeting,
                    Setting::HomeSearch,
                    Setting::HomeTopSites,
                    Setting::HomeStats,
                ] {
                    self.control(ui, config, setting);
                }
                self.section(ui, tr("Start page background"));
                self.background_picker(ui, config);
                if self.backgrounds.iter().any(|c| c.name == config.home.background) {
                    self.control_labeled(ui, config, Setting::HomeDim, tr("Dim"));
                    self.control_labeled(ui, config, Setting::HomeBlur, tr("Blur"));
                }
                self.control(ui, config, Setting::HomeGlassGround);
                self.control(ui, config, Setting::HomeLive);
                self.control(ui, config, Setting::HomeLiveIgnoreReduceMotion);
                self.note(ui, tr("Off: wallpapers show a still frame while Reduce Motion is on."));
                let row_node = self.row(ui, tr("Add from file"));
                self.background_path = Some(text_field(ui, row_node, "~/Pictures/sea.jpg", 220.0, false));
                pill_button(ui, row_node, tr("Add"), SettingsAction::AddBackgroundPath, false);
                let note = match &self.background_status {
                    Some(status) => status.clone(),
                    None => tr("Images, animated images (GIF, WebP) and videos (WebM, MP4) are copied into \
                             your profile. Videos play muted and loop; MP4 needs H.264 support. The \
                             gear on the start page picks them too.")
                    .to_owned(),
                };
                self.note(ui, &note);
                if config.glass.reduce_motion
                    && config.home.live_wallpapers
                    && !config.home.live_wallpapers_ignore_reduce_motion
                {
                    self.note(
                        ui,
                        tr("Reduce Motion is on (Appearance), so animated wallpapers and videos show a still \
                         frame."),
                    );
                }
                self.section(ui, tr("Developer"));
                self.control(ui, config, Setting::DevTools);
                self.note(
                    ui,
                    tr("Chromium's own developer tools (F12, Ctrl+Shift+I) open in a window of their own; \
                     Ctrl+U shows the page source. Off, none of these shortcuts do anything."),
                );
            }
            Category::Appearance => {
                let row_node = self.row(ui, tr("Appearance"));
                let themes = [
                    (tr("Automatic"), SettingsAction::Theme(ThemeMode::Auto).id()),
                    (tr("Light"), SettingsAction::Theme(ThemeMode::Light).id()),
                    (tr("Dark"), SettingsAction::Theme(ThemeMode::Dark).id()),
                ];
                let current = [ThemeMode::Auto, ThemeMode::Light, ThemeMode::Dark]
                    .iter()
                    .position(|&m| m == config.appearance.theme);
                segmented(ui, row_node, &themes, current);
                let row_node = self.row(ui, tr("Glass"));
                let styles = [
                    (tr("Liquid Glass"), SettingsAction::GlassStyle(GlassStyle::Liquid).id()),
                    (tr("Flat"), SettingsAction::GlassStyle(GlassStyle::Flat).id()),
                ];
                let current =
                    [GlassStyle::Liquid, GlassStyle::Flat].iter().position(|&m| m == config.glass.style);
                segmented(ui, row_node, &styles, current);
                self.note(
                    ui,
                    tr("Flat: solid cards, no refraction, blur or bounce. Skips the glass work (fast on weak GPUs)."),
                );
                let row_node = self.row(ui, tr("Accent colour"));
                swatches(ui, row_node, config.appearance.accent, SettingsAction::Accent);
                let row_node = self.row(ui, tr("Icon weight"));
                let weights: Vec<(&str, ActionId)> = ICON_WEIGHTS
                    .iter()
                    .enumerate()
                    .map(|(i, &(name, ..))| (tr(name), SettingsAction::IconWeight(i).id()))
                    .collect();
                segmented(ui, row_node, &weights, icon_weight_preset(config));
                for setting in
                    [Setting::AdaptToPage, Setting::TextScale, Setting::PageCorner, Setting::FloatSidebar]
                {
                    self.control(ui, config, setting);
                }
            }
            Category::LiquidGlass => {
                for setting in [
                    Setting::ReduceTransparency,
                    Setting::ReduceMotion,
                    Setting::Refraction,
                    Setting::Blur,
                    Setting::Dispersion,
                    Setting::Squash,
                    Setting::DragStretch,
                    Setting::MergeDistance,
                ] {
                    self.control(ui, config, setting);
                }
            }
            Category::Keyboard => {
                let note = ui.add_label(
                    self.content,
                    tr("Change shortcuts in ~/.config/axion/keymap.toml - edits apply immediately."),
                    TextStyle::Footnote,
                    Style {
                        margin: Rect {
                            left: length(0.0),
                            right: length(0.0),
                            top: length(0.0),
                            bottom: length(8.0),
                        },
                        ..Default::default()
                    },
                );
                if let Some(w) = ui.widget_mut(note) {
                    w.set_paint(Paint::Secondary);
                }
                let row_node = ui.add(
                    self.content,
                    Widget::container(),
                    Style {
                        flex_shrink: 0.0,
                        margin: Rect {
                            left: length(0.0),
                            right: length(0.0),
                            top: length(0.0),
                            bottom: length(4.0),
                        },
                        ..row(0.0)
                    },
                );
                let field = text_field(ui, row_node, tr("Search shortcuts"), 320.0, false);
                ui.set_field_text(field, &self.key_query);
                self.key_search = Some(field);
                let list = ui.add(
                    self.content,
                    Widget::container(),
                    Style { flex_direction: FlexDirection::Column, flex_shrink: 0.0, ..Default::default() },
                );
                self.key_list = Some(list);
                self.fill_shortcuts(ui, list, keymap);
            }
            Category::Tabs => {
                self.tab_bar_choice(ui, config);
                self.control(ui, config, Setting::TintFromPage);
                if config.appearance.tint_from_page {
                    let row_node = self.row(ui, tr("Page colour"));
                    let options: Vec<(&str, ActionId)> = TINT_MODES
                        .iter()
                        .map(|(name, mode)| (tr(name), SettingsAction::TintMode(*mode).id()))
                        .collect();
                    let current = TINT_MODES.iter().position(|(_, m)| *m == config.appearance.tint_mode);
                    segmented(ui, row_node, &options, current);
                }
                self.control(ui, config, Setting::OpenInBackground);
                self.control(ui, config, Setting::CloseAnimation);
                self.control(ui, config, Setting::KeepTabWidths);
                self.control(ui, config, Setting::MiddleClickClosesTab);
                self.control(ui, config, Setting::NewTabJoinsGroup);
                self.control(ui, config, Setting::GroupChipsInBar);
                self.control(ui, config, Setting::ShowAudioIndicator);
                self.control(ui, config, Setting::RememberSiteMute);
                self.control(ui, config, Setting::DragOutLinks);
                self.control(ui, config, Setting::DragOutImage);
                let row_node = self.row(ui, tr("Close tabs"));
                let options: Vec<(&str, ActionId)> = CLOSE_AFTER
                    .iter()
                    .map(|(name, days)| (tr(name), SettingsAction::CloseAfter(*days).id()))
                    .collect();
                let current = CLOSE_AFTER.iter().position(|(_, d)| *d == config.tabs.close_after_days);
                segmented(ui, row_node, &options, current);
                self.note(ui, tr("Axion can close tabs you haven't looked at for a while."));
                let row_node = self.row(ui, tr("Bookmarks bar"));
                let options: Vec<(&str, ActionId)> = BOOKMARKS_BAR
                    .iter()
                    .map(|(name, mode)| (tr(name), SettingsAction::BookmarksBar(*mode).id()))
                    .collect();
                let current = BOOKMARKS_BAR.iter().position(|(_, m)| *m == config.layout.bookmarks_bar);
                segmented(ui, row_node, &options, current);
                self.section(ui, tr("Memory"));
                for setting in [
                    Setting::FreezeAfter,
                    Setting::DiscardAfter,
                    Setting::LimitMemory,
                    Setting::MemoryBudget,
                    Setting::KeepMediaAwake,
                    Setting::RestoreSession,
                ] {
                    self.control(ui, config, setting);
                }
                let budget = crate::lifecycle::memory_budget(config).map_or(tr("none").into(), format_bytes);
                let used = self.memory_used.map_or(tr("not measured yet").into(), format_bytes);
                let row_node = self
                    .row(ui, &tr_fmt!("In use now: {used} · budget: {budget}", used = used, budget = budget));
                if let Some(label) = ui.children(row_node).first().copied()
                    && let Some(w) = ui.widget_mut(label)
                {
                    w.set_paint(Paint::Secondary);
                }
            }
            Category::Media => {
                for setting in [Setting::OwnFullscreenControls, Setting::HideControlsAfter] {
                    self.control(ui, config, setting);
                }
                self.note(ui, tr("Pages keep their own player; in fullscreen, Axion's takes over."));
                self.section(ui, tr("Playlist"));
                for setting in [
                    Setting::PlaylistQuickAction,
                    Setting::PlaylistAutoDownload,
                    Setting::PlaylistDeleteFiles,
                ] {
                    self.control(ui, config, setting);
                }
                let folder = if config.playlist.directory.trim().is_empty() {
                    tr("Axion data folder").to_owned()
                } else {
                    crate::downloads::display_path(std::path::Path::new(config.playlist.directory.trim()))
                };
                let row_node = self.row(ui, tr("Playlist sort"));
                let options: Vec<(&str, ActionId)> = crate::playlist::SORTS
                    .iter()
                    .map(|(sort, name, _)| (tr(name), SettingsAction::PlaylistSort(*sort).id()))
                    .collect();
                let current = crate::playlist::SORTS.iter().position(|(s, ..)| *s == config.playlist.sort);
                segmented(ui, row_node, &options, current);
                let row_node = self.row(ui, tr("Playlist downloads"));
                let label = ui.add_label(row_node, &folder, TextStyle::Callout, Style::default());
                if let Some(w) = ui.widget_mut(label) {
                    w.set_paint(Paint::Secondary);
                }
                pill_button(ui, row_node, tr("Change…"), SettingsAction::ChoosePlaylistFolder, false);
                if !config.playlist.directory.trim().is_empty() {
                    pill_button(ui, row_node, tr("Default"), SettingsAction::ResetPlaylistFolder, false);
                }
                self.note(ui, tr("Plain video files can be downloaded; streams are watched online only."));
                self.section(ui, tr("Sites that keep their own player"));
                for (i, site) in config.media.trusted_player_sites.iter().enumerate() {
                    let row_node = self.row(ui, site);
                    let remove = ui.add(
                        row_node,
                        Widget::container()
                            .with_action(SettingsAction::RemoveTrustedSite(i).id())
                            .with_radius(CAPSULE)
                            .with_background(Background::Glass(GlassLevel::Thin)),
                        Style {
                            size: Size { width: length(28.0), height: length(28.0) },
                            justify_content: JustifyContent::CENTER,
                            ..row(0.0)
                        },
                    );
                    ui.add(
                        remove,
                        Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary),
                        Style::default(),
                    );
                }
                let row_node = self.row(ui, tr("Add a site"));
                let field_box = ui.add(
                    row_node,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Label, 0.06))
                        .with_radius(12.0),
                    Style {
                        size: Size { width: auto(), height: length(38.0) },
                        padding: padding(12.0, 0.0),
                        ..row(0.0)
                    },
                );
                let field = ui.add_text_field(
                    field_box,
                    "example.com",
                    TextStyle::Body,
                    Style { size: Size { width: length(220.0), height: length(22.0) }, ..Default::default() },
                );
                self.trusted_site = Some(field);
                pill_button(ui, row_node, tr("Add"), SettingsAction::AddTrustedSite, false);
                self.note(ui, tr("Also in fullscreen, with their subdomains. Applies to pages loaded next."));
                self.section(ui, tr("Playback"));
                for setting in [Setting::HardwareDecoding, Setting::DesktopIntegration] {
                    self.control(ui, config, setting);
                }
            }
            Category::Hardware => self.hardware_page(ui, config),
            Category::Reading => self.reading_page(ui, config),
            Category::Shields => {
                let row_node = self.row(ui, tr("Shields on sites you haven't set"));
                let options: Vec<(&str, ActionId)> = SHIELD_LEVELS
                    .iter()
                    .map(|(name, l)| (tr(name), SettingsAction::ShieldLevel(*l).id()))
                    .collect();
                let current = SHIELD_LEVELS.iter().position(|(_, l)| *l == config.shields.default_level);
                segmented(ui, row_node, &options, current);
                for setting in [
                    Setting::HideAdElements,
                    Setting::BlockPopups,
                    Setting::BlockRedirects,
                    Setting::StripTracking,
                    Setting::ThirdPartyCookies,
                ] {
                    self.control(ui, config, setting);
                }
                let hours = config.shields.update_interval_hours;
                let label = tr_fmt!(
                    "Filter lists ({count}, refreshed every {hours} hours)",
                    count = config.shields.lists.len(),
                    hours = format!("{hours:.0}")
                );
                let row_node = self.row(ui, &label);
                pill_button(ui, row_node, tr("Update Now"), SettingsAction::UpdateFilterLists, false);
            }
            Category::Privacy => {
                self.control(ui, config, Setting::LockPrivateTabs);
                if config.privacy.lock_private_tabs && !self.account.has_password {
                    // The lock needs the account password: offer to create it right here.
                    self.password_rows(ui);
                    let note = self.account_status.clone().unwrap_or_else(|| {
                        tr_fmt!(
                            "Create a password for “{name}” to lock private tabs. It also protects Passwords.",
                            name = self.account.name
                        )
                    });
                    self.note(ui, &note);
                    self.section(ui, "");
                }
                let row_node = self.row(ui, tr("Browsing history"));
                let text =
                    if self.confirm_clear { tr("Click again to clear") } else { tr("Clear History…") };
                let button = ui.add(
                    row_node,
                    Widget::container()
                        .with_action(SettingsAction::ClearHistory.id())
                        .with_radius(CAPSULE)
                        .with_background(if self.confirm_clear {
                            Background::TintedGlass(GlassLevel::Thin, Paint::Fixed(Color::rgb(0xE2453A)))
                        } else {
                            Background::Glass(GlassLevel::Thin)
                        }),
                    Style {
                        size: Size { width: auto(), height: length(36.0) },
                        padding: padding(16.0, 0.0),
                        ..row(0.0)
                    },
                );
                let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
                if let Some(w) = ui.widget_mut(label) {
                    w.set_paint(if self.confirm_clear {
                        Paint::Fixed(Color::rgb(0xFFFFFF))
                    } else {
                        Paint::Fixed(Color::rgb(0xD0362B))
                    });
                }
                self.site_permissions_section(ui, config);
                self.section(ui, tr("Extensions"));
                self.control(ui, config, Setting::ExtensionsEnabled);
                self.control(ui, config, Setting::ExtensionsWebStore);
                self.control(ui, config, Setting::ExtensionsAutoUpdate);
                self.note(
                    ui,
                    tr("Manifest V3 extensions install from the Chrome Web Store (Axion asks first and shows every permission), or from the extensions button in the toolbar (an unpacked folder or a .crx file). Updates for Web Store extensions are downloaded in the background and apply after a restart."),
                );
            }
            Category::Passwords => {
                let row_node = self.row(ui, tr("Saved passwords, cards and addresses"));
                pill_button(ui, row_node, tr("Open Passwords"), SettingsAction::OpenPasswords, false);
                for setting in [Setting::OfferToSavePasswords, Setting::AutofillPasswords] {
                    self.control(ui, config, setting);
                }
                self.section(ui, tr("Security"));
                self.control(ui, config, Setting::ClearClipboardAfter);
                self.note(
                    ui,
                    tr("Encrypted with your account password and a key that never leaves this device."),
                );
                self.section(ui, tr("Import and export"));
                let row_node = self.row(ui, tr("File"));
                let field_box = ui.add(
                    row_node,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Label, 0.06))
                        .with_radius(12.0),
                    Style {
                        size: Size { width: auto(), height: length(38.0) },
                        padding: padding(12.0, 0.0),
                        ..row(0.0)
                    },
                );
                let field = ui.add_text_field(
                    field_box,
                    "~/passwords.csv",
                    TextStyle::Body,
                    Style { size: Size { width: length(260.0), height: length(22.0) }, ..Default::default() },
                );
                self.passwords_file = Some(field);
                let row_node = self.row(ui, tr("Password"));
                self.passwords_secret =
                    Some(text_field(ui, row_node, tr("For exports and restoring"), 260.0, true));
                let row_node = self.button_row(ui);
                pill_button(ui, row_node, tr("Import CSV"), SettingsAction::ImportPasswords, false);
                pill_button(
                    ui,
                    row_node,
                    tr("Restore Axion Export"),
                    SettingsAction::RestorePasswords,
                    false,
                );
                pill_button(
                    ui,
                    row_node,
                    tr("Export Encrypted Copy"),
                    SettingsAction::ExportPasswords,
                    false,
                );
                let note = self.passwords_status.clone().unwrap_or_else(|| {
                    tr("Imports CSV from Chrome, Firefox or Bitwarden. Exports restore on any device.")
                        .to_owned()
                });
                self.note(ui, &note);
            }
            Category::Data => self.data_page(ui, config),
            Category::Account => self.account_page(ui),
            Category::About => self.about_page(ui, config),
        }
    }

    /// Settings › Account: the current account's name, avatar colour and password, and
    /// deleting it.
    fn account_page(&mut self, ui: &mut Ui) {
        let account = self.account.clone();
        let parent = self.group_for_row(ui);
        let header = ui.add(
            parent,
            Widget::container(),
            Style {
                size: Size { width: auto(), height: length(AVATAR_SIZE + 24.0) },
                flex_shrink: 0.0,
                ..row(14.0)
            },
        );
        avatar(ui, header, &account.initials, account.color, AVATAR_SIZE, account.picture.as_ref());
        let names = ui.add(
            header,
            Widget::container(),
            Style { flex_direction: FlexDirection::Column, ..grow_label() },
        );
        ui.add_label(names, &account.name, TextStyle::Title2, Style::default());
        let detail = match account.count {
            0 | 1 => tr("The only account on this computer").to_owned(),
            n => tr_fmt!("One of {count} accounts on this computer", count = n),
        };
        let label = ui.add_label(names, &detail, TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
        self.note(
            ui,
            tr("Each account has its own tabs, history, bookmarks, passwords, website data and settings."),
        );

        self.section(ui, tr("Profile"));
        let row_node = self.row(ui, tr("Name"));
        let field = text_field(ui, row_node, tr("Name"), 220.0, false);
        ui.set_field_text(field, &account.name);
        self.account_name = Some(field);
        pill_button(ui, row_node, tr("Save"), SettingsAction::SaveAccountName, false);
        let row_node = self.row(ui, tr("Photo"));
        avatar(ui, row_node, &account.initials, account.color, PHOTO_PREVIEW, account.picture.as_ref());
        glass_pill(ui, row_node, tr("Choose Photo…"), SettingsAction::ChooseAccountPhoto);
        if account.picture.is_some() {
            glass_pill(ui, row_node, tr("Remove Photo"), SettingsAction::RemoveAccountPhoto);
        }
        if let Some(status) = self.photo_status.clone() {
            self.note(ui, &status);
        }
        let row_node = self.row(ui, tr("Colour"));
        swatches(ui, row_node, account.color, SettingsAction::AccountColor);

        self.section(ui, tr("Password"));
        self.password_rows(ui);
        match self.account_status.clone() {
            Some(status) => self.note(ui, &status),
            None => {
                self.note(
                    ui,
                    tr("One password for the whole account: it unlocks Passwords and private tabs."),
                );
                self.note(ui, tr("Axion can't recover it if you forget it."));
            }
        }

        self.section(ui, tr("Delete"));
        if account.count > 1 {
            let row_node = self.row(ui, tr("Delete this account and its data"));
            let text =
                if self.confirm_delete { tr("Click Again to Delete") } else { tr("Delete Account…") };
            destructive_button(ui, row_node, text, SettingsAction::DeleteAccount, self.confirm_delete);
            self.note(ui, tr("Removes its tabs, history, bookmarks, passwords, website data and settings."));
            self.note(ui, tr("Axion then opens another account."));
        } else {
            self.row(ui, tr("The only account can't be deleted"));
            self.note(
                ui,
                tr("Add another account first, with the account button at the bottom of the sidebar."),
            );
        }
    }

    /// The fields to set the account password, or to change it.
    /// Settings › About & Updates: the app, the update status with the available release's
    /// notes and its buttons, and the update preferences.
    fn about_page(&mut self, ui: &mut Ui, config: &Config) {
        let about = self.about.clone();
        let parent = self.group_for_row(ui);
        let header = ui.add(
            parent,
            Widget::container(),
            Style {
                min_size: Size { width: auto(), height: length(ABOUT_ICON + 28.0) },
                flex_shrink: 0.0,
                padding: padding(0.0, 14.0),
                ..row(16.0)
            },
        );
        let icon_box = ui.add(
            header,
            Widget::container()
                .with_background(Background::Fill(Paint::Accent, 0.14))
                .with_radius(ABOUT_ICON_RADIUS),
            Style {
                size: Size { width: length(ABOUT_ICON), height: length(ABOUT_ICON) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        match &about.icon {
            Some(picture) => {
                let node = ui.add(
                    icon_box,
                    Widget::image().with_radius(ABOUT_ICON_RADIUS),
                    Style {
                        position: Position::Absolute,
                        inset: Rect { left: length(0.0), right: auto(), top: length(0.0), bottom: auto() },
                        size: Size { width: length(ABOUT_ICON), height: length(ABOUT_ICON) },
                        ..Default::default()
                    },
                );
                ui.set_image(node, Some(picture.clone()));
            }
            None => {
                ui.add(icon_box, Widget::icon(Icon::Info, 30.0).with_paint(Paint::Accent), Style::default());
            }
        }
        let text = ui.add(
            header,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );
        ui.add_label(text, "Axion", TextStyle::Title2, Style::default());
        let version = if about.build.is_empty() {
            tr_fmt!("Version {version}", version = about.version.as_str())
        } else {
            tr_fmt!(
                "Version {version} ({build})",
                version = about.version.as_str(),
                build = about.build.as_str()
            )
        };
        let secondary = |ui: &mut Ui, text_node: NodeId, line: &str| {
            let label = ui.add_label(text_node, line, TextStyle::Footnote, Style::default());
            ui.set_label_lines(label, 2);
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
        };
        secondary(ui, text, &version);
        let install = match about.mode {
            InstallMode::AppImage => tr("Installed as an AppImage"),
            InstallMode::Source => tr("Built from a Git checkout"),
            InstallMode::Other => tr("Installed with a package manager or by hand"),
        };
        secondary(ui, text, install);

        self.section(ui, tr("Software update"));
        let source_set = !matches!(about.state, AboutState::NoSource);
        let status = match &about.state {
            AboutState::NoSource => tr("No update source set yet").to_owned(),
            AboutState::NeverChecked => tr("Not checked yet").to_owned(),
            AboutState::Checking => tr("Checking for updates…").to_owned(),
            AboutState::UpToDate => tr("Axion is up to date").to_owned(),
            AboutState::Available { version, prerelease: false } => {
                tr_fmt!("Axion {version} is available", version = version.as_str())
            }
            AboutState::Available { version, prerelease: true } => {
                tr_fmt!("Axion {version} (pre-release) is available", version = version.as_str())
            }
            AboutState::Installed { version } => {
                tr_fmt!("Axion {version} is installed. Relaunch to use it.", version = version.as_str())
            }
            AboutState::Error(message) => message.clone(),
        };
        let row_node = self.row(ui, &status);
        match &about.state {
            AboutState::Checking => idle_pill(ui, row_node, tr("Checking…")),
            AboutState::NoSource => {}
            AboutState::Installed { .. } => {
                primary_button(ui, row_node, tr("Relaunch Now"), SettingsAction::RelaunchNow)
            }
            _ if about.busy => {}
            _ => pill_button(ui, row_node, tr("Check for Updates"), SettingsAction::CheckForUpdates, false),
        }
        if source_set {
            let row_node = self.row(ui, tr("Last checked"));
            let when = if about.last_checked.is_empty() {
                tr("Never").to_owned()
            } else {
                about.last_checked.clone()
            };
            let label = ui.add_label(row_node, &when, TextStyle::Callout, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
        }
        if let AboutState::Available { .. } = &about.state {
            if !about.notes.is_empty() {
                self.section(ui, tr("Release notes"));
                self.notes_box(ui, &about.notes);
            }
            let row_node = self.button_row(ui);
            if !about.busy {
                let (text, action) = match about.mode {
                    InstallMode::AppImage => (tr("Update"), SettingsAction::InstallUpdate),
                    InstallMode::Source => (tr("Update from Git"), SettingsAction::InstallUpdate),
                    InstallMode::Other => (tr("Open Release Page"), SettingsAction::OpenReleasePage),
                };
                primary_button(ui, row_node, text, action);
                if !about.skipped {
                    pill_button(ui, row_node, tr("Skip This Version"), SettingsAction::SkipUpdate, false);
                    pill_button(ui, row_node, tr("Remind Me Later"), SettingsAction::RemindLater, false);
                }
                if about.mode != InstallMode::Other {
                    pill_button(ui, row_node, tr("Release Page"), SettingsAction::OpenReleasePage, false);
                }
            }
            if about.skipped {
                self.note(ui, tr("You skipped this version: Axion won't announce it again."));
            }
            if about.mode == InstallMode::Other {
                self.note(
                    ui,
                    tr("Axion was installed by a package manager or by hand. Update it the same way; the release page has the notes and downloads."),
                );
            }
        }
        if !source_set {
            self.note(
                ui,
                tr("Until a repository is set, Axion never contacts an update server. Enter it below, or set updates.manifest_url in config.toml."),
            );
        }

        self.section(ui, tr("Updates"));
        self.control(ui, config, Setting::AutoCheckUpdates);
        self.control(ui, config, Setting::UpdatePrereleases);
        let row_node = self.row(ui, tr("Repository"));
        // The default repository is the placeholder; the field holds only what the user typed.
        let placeholder = if axion_config::DEFAULT_REPOSITORY.is_empty() {
            tr("owner/name")
        } else {
            axion_config::DEFAULT_REPOSITORY
        };
        let field = text_field(ui, row_node, placeholder, 230.0, false);
        let shown = self
            .repository_draft
            .clone()
            .unwrap_or_else(|| config.updates.repository_field_text().to_owned());
        ui.set_field_text(field, &shown);
        self.repository_field = Some(field);
        pill_button(ui, row_node, tr("Save"), SettingsAction::SaveRepository, false);
        let note = self.repository_status.clone().unwrap_or_else(|| {
            tr("Where Axion's releases are published, as owner/name. Leave it empty to use the default. Looking for an update sends nothing but a request for the newest release.")
                .to_owned()
        });
        self.note(ui, &note);
        if !config.updates.manifest_url.trim().is_empty() {
            self.note(
                ui,
                tr("updates.manifest_url is set in config.toml and is used instead of the repository."),
            );
        }
    }

    /// The release notes in a box of their own that scrolls when they are longer.
    fn notes_box(&mut self, ui: &mut Ui, notes: &str) {
        let parent = self.group_for_row(ui);
        let hidden = axion_ui::taffy::Point {
            x: axion_ui::taffy::Overflow::Hidden,
            y: axion_ui::taffy::Overflow::Hidden,
        };
        let viewport = ui.add(
            parent,
            Widget::container().with_clip(),
            Style {
                size: Size { width: auto(), height: length(NOTES_HEIGHT) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::START,
                overflow: hidden,
                ..Default::default()
            },
        );
        let content = ui.add(
            viewport,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: auto() },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(3.0) },
                padding: padding(0.0, 10.0),
                ..Default::default()
            },
        );
        for line in notes.lines() {
            if line.trim().is_empty() {
                ui.add(
                    content,
                    Widget::container(),
                    Style { size: Size { width: auto(), height: length(6.0) }, ..Default::default() },
                );
                continue;
            }
            let label = ui.add_label(
                content,
                line,
                TextStyle::Callout,
                Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
            );
            ui.set_label_lines(label, NOTES_LINE_WRAP);
        }
        self.notes = Some((viewport, content));
        self.notes_scroll = 0.0;
    }

    /// A point inside the release notes box (capture aid), if the page has one.
    pub fn notes_point(&self, ui: &Ui) -> Option<Vec2> {
        self.notes.map(|(viewport, _)| ui.frame(viewport).center())
    }

    /// Scrolls the release notes under the pointer, if there are some that overflow; returns
    /// whether it took the wheel (the page does not scroll then).
    pub fn scroll_notes(&mut self, ui: &mut Ui, dy: f32, pointer: Option<Vec2>) -> bool {
        let (Some((viewport, content)), Some(p)) = (self.notes, pointer) else { return false };
        if !ui.frame(viewport).contains(p) {
            return false;
        }
        let max = (ui.frame(content).size.y - ui.frame(viewport).size.y).max(0.0);
        if max <= 0.0 {
            return false;
        }
        let next = (self.notes_scroll - dy).clamp(0.0, max);
        // At either end the wheel goes on to the page.
        if (next - self.notes_scroll).abs() < f32::EPSILON {
            return false;
        }
        self.notes_scroll = next;
        let mut style = ui.style(content);
        style.margin =
            Rect { left: length(0.0), right: length(0.0), top: length(-next), bottom: length(0.0) };
        ui.set_style(content, style);
        true
    }

    fn password_rows(&mut self, ui: &mut Ui) {
        let current = if self.account.has_password {
            let row_node = self.row(ui, tr("Current password"));
            Some(text_field(ui, row_node, tr("Required"), 220.0, true))
        } else {
            None
        };
        let row_node = self.row(ui, tr("New password"));
        let new = text_field(
            ui,
            row_node,
            &tr_fmt!("At least {count} characters", count = MIN_MASTER_LENGTH),
            220.0,
            true,
        );
        let row_node = self.row(ui, tr("Confirm new password"));
        let confirm = text_field(ui, row_node, tr("Repeat it"), 220.0, true);
        let row_node = self.row(ui, "");
        let text = if self.account.has_password { tr("Change Password") } else { tr("Set Password") };
        pill_button(ui, row_node, text, SettingsAction::SetPassword, false);
        self.password_fields = Some(PasswordFields { current, new, confirm });
    }

    /// Data & Downloads (Safari's Downloads and "Clear History and Website Data", Chrome's
    /// "Delete browsing data", Firefox's "Cookies and Site Data").
    fn data_page(&mut self, ui: &mut Ui, config: &Config) {
        self.section(ui, tr("Downloads"));
        let folder = crate::downloads::display_path(&crate::downloads::download_dir(config));
        let row_node = self.row(ui, tr("Download location"));
        let label = ui.add_label(row_node, &folder, TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
        pill_button(ui, row_node, tr("Change…"), SettingsAction::ChooseDownloadFolder, false);
        for setting in [Setting::AskWhereToSave, Setting::OpenSafeFiles] {
            self.control(ui, config, setting);
        }
        let row_node = self.row(ui, tr("Remove download list items"));
        let options: Vec<(&str, ActionId)> = REMOVE_DOWNLOADS
            .iter()
            .map(|(name, r)| (tr(name), SettingsAction::RemoveDownloads(*r).id()))
            .collect();
        let current = REMOVE_DOWNLOADS.iter().position(|(_, r)| *r == config.downloads.remove_list_items);
        segmented(ui, row_node, &options, current);
        self.note(ui, tr("“Safe” files are pictures, movies, sounds, PDF and text documents."));

        self.import_group(ui, config);

        self.section(ui, tr("Clear browsing data"));
        let row_node = self.row(ui, tr("Time range"));
        let options: Vec<(&str, ActionId)> =
            CLEAR_RANGES.iter().map(|(name, r)| (tr(name), SettingsAction::ClearRange(*r).id())).collect();
        let current = CLEAR_RANGES.iter().position(|(_, r)| *r == config.data.clear_range);
        segmented(ui, row_node, &options, current);
        for setting in [Setting::ClearHistoryItem, Setting::ClearCookiesItem] {
            self.control(ui, config, setting);
        }
        let cache = match self.cache_size {
            Some(size) => {
                tr_fmt!(
                    "{name} ({size})",
                    name = Setting::ClearCacheItem.label(),
                    size = crate::downloads::human_size(size as i64)
                )
            }
            None => Setting::ClearCacheItem.label().to_owned(),
        };
        self.control_labeled(ui, config, Setting::ClearCacheItem, &cache);
        self.control(ui, config, Setting::ClearDownloadsItem);
        let row_node = self.row(ui, "");
        let text =
            if self.confirm_clear_data { tr("Click Again to Clear") } else { tr("Clear Browsing Data…") };
        destructive_button(ui, row_node, text, SettingsAction::ClearBrowsingData, self.confirm_clear_data);
        self.note(
            ui,
            tr("A time range clears cookies only for the sites visited in it; the cache is cleared completely."),
        );

        self.section(ui, tr("Cookies and website data"));
        self.control(ui, config, Setting::ClearCookiesOnQuit);
        match self.site_data.clone() {
            None => {
                self.row(ui, tr("Looking for websites with data…"));
            }
            Some(sites) => {
                let summary = match sites.len() {
                    0 => tr("No websites store cookies").to_owned(),
                    n => tr_n("{n} website stores cookies", "{n} websites store cookies", n),
                };
                let row_node = self.row(ui, &summary);
                if !sites.is_empty() {
                    let text =
                        if self.confirm_remove_all { tr("Click Again to Remove") } else { tr("Remove All") };
                    let confirming = self.confirm_remove_all;
                    destructive_button(ui, row_node, text, SettingsAction::RemoveAllSiteData, confirming);
                }
                for (i, (site, count)) in sites.iter().take(SITE_DATA_LIMIT).enumerate() {
                    let row_node = self.row_with_height(ui, site, 40.0);
                    let cookies = tr_n("{n} cookie", "{n} cookies", *count);
                    let label = ui.add_label(row_node, &cookies, TextStyle::Footnote, Style::default());
                    if let Some(w) = ui.widget_mut(label) {
                        w.set_paint(Paint::Secondary);
                    }
                    let remove = ui.add(
                        row_node,
                        Widget::container()
                            .with_action(SettingsAction::RemoveSiteData(i).id())
                            .with_radius(CAPSULE)
                            .with_background(Background::Glass(GlassLevel::Thin)),
                        Style {
                            size: Size { width: length(28.0), height: length(28.0) },
                            justify_content: JustifyContent::CENTER,
                            ..row(0.0)
                        },
                    );
                    let cross = Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary);
                    ui.add(remove, cross, Style::default());
                }
                if sites.len() > SITE_DATA_LIMIT {
                    self.note(ui, &tr_fmt!("And {count} more.", count = sites.len() - SITE_DATA_LIMIT));
                }
            }
        }
    }

    fn row(&mut self, ui: &mut Ui, label: &str) -> NodeId {
        self.row_with_height(ui, label, ROW_HEIGHT)
    }

    /// Privacy › Site Permissions: what sites may ask for, and the decisions remembered per
    /// site (with forget buttons and "Reset All").
    fn site_permissions_section(&mut self, ui: &mut Ui, config: &Config) {
        self.section(ui, tr("Site Permissions"));
        for (i, group) in Group::ALL.into_iter().enumerate() {
            let row_node = self.row(ui, tr(group.name()));
            let options: Vec<(&str, ActionId)> = PERMISSION_DEFAULTS
                .iter()
                .map(|(name, value)| (tr(name), SettingsAction::PermissionDefault(i, *value).id()))
                .collect();
            let current = PERMISSION_DEFAULTS.iter().position(|(_, v)| *v == group.default_of(config));
            segmented(ui, row_node, &options, current);
        }
        self.note(
            ui,
            tr("Ask shows a prompt under the address field. Screen sharing asks every time; it is never remembered."),
        );
        self.section(ui, tr("Remembered for Sites"));
        let decisions = self.site_permissions.clone();
        let summary = match decisions.len() {
            0 => tr("No decisions remembered").to_owned(),
            n => tr_n("{n} decision remembered", "{n} decisions remembered", n),
        };
        let row_node = self.row(ui, &summary);
        if !decisions.is_empty() {
            let confirming = self.confirm_reset_permissions;
            let text = if confirming { tr("Click Again to Reset") } else { tr("Reset All") };
            destructive_button(ui, row_node, text, SettingsAction::ResetPermissions, confirming);
        }
        for (i, (origin, kind, allowed)) in decisions.iter().take(PERMISSION_SITE_LIMIT).enumerate() {
            let site = crate::url::display_host(origin);
            let row_node = self.row_with_height(ui, if site.is_empty() { origin } else { &site }, 40.0);
            let state = if *allowed { tr("Allowed") } else { tr("Blocked") };
            let what = format!("{} · {state}", tr(crate::permissions::kind_name(*kind)));
            let label = ui.add_label(row_node, &what, TextStyle::Footnote, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            let remove = ui.add(
                row_node,
                Widget::container()
                    .with_action(SettingsAction::ForgetPermission(i).id())
                    .with_radius(CAPSULE)
                    .with_background(Background::Glass(GlassLevel::Thin)),
                Style {
                    size: Size { width: length(28.0), height: length(28.0) },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            let cross = Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary);
            ui.add(remove, cross, Style::default());
        }
        if decisions.len() > PERMISSION_SITE_LIMIT {
            self.note(ui, &tr_fmt!("And {count} more.", count = decisions.len() - PERMISSION_SITE_LIMIT));
        }
        self.note(ui, tr("Decisions in private tabs are forgotten when Axion quits."));
    }

    /// The start page's backgrounds: a large preview of the selected one (dimmed and blurred
    /// like the page), then a card per background, "None" and "Add…".
    fn background_picker(&mut self, ui: &mut Ui, config: &Config) {
        let parent = self.group_for_row(ui);
        let selected = self.backgrounds.iter().position(|c| c.name == config.home.background);
        let stage = ui.add(
            parent,
            Widget::container(),
            Style {
                justify_content: JustifyContent::CENTER,
                padding: padding(0.0, 16.0),
                flex_shrink: 0.0,
                ..row(0.0)
            },
        );
        let preview_style = Style {
            size: Size { width: length(BACKGROUND_PREVIEW.x), height: length(BACKGROUND_PREVIEW.y) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        };
        let image = selected.and_then(|i| {
            let card = &self.backgrounds[i];
            match &self.background_preview {
                Some((name, image)) if *name == card.name => Some(image.clone()),
                _ => card.image.clone(),
            }
        });
        let shadow = axion_ui::Shadow { offset_y: 2.0, blur: 10.0, opacity: 0.18 };
        // The page's ground (black under a picture), the picture, and a clock like the page's
        // to judge legibility. The picture's opacity over black is the dimming: images draw
        // above fills, so a dark fill can't lie on top of it.
        let ground = if selected.is_some() { Paint::Fixed(Color::rgb(0x000000)) } else { Paint::Ground };
        let preview = ui.add(
            stage,
            Widget::container()
                .with_background(Background::Fill(ground, 1.0))
                .with_radius(BACKGROUND_PREVIEW_RADIUS)
                .with_shadow(shadow),
            preview_style,
        );
        if let Some(image) = image {
            let node = ui.add(
                preview,
                Widget::image().with_radius(BACKGROUND_PREVIEW_RADIUS),
                Style {
                    position: Position::Absolute,
                    inset: Rect { left: length(0.0), right: auto(), top: length(0.0), bottom: auto() },
                    size: Size { width: length(BACKGROUND_PREVIEW.x), height: length(BACKGROUND_PREVIEW.y) },
                    ..Default::default()
                },
            );
            ui.set_image_crop(node, Some(cover_crop(image.1, BACKGROUND_PREVIEW)));
            ui.set_image(node, Some(image));
            ui.set_opacity(node, 1.0 - config.home.background_dim);
            self.preview_node = Some(node);
        }
        let clock = ui.add_label(preview, "9:41", TextStyle::LargeTitle, Style::default());
        if let Some(w) = ui.widget_mut(clock) {
            w.set_paint(if selected.is_some() { Paint::Fixed(Color::rgb(0xFFFFFF)) } else { Paint::Label });
        }

        let grid = ui.add(
            parent,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                justify_content: JustifyContent::CENTER,
                gap: Size { width: length(10.0), height: length(10.0) },
                padding: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(16.0),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        let card_style = Style {
            size: Size { width: length(BACKGROUND_CARD.x), height: length(BACKGROUND_CARD.y) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            flex_direction: FlexDirection::Column,
            gap: Size { width: length(0.0), height: length(2.0) },
            ..Default::default()
        };
        // "None": the plain white or black page.
        let ring = background_ring(ui, grid, SettingsAction::NoBackground, selected.is_none());
        let none = ui.add(
            ring,
            Widget::container()
                .with_background(Background::Fill(Paint::Ground, 1.0))
                .with_radius(BACKGROUND_CARD_RADIUS),
            card_style.clone(),
        );
        let label = ui.add_label(none, tr("None"), TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
        for (i, card) in self.backgrounds.iter().enumerate().take(BACKGROUND_SLOTS) {
            let ring = background_ring(ui, grid, SettingsAction::Background(i), selected == Some(i));
            match &card.image {
                Some(image) => {
                    let node =
                        ui.add(ring, Widget::image().with_radius(BACKGROUND_CARD_RADIUS), card_style.clone());
                    ui.set_image_crop(node, Some(cover_crop(image.1, BACKGROUND_CARD)));
                    ui.set_image(node, Some(image.clone()));
                }
                None => {
                    // Still being made, or a format without a thumbnail here (the page shows it).
                    let node = ui.add(
                        ring,
                        Widget::container()
                            .with_background(Background::Fill(Paint::Label, 0.07))
                            .with_radius(BACKGROUND_CARD_RADIUS),
                        card_style.clone(),
                    );
                    if !card.loading {
                        ui.add(
                            node,
                            Widget::icon(Icon::File, 18.0).with_paint(Paint::Secondary),
                            Style::default(),
                        );
                        let ext = card.name.rsplit('.').next().unwrap_or_default().to_uppercase();
                        let label = ui.add_label(node, &ext, TextStyle::Caption, Style::default());
                        if let Some(w) = ui.widget_mut(label) {
                            w.set_paint(Paint::Secondary);
                        }
                    }
                }
            }
            // Glass, so it lies above the thumbnail (images draw above fills).
            let side = 22.0;
            let remove = ui.add(
                ring,
                Widget::container()
                    .with_action(SettingsAction::RemoveBackground(i).id())
                    .with_radius(CAPSULE)
                    .with_background(Background::Glass(GlassLevel::Thin)),
                Style {
                    position: Position::Absolute,
                    inset: Rect {
                        left: length(BACKGROUND_RING + BACKGROUND_CARD.x - side - 5.0),
                        right: auto(),
                        top: length(BACKGROUND_RING + 5.0),
                        bottom: auto(),
                    },
                    size: Size { width: length(side), height: length(side) },
                    justify_content: JustifyContent::CENTER,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            );
            ui.add(remove, Widget::icon(Icon::Close, 11.0).with_paint(Paint::Label), Style::default());
        }
        let limit = (config.home.max_backgrounds as usize).min(BACKGROUND_SLOTS);
        if self.backgrounds.len() < limit {
            let ring = background_ring(ui, grid, SettingsAction::ChooseBackground, false);
            let add = ui.add(
                ring,
                Widget::container()
                    .with_background(Background::Fill(Paint::Accent, 0.10))
                    .with_radius(BACKGROUND_CARD_RADIUS),
                card_style,
            );
            ui.add(add, Widget::icon(Icon::Plus, 18.0).with_paint(Paint::Accent), Style::default());
            let label = ui.add_label(add, tr("Add…"), TextStyle::Footnote, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Accent);
            }
        }
    }

    /// The settings matching the search, grouped by page: toggles and sliders work right
    /// here, everything else has a button that opens its page.
    /// A row with a short label and a grey value (the status rows).
    fn info_row(&mut self, ui: &mut Ui, label: &str, value: &str) {
        let row_node = self.row(ui, label);
        let text = ui.add_label(
            row_node,
            value,
            TextStyle::Callout,
            Style {
                flex_grow: 0.0,
                flex_shrink: 1.0,
                // The values start at one edge, whatever the labels' widths.
                flex_basis: percent(0.6),
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        ui.set_label_lines(text, NOTE_LINES);
        if let Some(w) = ui.widget_mut(text) {
            w.set_paint(Paint::Secondary);
        }
    }

    /// A note in warning red.
    fn warning_note(&mut self, ui: &mut Ui, text: &str) {
        self.note(ui, text);
        if let Some(&label) = ui.children(self.content).last()
            && let Some(w) = ui.widget_mut(label)
        {
            w.set_paint(Paint::Fixed(DESTRUCTIVE));
        }
    }

    /// Settings › Hardware: acceleration switches, page rendering, graphics card, and what is
    /// actually active (all read when Axion starts).
    fn reading_page(&mut self, ui: &mut Ui, config: &Config) {
        self.section(ui, tr("Reader View"));
        for setting in [Setting::ReaderButton, Setting::ReaderRememberPerSite] {
            self.control(ui, config, setting);
        }
        let row_node = self.row(ui, tr("Font"));
        let options: Vec<(&str, ActionId)> = READER_FONTS
            .iter()
            .map(|(name, font)| (tr(name), SettingsAction::ReaderFont(*font).id()))
            .collect();
        let current = READER_FONTS.iter().position(|(_, f)| *f == config.reader.font);
        segmented(ui, row_node, &options, current);
        let row_node = self.row(ui, tr("Theme"));
        let options: Vec<(&str, ActionId)> = READER_THEMES
            .iter()
            .map(|(name, theme)| (tr(name), SettingsAction::ReaderTheme(*theme).id()))
            .collect();
        let current = READER_THEMES.iter().position(|(_, t)| *t == config.reader.theme);
        segmented(ui, row_node, &options, current);
        self.note(ui, tr("Text size and column width are in Reader Options of the page menu."));

        self.section(ui, tr("Translation"));
        for setting in [Setting::TranslateOffer, Setting::TranslatePrompt, Setting::TranslateChanges] {
            self.control(ui, config, setting);
        }
        let row_node = self.row(ui, tr("Translate pages into"));
        let options: Vec<(&str, ActionId)> = TRANSLATE_TARGETS
            .iter()
            .enumerate()
            .map(|(i, (_, name))| (tr(name), SettingsAction::TranslateTarget(i).id()))
            .collect();
        let current =
            TRANSLATE_TARGETS.iter().position(|(code, _)| *code == config.translation.target_language);
        segmented(ui, row_node, &options, current);
        let row_node = self.row(ui, tr("Translate"));
        let options: Vec<(&str, ActionId)> = TRANSLATION_BACKENDS
            .iter()
            .map(|(name, backend)| (tr(name), SettingsAction::TranslationBackend(*backend).id()))
            .collect();
        let current = TRANSLATION_BACKENDS.iter().position(|(_, b)| *b == config.translation.backend);
        segmented(ui, row_node, &options, current);
        self.note(
            ui,
            tr("On this computer: language data from Mozilla is downloaded once, with your consent; pages never leave it."),
        );
        if config.translation.backend == TranslationBackend::Endpoint {
            let note = if config.translation.endpoint_url.trim().is_empty() {
                tr(
                    "Set translation.endpoint_url in the settings file to a LibreTranslate server. Page text is sent to it.",
                )
            } else {
                tr("Page text is sent to the server in translation.endpoint_url.")
            };
            self.note(ui, note);
        }
        let row_node = self.row(ui, tr("Downloaded language data"));
        pill_button(ui, row_node, tr("Remove"), SettingsAction::RemoveTranslationModels, true);
        if !config.translation.never_sites.is_empty() {
            self.section(ui, tr("Never translated"));
            self.note(ui, &config.translation.never_sites.join(", "));
        }
    }

    fn hardware_page(&mut self, ui: &mut Ui, config: &Config) {
        let status = self.hardware.clone();
        let amd = status.as_ref().is_some_and(|h| crate::hardware::is_amd(h.in_use.vendor));

        self.section(ui, tr("Acceleration"));
        self.control(ui, config, Setting::HardwareEnabled);
        self.control_labeled(ui, config, Setting::HardwareDecoding, tr("Hardware video decoding"));
        self.control(ui, config, Setting::GpuRasterization);
        self.note(
            ui,
            tr("Off: Chromium draws pages, video and canvas in software. Axion's own interface always \
                uses the graphics card (it cannot run without one)."),
        );

        self.section(ui, tr("Page rendering"));
        let row_node = self.row(ui, tr("Page frames"));
        let options: Vec<(&str, ActionId)> =
            PAGE_FRAMES.iter().map(|&(name, f)| (tr(name), SettingsAction::PageFrames(f).id())).collect();
        let current = PAGE_FRAMES.iter().position(|&(_, f)| f == config.hardware.page_frames);
        segmented(ui, row_node, &options, current);
        self.note(
            ui,
            tr(
                "Automatic shares GPU memory only where that is known to work (NVIDIA on Wayland) and copies \
                frames through system memory elsewhere. Software is the safe choice.",
            ),
        );
        if amd {
            let text =
                tr("Warning: on AMD graphics (RADV), “Shared GPU memory” can crash the graphics driver. \
                           Keep Automatic unless you know your setup works.");
            if config.hardware.page_frames == PageFrames::Gpu {
                self.warning_note(ui, text);
            } else {
                self.note(ui, text);
            }
        }

        self.section(ui, tr("Graphics card"));
        let row_node = self.row(ui, tr("Graphics card"));
        let options: Vec<(&str, ActionId)> =
            GPU_CARDS.iter().map(|&(name, g)| (tr(name), SettingsAction::GpuCard(g).id())).collect();
        let current = GPU_CARDS.iter().position(|&(_, g)| g == config.hardware.gpu);
        segmented(ui, row_node, &options, current);
        if let Some(status) = &status {
            let describe = |a: &crate::hardware::AdapterLine| {
                tr_fmt!(
                    "{name} ({backend}, {kind})",
                    name = a.name.as_str(),
                    backend = a.backend.as_str(),
                    kind = adapter_kind(a.kind)
                )
            };
            self.info_row(ui, tr("In use"), &describe(&status.in_use));
            for adapter in status.adapters.iter().filter(|a| {
                (a.name.as_str(), a.vendor, a.device)
                    != (status.in_use.name.as_str(), status.in_use.vendor, status.in_use.device)
            }) {
                self.info_row(ui, tr("Also available"), &describe(adapter));
            }
        }
        self.note(
            ui,
            tr("Integrated saves power, Dedicated is the fastest. Automatic prefers the dedicated card if \
                there is one. Applies after restarting Axion."),
        );

        if let Some(status) = &status {
            self.section(ui, tr("GPU status"));
            let frames = match status.frames {
                crate::hardware::FrameMode::Gpu => tr("Shared GPU memory"),
                crate::hardware::FrameMode::Software => tr("Software (system memory)"),
            };
            self.info_row(ui, tr("Page frames"), frames);
            let video = if status.video_decoding { tr("VA-API requested") } else { tr("Off") };
            self.info_row(ui, tr("Video decoding"), video);
            self.info_row(ui, tr("Chromium"), &status.cef_version);
            let mut overridden = Vec::new();
            if let Some(v) = &status.env.gpu {
                overridden.push(format!("AXION_GPU={v}"));
            }
            if let Some(v) = &status.env.osr {
                overridden.push(format!("AXION_OSR={v}"));
            }
            if !overridden.is_empty() {
                self.note(
                    ui,
                    &tr_fmt!("Overridden by the environment: {names}.", names = overridden.join(", ")),
                );
            }
            let row_node = self.row(ui, tr("Diagnostics"));
            glass_pill(ui, row_node, tr("Copy Diagnostics"), SettingsAction::CopyDiagnostics);
        }

        if status.as_ref().is_some_and(|h| h.startup.needs_restart(config)) {
            self.section(ui, "");
            let row_node = self.row(ui, tr("Applies after restarting Axion."));
            primary_button(ui, row_node, tr("Restart Axion to Apply"), SettingsAction::RestartAxion);
        }
    }

    fn show_results(&mut self, ui: &mut Ui, config: &Config) {
        ui.add_label(
            self.content,
            tr("Search Results"),
            TextStyle::Title1,
            Style {
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(12.0),
                },
                ..Default::default()
            },
        );
        let hits = search::search(&self.query);
        if hits.is_empty() {
            let label = ui.add_label(
                self.content,
                &tr_fmt!("No settings match “{query}”.", query = self.query),
                TextStyle::Body,
                Style::default(),
            );
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            return;
        }
        // Pages in the order of the best hit on each.
        let mut pages: Vec<Category> = Vec::new();
        for hit in &hits {
            if !pages.contains(&hit.category) {
                pages.push(hit.category);
            }
        }
        for category in pages {
            self.section(ui, category.title());
            for hit in hits.iter().filter(|h| h.category == category) {
                match hit.target {
                    search::Target::Control(setting) => self.control(ui, config, setting),
                    search::Target::Page(title) => {
                        let row_node = self.row(ui, tr(title));
                        pill_button(ui, row_node, tr("Show"), SettingsAction::Category(category), false);
                    }
                }
            }
        }
    }

    /// Safari's two tab bar layouts as small pictures with a radio mark under each.
    fn tab_bar_choice(&mut self, ui: &mut Ui, config: &Config) {
        let parent = self.group_for_row(ui);
        let choices = ui.add(
            parent,
            Widget::container(),
            Style {
                justify_content: JustifyContent::CENTER,
                padding: padding(0.0, 14.0),
                flex_shrink: 0.0,
                ..row(64.0)
            },
        );
        for (style, name) in [(TabBarStyle::Separate, tr("Separate")), (TabBarStyle::Compact, tr("Compact"))]
        {
            let selected = config.layout.tab_bar == style;
            let choice = ui.add(
                choices,
                Widget::container().with_action(SettingsAction::TabBar(style).id()).with_radius(12.0),
                Style {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(0.0), height: length(6.0) },
                    padding: padding(8.0, 6.0),
                    ..Default::default()
                },
            );
            tab_bar_picture(ui, choice, style, selected);
            ui.add_label(choice, name, TextStyle::Footnote, Style::default());
            let radio = if selected {
                Widget::icon(Icon::Check, 12.0).with_paint(Paint::Fixed(Color::rgb(0xFFFFFF)))
            } else {
                Widget::container()
            };
            let mark = ui.add(
                choice,
                Widget::container().with_radius(CAPSULE).with_background(if selected {
                    Background::Fill(Paint::Accent, 1.0)
                } else {
                    Background::Fill(Paint::Label, 0.12)
                }),
                Style {
                    size: Size { width: length(20.0), height: length(20.0) },
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            ui.add(mark, radio, Style::default());
        }
    }

    /// (Re)builds the shortcut rows in `list`, grouped by area, keeping those whose name or
    /// chords contain every word of the filter.
    fn fill_shortcuts(&mut self, ui: &mut Ui, list: NodeId, keymap: &Keymap) {
        ui.clear_children(list);
        let words: Vec<String> =
            self.key_query.to_lowercase().split_whitespace().map(str::to_owned).collect();
        let content = std::mem::replace(&mut self.content, list);
        self.group = None;
        let mut shown = 0;
        let mut last_group = "";
        for action in shortcut_actions() {
            let name = shortcut_name(action);
            let chords: Vec<String> = keymap.chords_for(action).iter().map(ToString::to_string).collect();
            // Matches the translated name, the English one and the chords.
            let haystack =
                format!("{name} {} {}", shortcut_source_name(action), chords.join(" ")).to_lowercase();
            if !words.iter().all(|w| haystack.contains(w.as_str())) {
                continue;
            }
            let group = shortcut_group(action);
            if group != last_group {
                last_group = group;
                self.section(ui, group);
            }
            let row_node = self.row_with_height(ui, name, 34.0);
            let label = ui.add_label(row_node, &chords.join("   "), TextStyle::Callout, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            shown += 1;
        }
        if shown == 0 {
            self.note(ui, tr("No shortcuts match."));
        }
        self.group = None;
        self.content = content;
    }

    /// Starts a new group of rows, with a small grey title above it (empty: none).
    fn section(&mut self, ui: &mut Ui, title: &str) {
        self.group = None;
        if !title.is_empty() {
            let label = ui.add_label(
                self.content,
                &title.to_uppercase(),
                TextStyle::Caption,
                Style {
                    margin: Rect {
                        left: length(GROUP_INSET),
                        right: length(0.0),
                        top: length(GROUP_GAP),
                        bottom: length(6.0),
                    },
                    ..Default::default()
                },
            );
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
        }
    }

    /// Small grey text under the current group.
    fn note(&mut self, ui: &mut Ui, text: &str) {
        self.group = None;
        let label = ui.add_label(
            self.content,
            text,
            TextStyle::Footnote,
            Style {
                margin: Rect {
                    left: length(GROUP_INSET),
                    right: length(0.0),
                    top: length(6.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        ui.set_label_lines(label, NOTE_LINES);
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
    }

    /// A row of buttons that wraps onto more lines when they don't fit, right-aligned.
    fn button_row(&mut self, ui: &mut Ui) -> NodeId {
        let parent = self.group_for_row(ui);
        ui.add(
            parent,
            Widget::container(),
            Style {
                flex_wrap: FlexWrap::Wrap,
                justify_content: JustifyContent::FLEX_END,
                padding: padding(0.0, 5.0),
                flex_shrink: 0.0,
                gap: Size { width: length(8.0), height: length(8.0) },
                ..row(8.0)
            },
        )
    }

    /// The card the next row goes in (with a separator above it if the card has rows).
    fn group_for_row(&mut self, ui: &mut Ui) -> NodeId {
        if let Some(group) = self.group {
            ui.add(
                group,
                Widget::container().with_background(Background::Fill(Paint::Hairline, 1.0)),
                Style {
                    size: Size { width: auto(), height: length(1.0) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            return group;
        }
        let first = ui.children(self.content).len() <= 1;
        let group = ui.add(
            self.content,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, 0.85))
                .with_radius(GROUP_RADIUS),
            Style {
                flex_direction: FlexDirection::Column,
                flex_shrink: 0.0,
                padding: padding(GROUP_INSET, 0.0),
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(if first { 0.0 } else { 8.0 }),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        self.group = Some(group);
        group
    }

    fn row_with_height(&mut self, ui: &mut Ui, label: &str, height: f32) -> NodeId {
        let parent = self.group_for_row(ui);
        // At least `height` tall, taller when a long label wraps.
        let row_node = ui.add(
            parent,
            Widget::container(),
            Style {
                min_size: Size { width: auto(), height: length(height) },
                padding: padding(0.0, 6.0),
                flex_shrink: 0.0,
                ..row(10.0)
            },
        );
        let label = ui.add_label(row_node, label, TextStyle::Body, grow_label());
        ui.set_label_lines(label, LABEL_LINES);
        row_node
    }

    fn control(&mut self, ui: &mut Ui, config: &Config, setting: Setting) {
        self.control_labeled(ui, config, setting, setting.label());
    }

    fn control_labeled(&mut self, ui: &mut Ui, config: &Config, setting: Setting, label: &str) {
        let row_node = self.row(ui, label);
        let value = setting.get(config);
        let node = if setting.is_toggle() {
            ui.add(row_node, Widget::toggle(value > 0.5), Style::default())
        } else {
            let slider = ui.add(
                row_node,
                Widget::slider(value),
                Style {
                    size: Size { width: length(SLIDER_WIDTH), height: auto() },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            // The current value, right of the slider; a fixed width keeps the sliders aligned.
            let label = ui.add_label(
                row_node,
                &setting.display(config),
                TextStyle::Footnote,
                Style {
                    min_size: Size { width: length(VALUE_WIDTH), height: auto() },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            self.values.push((setting, label));
            slider
        };
        self.controls.push((node, setting));
    }

    /// A toggle or slider moved.
    pub fn value_changed(&mut self, ui: &mut Ui, node: NodeId, value: f32, config: &mut Config) -> Outcome {
        match self.controls.iter().find(|(n, _)| *n == node) {
            Some(&(_, setting)) => {
                setting.set(config, value);
                // The Privacy page shows the password fields while the lock has no password.
                self.rebuild |= matches!(
                    setting,
                    Setting::LockPrivateTabs
                        | Setting::TintFromPage
                        | Setting::HardwareEnabled
                        | Setting::HardwareDecoding
                        | Setting::GpuRasterization
                );
                // The preview dims live; its blur follows once it is made (see the browser).
                if setting == Setting::HomeDim
                    && let Some(node) = self.preview_node
                {
                    ui.set_opacity(node, 1.0 - config.home.background_dim);
                }
                // Live while dragging.
                for &(_, label) in self.values.iter().filter(|(s, _)| *s == setting) {
                    ui.set_text(label, &setting.display(config));
                }
                Outcome::Changed
            }
            None => Outcome::Nothing,
        }
    }

    /// A click in the sheet.
    pub fn action(
        &mut self,
        ui: &mut Ui,
        action: SettingsAction,
        config: &mut Config,
        keymap: &Keymap,
    ) -> Outcome {
        let outcome = match action {
            SettingsAction::Close => return Outcome::Close,
            SettingsAction::Category(category) => {
                self.category = category;
                self.query.clear();
                ui.set_field_text(self.search, "");
                self.confirm_delete = false;
                self.account_status = None;
                self.confirm_clear = false;
                self.confirm_clear_data = false;
                self.confirm_remove_all = false;
                self.confirm_reset_permissions = false;
                if category == Category::Data {
                    self.site_data = None;
                    Outcome::DataPage
                } else {
                    Outcome::Nothing
                }
            }
            SettingsAction::ChooseDownloadFolder => return Outcome::ChooseDownloadFolder,
            SettingsAction::ChoosePlaylistFolder => return Outcome::ChoosePlaylistFolder,
            SettingsAction::ResetPlaylistFolder => {
                config.playlist.directory.clear();
                Outcome::Changed
            }
            SettingsAction::NoBackground => {
                config.home.background.clear();
                Outcome::Changed
            }
            SettingsAction::Background(i) => match self.backgrounds.get(i) {
                Some(card) => {
                    config.home.background.clone_from(&card.name);
                    Outcome::Changed
                }
                None => Outcome::Nothing,
            },
            SettingsAction::RemoveBackground(i) => match self.backgrounds.get(i) {
                Some(card) => return Outcome::RemoveBackground(card.name.clone()),
                None => Outcome::Nothing,
            },
            SettingsAction::ChooseBackground => return Outcome::ChooseBackground,
            SettingsAction::AddBackgroundPath => {
                let text =
                    self.background_path.and_then(|f| ui.field_text(f)).unwrap_or_default().trim().to_owned();
                if text.is_empty() {
                    self.background_status = Some(tr("Enter the image's path first.").to_owned());
                    Outcome::Nothing
                } else {
                    return Outcome::AddBackground(expand_home(&text));
                }
            }
            SettingsAction::RemoveDownloads(r) => {
                config.downloads.remove_list_items = r;
                Outcome::Changed
            }
            SettingsAction::ClearRange(r) => {
                config.data.clear_range = r;
                Outcome::Changed
            }
            SettingsAction::ClearBrowsingData => {
                self.confirm_clear_data = !self.confirm_clear_data;
                if self.confirm_clear_data {
                    Outcome::Nothing
                } else {
                    self.cache_size = None;
                    self.site_data = None;
                    Outcome::ClearBrowsingData
                }
            }
            SettingsAction::RemoveAllSiteData => {
                self.confirm_remove_all = !self.confirm_remove_all;
                if self.confirm_remove_all {
                    Outcome::Nothing
                } else {
                    self.site_data = None;
                    Outcome::RemoveSiteData(None)
                }
            }
            SettingsAction::PermissionDefault(group, value) => match Group::ALL.get(group) {
                Some(group) => {
                    group.set_default(config, value);
                    Outcome::Changed
                }
                None => Outcome::Nothing,
            },
            SettingsAction::ResetPermissions => {
                self.confirm_reset_permissions = !self.confirm_reset_permissions;
                if self.confirm_reset_permissions {
                    Outcome::Nothing
                } else {
                    self.site_permissions.clear();
                    Outcome::ForgetPermissions(None)
                }
            }
            SettingsAction::ForgetPermission(i) => {
                if i < self.site_permissions.len() {
                    let (origin, kind, _) = self.site_permissions.remove(i);
                    Outcome::ForgetPermissions(Some((origin, kind)))
                } else {
                    Outcome::Nothing
                }
            }
            SettingsAction::RemoveSiteData(i) => {
                match self.site_data.as_mut().filter(|s| i < s.len()).map(|s| s.remove(i)) {
                    Some((site, _)) => Outcome::RemoveSiteData(Some(site)),
                    None => Outcome::Nothing,
                }
            }
            SettingsAction::Theme(mode) => {
                config.appearance.theme = mode;
                Outcome::Changed
            }
            SettingsAction::GlassStyle(style) => {
                config.glass.style = style;
                Outcome::Changed
            }
            SettingsAction::ShowWelcomeTour => return Outcome::ShowWelcomeTour,
            SettingsAction::IconWeight(i) => {
                set_icon_weight(config, i);
                Outcome::Changed
            }
            SettingsAction::SearchEngine(i) => {
                config.general.search_url = SEARCH_ENGINES[i].1.to_owned();
                Outcome::Changed
            }
            SettingsAction::Accent(i) => {
                config.appearance.accent = ACCENTS[i];
                Outcome::Changed
            }
            SettingsAction::AccountColor(i) => {
                self.account.color = ACCENTS[i];
                Outcome::AccountColor(ACCENTS[i])
            }
            SettingsAction::ChooseAccountPhoto => return Outcome::ChooseAccountPhoto,
            SettingsAction::RemoveAccountPhoto => return Outcome::RemoveAccountPhoto,
            SettingsAction::SaveAccountName => {
                let name =
                    self.account_name.and_then(|f| ui.field_text(f)).unwrap_or_default().trim().to_owned();
                if name.is_empty() {
                    return Outcome::Nothing;
                }
                return Outcome::RenameAccount(name);
            }
            SettingsAction::DeleteAccount => {
                self.confirm_delete = !self.confirm_delete;
                if self.confirm_delete {
                    Outcome::Nothing
                } else {
                    return Outcome::DeleteAccount;
                }
            }
            SettingsAction::ShieldLevel(level) => {
                config.shields.default_level = level;
                Outcome::Changed
            }
            SettingsAction::UpdateFilterLists => return Outcome::UpdateFilterLists,
            SettingsAction::TabBar(style) => {
                config.layout.tab_bar = style;
                Outcome::Changed
            }
            SettingsAction::CloseAfter(days) => {
                config.tabs.close_after_days = days;
                Outcome::Changed
            }
            SettingsAction::BookmarksBar(mode) => {
                config.layout.bookmarks_bar = mode;
                Outcome::Changed
            }
            SettingsAction::TintMode(mode) => {
                config.appearance.tint_mode = mode;
                Outcome::Changed
            }
            SettingsAction::PlaylistSort(sort) => {
                config.playlist.sort = sort;
                Outcome::Changed
            }
            SettingsAction::ReaderFont(font) => {
                config.reader.font = font;
                Outcome::Changed
            }
            SettingsAction::ReaderTheme(theme) => {
                config.reader.theme = theme;
                Outcome::Changed
            }
            SettingsAction::TranslationBackend(backend) => {
                config.translation.backend = backend;
                Outcome::Changed
            }
            SettingsAction::TranslateTarget(i) => {
                if let Some((code, _)) = TRANSLATE_TARGETS.get(i) {
                    config.translation.target_language = (*code).to_owned();
                }
                Outcome::Changed
            }
            SettingsAction::RemoveTranslationModels => return Outcome::RemoveTranslationModels,
            // Saved at once; the interface is built in one language per run, so it takes
            // effect at the next start (the note under the control says so).
            SettingsAction::Language(language) => {
                config.general.language = language;
                Outcome::Changed
            }
            SettingsAction::AddTrustedSite => {
                let text = self.trusted_site.and_then(|f| ui.field_text(f)).unwrap_or_default().to_owned();
                if config.media.add_trusted_site(&text) { Outcome::Changed } else { Outcome::Nothing }
            }
            SettingsAction::RemoveTrustedSite(i) => {
                if i < config.media.trusted_player_sites.len() {
                    config.media.trusted_player_sites.remove(i);
                    Outcome::Changed
                } else {
                    Outcome::Nothing
                }
            }
            SettingsAction::SetPassword => {
                let Some(fields) = self.password_fields else { return Outcome::Nothing };
                let text = |node: NodeId| Zeroizing::new(ui.field_text(node).unwrap_or_default().to_owned());
                let (current, new, confirm) =
                    (fields.current.map(text), text(fields.new), text(fields.confirm));
                let problem = if current.as_ref().is_some_and(|c| c.is_empty()) {
                    Some(tr("Enter the current password first.").to_owned())
                } else if new.chars().count() < MIN_MASTER_LENGTH {
                    Some(tr_fmt!("Use at least {count} characters.", count = MIN_MASTER_LENGTH))
                } else if *new != *confirm {
                    Some(tr("The new passwords don't match.").to_owned())
                } else {
                    None
                };
                if let Some(problem) = problem {
                    self.account_status = Some(problem);
                    self.show(ui, config, keymap);
                    return Outcome::Nothing;
                }
                return Outcome::SetAccountPassword { current, new };
            }
            SettingsAction::OpenPasswords => return Outcome::OpenPasswords,
            SettingsAction::ImportPasswords
            | SettingsAction::RestorePasswords
            | SettingsAction::ExportPasswords => {
                let text =
                    self.passwords_file.and_then(|f| ui.field_text(f)).unwrap_or_default().trim().to_owned();
                let password = Zeroizing::new(
                    self.passwords_secret.and_then(|f| ui.field_text(f)).unwrap_or_default().to_owned(),
                );
                let problem = if text.is_empty() {
                    Some(tr("Enter the file's path first."))
                } else if action != SettingsAction::ImportPasswords && password.is_empty() {
                    Some(tr("Enter the password first."))
                } else {
                    None
                };
                if let Some(problem) = problem {
                    self.passwords_status = Some(problem.to_owned());
                    self.show(ui, config, keymap);
                    return Outcome::Nothing;
                }
                let path = expand_home(&text);
                return match action {
                    SettingsAction::ImportPasswords => Outcome::ImportPasswords(path),
                    SettingsAction::RestorePasswords => Outcome::RestorePasswords { path, password },
                    _ => Outcome::ExportPasswords { path, password },
                };
            }
            SettingsAction::CheckForUpdates => return Outcome::CheckForUpdates,
            SettingsAction::InstallUpdate => return Outcome::InstallUpdate,
            SettingsAction::SkipUpdate => return Outcome::SkipUpdate,
            SettingsAction::RemindLater => return Outcome::RemindLater,
            SettingsAction::OpenReleasePage => return Outcome::OpenReleasePage,
            SettingsAction::RelaunchNow => return Outcome::RelaunchNow,
            SettingsAction::PageFrames(frames) => {
                let amd = self
                    .hardware
                    .as_ref()
                    .is_some_and(|h| crate::hardware::needs_gpu_frames_confirmation(h.in_use.vendor, frames));
                if amd && config.hardware.page_frames != PageFrames::Gpu {
                    return Outcome::ConfirmGpuFrames;
                }
                config.hardware.page_frames = frames;
                Outcome::Changed
            }
            SettingsAction::GpuCard(card) => {
                config.hardware.gpu = card;
                Outcome::Changed
            }
            SettingsAction::CopyDiagnostics => return Outcome::CopyDiagnostics,
            SettingsAction::RestartAxion => return Outcome::RestartAxion,
            SettingsAction::Import(action) => self.import_action(action),
            SettingsAction::SaveRepository => {
                let text = self
                    .repository_field
                    .and_then(|f| ui.field_text(f))
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if text.is_empty() {
                    config.updates.set_repository("");
                    self.repository_draft = None;
                    let default = config.updates.effective_repository().to_owned();
                    self.repository_status = Some(if default.is_empty() {
                        tr("Updates are off: no repository is set.").to_owned()
                    } else {
                        tr_fmt!(
                            "Back to the default: looking for updates in {repository}.",
                            repository = default
                        )
                    });
                    Outcome::SourceChanged
                } else {
                    match axion_update::parse_repository(&text) {
                        Ok(repository) => {
                            config.updates.set_repository(&repository.to_string());
                            self.repository_draft = None;
                            self.repository_status = Some(tr_fmt!(
                                "Saved. Looking for updates in {repository}.",
                                repository = repository.to_string()
                            ));
                            Outcome::SourceChanged
                        }
                        Err(_) => {
                            self.repository_status = Some(
                                tr("That isn't a repository. Use owner/name, for example henrik/axion.")
                                    .to_owned(),
                            );
                            self.repository_draft = Some(text);
                            Outcome::Nothing
                        }
                    }
                }
            }
            SettingsAction::ClearHistory => {
                if self.confirm_clear {
                    self.confirm_clear = false;
                    self.show(ui, config, keymap);
                    return Outcome::ClearHistory;
                }
                self.confirm_clear = true;
                Outcome::Nothing
            }
        };
        self.show(ui, config, keymap);
        outcome
    }
}

use crate::home::expand_home;

/// The part of a texture of `size` px that covers a frame of `frame` (centred, like CSS's
/// `background-size: cover`), in 0–1 coordinates.
fn cover_crop(size: (u32, u32), frame: Vec2) -> Frame {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let scale = (frame.x / w).max(frame.y / h);
    let visible = Vec2::new((frame.x / scale / w).min(1.0), (frame.y / scale / h).min(1.0));
    Frame { origin: (Vec2::ONE - visible) / 2.0, size: visible }
}

/// A background card's clickable frame, ringed in the accent colour while `selected`.
fn background_ring(ui: &mut Ui, parent: NodeId, action: SettingsAction, selected: bool) -> NodeId {
    ui.add(
        parent,
        Widget::container()
            .with_action(action.id())
            .with_radius(BACKGROUND_CARD_RADIUS + BACKGROUND_RING)
            .with_background(if selected {
                Background::Fill(Paint::Accent, 1.0)
            } else {
                Background::Fill(Paint::Hairline, 1.0)
            }),
        Style { padding: padding(BACKGROUND_RING, BACKGROUND_RING), flex_shrink: 0.0, ..Default::default() },
    )
}

/// A solid accent button: the main action of a row of buttons.
fn primary_button(ui: &mut Ui, parent: NodeId, text: &str, action: SettingsAction) {
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action.id())
            .with_radius(CAPSULE)
            .with_background(Background::TintedGlass(GlassLevel::Thin, Paint::Accent)),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(18.0, 0.0), ..row(0.0) },
    );
    let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnAccent);
    }
}

/// A button that does nothing (a check is running).
fn idle_pill(ui: &mut Ui, parent: NodeId, text: &str) {
    let button = ui.add(
        parent,
        Widget::container().with_radius(CAPSULE).with_background(Background::Glass(GlassLevel::Thin)),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(16.0, 0.0), ..row(0.0) },
    );
    let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
}

/// A small capsule button (accent tint, or red for destructive ones).
fn pill_button(ui: &mut Ui, parent: NodeId, text: &str, action: SettingsAction, destructive: bool) {
    let (tint, on) = if destructive {
        (Paint::Fixed(DESTRUCTIVE), Paint::OnColor(DESTRUCTIVE))
    } else {
        (Paint::Accent, Paint::OnAccent)
    };
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action.id())
            .with_radius(CAPSULE)
            .with_background(Background::TintedGlass(GlassLevel::Thin, tint)),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(16.0, 0.0), ..row(0.0) },
    );
    let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(on);
    }
}

/// A button of thin glass (Liquid Glass lying on the sheet's glass), for actions that
/// aren't the page's main one.
pub fn glass_pill(ui: &mut Ui, parent: NodeId, text: &str, action: SettingsAction) {
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action.id())
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thin)),
        Style {
            size: Size { width: auto(), height: length(36.0) },
            padding: padding(16.0, 0.0),
            // Apart enough that neighbouring glass buttons don't merge.
            margin: Rect { left: length(6.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
            ..row(0.0)
        },
    );
    ui.add_label(button, text, TextStyle::Callout, Style::default());
}

/// A red button for destructive actions; filled while it asks for confirmation.
fn destructive_button(ui: &mut Ui, parent: NodeId, text: &str, action: SettingsAction, confirming: bool) {
    let button = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE).with_background(
            Background::TintedGlass(
                if confirming { GlassLevel::Regular } else { GlassLevel::Thin },
                Paint::Fixed(DESTRUCTIVE),
            ),
        ),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(16.0, 0.0), ..row(0.0) },
    );
    let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnColor(DESTRUCTIVE));
    }
}

/// The current account as the Account page shows it.
#[derive(Clone, Debug)]
pub struct AccountInfo {
    pub name: String,
    pub initials: String,
    pub color: Color,
    pub has_password: bool,
    /// The profile picture, if the account has one (else the initials show).
    pub picture: Option<Image>,
    /// How many accounts there are (the only one can't be deleted).
    pub count: usize,
}

impl Default for AccountInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            initials: String::new(),
            color: ACCENTS[0],
            has_password: false,
            picture: None,
            count: 1,
        }
    }
}

/// The account password fields on the page.
#[derive(Clone, Copy, Debug)]
struct PasswordFields {
    current: Option<NodeId>,
    new: NodeId,
    confirm: NodeId,
}

/// A text field in a rounded box, `width` wide; `secure` hides what is typed.
fn text_field(ui: &mut Ui, parent: NodeId, placeholder: &str, width: f32, secure: bool) -> NodeId {
    let field_box = ui.add(
        parent,
        Widget::container().with_background(Background::Fill(Paint::Label, 0.06)).with_radius(12.0),
        Style { size: Size { width: auto(), height: length(38.0) }, padding: padding(12.0, 0.0), ..row(0.0) },
    );
    let field = ui.add_text_field(
        field_box,
        placeholder,
        TextStyle::Body,
        Style { size: Size { width: length(width), height: length(22.0) }, ..Default::default() },
    );
    if secure {
        crate::eye::add(ui, field_box, field);
    }
    field
}

/// The accent swatches, `current` ringed; a click sends `action(index)`.
fn swatches(ui: &mut Ui, parent: NodeId, current: Color, action: fn(usize) -> SettingsAction) {
    for (i, &color) in ACCENTS.iter().enumerate() {
        let selected = current == color;
        let swatch = ui.add(
            parent,
            Widget::container().with_action(action(i).id()).with_radius(CAPSULE).with_background(
                if selected { Background::Fill(Paint::Label, 0.9) } else { Background::None },
            ),
            Style {
                size: Size { width: length(30.0), height: length(30.0) },
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        ui.add(
            swatch,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(color), 1.0))
                .with_radius(CAPSULE),
            Style { size: Size { width: length(24.0), height: length(24.0) }, ..Default::default() },
        );
    }
}

/// A round avatar: the account's picture (clipped to a circle) or, without one, its initials
/// on its colour.
pub fn avatar(
    ui: &mut Ui,
    parent: NodeId,
    initials: &str,
    color: Color,
    side: f32,
    picture: Option<&Image>,
) -> NodeId {
    let circle = ui.add(
        parent,
        Widget::container().with_background(Background::Fill(Paint::Fixed(color), 1.0)).with_radius(CAPSULE),
        Style {
            size: Size { width: length(side), height: length(side) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    if let Some(picture) = picture {
        picture_fill(ui, circle, picture, side);
        return circle;
    }
    let style = if side >= 40.0 { TextStyle::Title2 } else { TextStyle::Callout };
    let label = ui.add_label(circle, initials, style, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
    }
    circle
}

/// A round picture covering `circle` (an avatar of `side` px).
pub fn picture_fill(ui: &mut Ui, circle: NodeId, picture: &Image, side: f32) -> NodeId {
    let node = ui.add(
        circle,
        Widget::image().with_radius(side / 2.0),
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(0.0), right: auto(), top: length(0.0), bottom: auto() },
            size: Size { width: length(side), height: length(side) },
            ..Default::default()
        },
    );
    ui.set_image(node, Some(picture.clone()));
    node
}

/// A segmented control: options as capsules, the current one filled.
pub fn segmented(ui: &mut Ui, parent: NodeId, options: &[(&str, ActionId)], current: Option<usize>) {
    let style = Style {
        size: Size { width: auto(), height: length(SEGMENTED_HEIGHT) },
        // Squeezed, the segments' labels end in an ellipsis instead of running out of the pill.
        min_size: Size { width: length(0.0), height: auto() },
        // Gives way after the row's label, which wraps instead.
        flex_shrink: 0.25,
        padding: padding(SEGMENTED_INSET, SEGMENTED_INSET),
        ..row(2.0)
    };
    let background = Background::Fill(Paint::Label, 0.07);
    // With a selection, the Liquid Glass control of the tab overview carries the pill (a tap
    // reports the segment's action); without one the segments are plain.
    let widget = match current {
        Some(i) => Widget::segmented(i),
        None => Widget::container(),
    };
    let group = ui.add(parent, widget.with_background(background).with_radius(CAPSULE), style);
    for (i, (label, action)) in options.iter().enumerate() {
        let segment = ui.add(
            group,
            Widget::container().with_action(*action).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(SEGMENTED_HEIGHT - 2.0 * SEGMENTED_INSET) },
                min_size: Size { width: length(0.0), height: auto() },
                padding: padding(14.0, 0.0),
                ..row(0.0)
            },
        );
        let style = if Some(i) == current { TextStyle::BodyEmphasis } else { TextStyle::Body };
        ui.add_label(
            segment,
            label,
            style,
            Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
        );
    }
}

/// Height of a settings segmented control and the gap between its edge and the pill.
const SEGMENTED_HEIGHT: f32 = 36.0;
const SEGMENTED_INSET: f32 = 3.0;

fn shortcut_actions() -> Vec<KeyAction> {
    use KeyAction::*;
    vec![
        NewTab,
        CloseTab,
        ReopenClosedTab,
        SearchTabs,
        NextTab,
        PreviousTab,
        PinTab,
        TabOverview,
        FocusAddressBar,
        Back,
        Forward,
        Reload,
        HardReload,
        Find,
        FindNext,
        FindPrevious,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Print,
        Screenshot,
        BookmarkPage,
        ShowBookmarks,
        ShowHistory,
        OpenPlaylist,
        PlaylistAdd,
        OpenPasswords,
        ToggleSidebar,
        OpenSettings,
        CommandPalette,
        ToggleFullscreen,
        Quit,
        Lock,
        DevTools,
        DevToolsConsole,
        ViewSource,
        MediaPlayPause,
        MediaSeekBack,
        MediaSeekForward,
        MediaVolumeUp,
        MediaVolumeDown,
        MediaMute,
        MediaFullscreen,
        MediaCaptions,
        MediaSlower,
        MediaFaster,
    ]
}

/// The heading a shortcut is listed under on the Keyboard page; consecutive actions in
/// `shortcut_actions` with the same heading share one section.
pub(crate) fn shortcut_group(action: KeyAction) -> &'static str {
    use KeyAction::*;
    match action {
        NewTab | CloseTab | ReopenClosedTab | SearchTabs | NextTab | PreviousTab | PinTab | TabOverview
        | SelectTab(_) | SelectLastTab => tr("Tabs"),
        FocusAddressBar | Back | Forward | Reload | HardReload => tr("Navigation"),
        Find | FindNext | FindPrevious | ZoomIn | ZoomOut | ZoomReset | Paste | PastePlainText | Print
        | Screenshot => tr("Page"),
        BookmarkPage | ShowBookmarks | ShowHistory | OpenPlaylist | PlaylistAdd | OpenPasswords
        | ToggleSidebar => tr("Panels"),
        Lock => tr("Privacy"),
        DevTools | DevToolsConsole | ViewSource => tr("Developer"),
        OpenSettings | CommandPalette | ToggleFullscreen | Quit => tr("Window"),
        _ => tr("Media (fullscreen video)"),
    }
}

/// The name a shortcut is listed under, translated.
pub(crate) fn shortcut_name(action: KeyAction) -> &'static str {
    tr(shortcut_source_name(action))
}

/// The shortcut's English name.
fn shortcut_source_name(action: KeyAction) -> &'static str {
    use KeyAction::*;
    match action {
        NewTab => tr_mark("New tab"),
        CloseTab => tr_mark("Close tab"),
        ReopenClosedTab => tr_mark("Reopen closed tab"),
        SearchTabs => tr_mark("Search tabs"),
        NextTab => tr_mark("Next tab"),
        PreviousTab => tr_mark("Previous tab"),
        PinTab => tr_mark("Pin tab"),
        TabOverview => tr_mark("Tab overview"),
        SelectTab(_) => tr_mark("Select tab"),
        SelectLastTab => tr_mark("Select last tab"),
        FocusAddressBar => tr_mark("Focus address bar"),
        Back => tr_mark("Back"),
        Forward => tr_mark("Forward"),
        Reload => tr_mark("Reload"),
        HardReload => tr_mark("Hard reload"),
        Find => tr_mark("Find"),
        FindNext => tr_mark("Find next"),
        FindPrevious => tr_mark("Find previous"),
        Paste => tr_mark("Paste"),
        PastePlainText => tr_mark("Paste plain text"),
        ZoomIn => tr_mark("Zoom in"),
        ZoomOut => tr_mark("Zoom out"),
        ZoomReset => tr_mark("Zoom reset"),
        BookmarkPage => tr_mark("Bookmark page"),
        ShowBookmarks => tr_mark("Show bookmarks"),
        ShowHistory => tr_mark("Show history"),
        OpenPlaylist => tr_mark("Open playlist"),
        PlaylistAdd => tr_mark("Playlist add"),
        ToggleReader => tr_mark("Toggle reader view"),
        TranslatePage => tr_mark("Translate page"),
        OpenPasswords => tr_mark("Open passwords"),
        ToggleSidebar => tr_mark("Toggle sidebar"),
        OpenSettings => tr_mark("Open settings"),
        CommandPalette => tr_mark("Command palette"),
        ToggleFullscreen => tr_mark("Toggle fullscreen"),
        Quit => tr_mark("Quit"),
        Lock => tr_mark("Lock"),
        DevTools => tr_mark("Developer tools"),
        DevToolsConsole => tr_mark("Developer console"),
        ViewSource => tr_mark("View source"),
        Print => tr_mark("Print or save as PDF"),
        Screenshot => tr_mark("Screenshot"),
        MediaPlayPause => tr_mark("Media play pause"),
        MediaSeekBack => tr_mark("Media seek back"),
        MediaSeekForward => tr_mark("Media seek forward"),
        MediaVolumeUp => tr_mark("Media volume up"),
        MediaVolumeDown => tr_mark("Media volume down"),
        MediaMute => tr_mark("Media mute"),
        MediaFullscreen => tr_mark("Media fullscreen"),
        MediaCaptions => tr_mark("Media captions"),
        MediaSlower => tr_mark("Media slower"),
        MediaFaster => tr_mark("Media faster"),
    }
}

/// A tiny picture of a browser window with the given tab bar layout.
fn tab_bar_picture(ui: &mut Ui, parent: NodeId, style: TabBarStyle, selected: bool) {
    let accent = if selected { Paint::Accent } else { Paint::Secondary };
    let window = ui.add(
        parent,
        Widget::container()
            .with_background(Background::Fill(Paint::Surface, 1.0))
            .with_radius(8.0)
            .with_shadow(axion_ui::Shadow { offset_y: 1.0, blur: 4.0, opacity: 0.15 }),
        Style {
            size: Size { width: length(96.0), height: length(64.0) },
            flex_direction: FlexDirection::Column,
            gap: Size { width: length(0.0), height: length(3.0) },
            padding: padding(5.0, 5.0),
            ..Default::default()
        },
    );
    let bar = |ui: &mut Ui, parent: NodeId, width: Dimension, opacity: f32| {
        ui.add(
            parent,
            Widget::container().with_background(Background::Fill(accent, opacity)).with_radius(CAPSULE),
            Style { size: Size { width, height: length(5.0) }, flex_shrink: 0.0, ..Default::default() },
        );
    };
    match style {
        TabBarStyle::Separate => {
            let top = ui.add(
                window,
                Widget::container(),
                Style { justify_content: JustifyContent::CENTER, ..row(0.0) },
            );
            bar(ui, top, length(40.0), 0.9);
            let tabs = ui.add(window, Widget::container(), row(3.0));
            for opacity in [0.9, 0.35, 0.35] {
                let slot = ui.add(tabs, Widget::container(), Style { flex_grow: 1.0, ..Default::default() });
                bar(ui, slot, percent(1.0), opacity);
            }
        }
        TabBarStyle::Compact => {
            let tabs = ui.add(window, Widget::container(), row(3.0));
            for (opacity, grow) in [(0.9, 2.0), (0.35, 1.0), (0.35, 1.0)] {
                let slot = ui.add(tabs, Widget::container(), Style { flex_grow: grow, ..Default::default() });
                bar(ui, slot, percent(1.0), opacity);
            }
        }
    }
}

/// 2.0 → "2", 2.5 → "2.5".
fn trim_number(v: f32) -> String {
    let text = format!("{v:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The memory slider's upper end: budgets above this are unrealistic for tabs (a typed
/// value in the config can still go higher).
const MEMORY_SLIDER_MAX_MB: f32 = 6144.0;

fn memory_slider_max_mb() -> f32 {
    let system = axion_tabs::system_memory().map_or(f32::MAX, |b| (b / (1024 * 1024)) as f32);
    MEMORY_SLIDER_MAX_MB.min(system)
}

/// 1536 MiB → "1.5 GB".
pub fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_round_trip_through_config() {
        let mut config = Config::default();
        for setting in [
            Setting::TextScale,
            Setting::Refraction,
            Setting::Blur,
            Setting::Squash,
            Setting::MergeDistance,
            Setting::DragStretch,
        ] {
            setting.set(&mut config, 0.5);
            // Values are rounded to sensible steps (text size to 0.05), so allow one step.
            assert!((setting.get(&config) - 0.5).abs() < 0.12, "{setting:?}: {}", setting.get(&config));
        }
        Setting::ReduceMotion.set(&mut config, 1.0);
        assert!(config.glass.reduce_motion);
    }

    #[test]
    fn icon_weight_presets_set_both_multipliers() {
        let mut config = Config::default();
        // The default is the Medium preset; dark stays as designed.
        assert_eq!(icon_weight_preset(&config), Some(1));
        assert_eq!(config.appearance.icon_stroke_dark, 1.0);
        for (i, &(_, light, dark)) in ICON_WEIGHTS.iter().enumerate() {
            set_icon_weight(&mut config, i);
            assert_eq!(
                (config.appearance.icon_stroke_light, config.appearance.icon_stroke_dark),
                (light, dark)
            );
            assert_eq!(icon_weight_preset(&config), Some(i));
        }
        config.appearance.icon_stroke_light = 1.33;
        assert_eq!(icon_weight_preset(&config), None);
    }

    #[test]
    fn action_ids_round_trip() {
        for action in [
            SettingsAction::Close,
            SettingsAction::Category(Category::Privacy),
            SettingsAction::Theme(ThemeMode::Dark),
            SettingsAction::Language(UiLanguage::German),
            SettingsAction::GlassStyle(GlassStyle::Liquid),
            SettingsAction::GlassStyle(GlassStyle::Flat),
            SettingsAction::IconWeight(2),
            SettingsAction::SearchEngine(4),
            SettingsAction::Accent(5),
            SettingsAction::ClearHistory,
            SettingsAction::ShieldLevel(ShieldLevel::Aggressive),
            SettingsAction::UpdateFilterLists,
            SettingsAction::AddTrustedSite,
            SettingsAction::RemoveTrustedSite(7),
            SettingsAction::Category(Category::Data),
            SettingsAction::ChooseDownloadFolder,
            SettingsAction::ChoosePlaylistFolder,
            SettingsAction::ResetPlaylistFolder,
            SettingsAction::RemoveDownloads(RemoveDownloads::OnQuit),
            SettingsAction::ClearRange(ClearRange::LastWeek),
            SettingsAction::ClearBrowsingData,
            SettingsAction::RemoveAllSiteData,
            SettingsAction::RemoveSiteData(99),
            SettingsAction::PermissionDefault(0, PermissionDefault::Ask),
            SettingsAction::PermissionDefault(6, PermissionDefault::Block),
            SettingsAction::ResetPermissions,
            SettingsAction::ForgetPermission(0),
            SettingsAction::ForgetPermission(PERMISSION_SITE_LIMIT - 1),
            SettingsAction::RemoveTrustedSite(399),
            SettingsAction::Category(Category::Passwords),
            SettingsAction::ExportPasswords,
            SettingsAction::RestorePasswords,
            SettingsAction::Category(Category::Account),
            SettingsAction::Category(Category::General),
            SettingsAction::AccountColor(5),
            SettingsAction::ChooseAccountPhoto,
            SettingsAction::RemoveAccountPhoto,
            SettingsAction::SaveAccountName,
            SettingsAction::DeleteAccount,
            SettingsAction::SetPassword,
            SettingsAction::NoBackground,
            SettingsAction::ChooseBackground,
            SettingsAction::AddBackgroundPath,
            SettingsAction::Background(0),
            SettingsAction::Background(BACKGROUND_SLOTS - 1),
            SettingsAction::RemoveBackground(0),
            SettingsAction::RemoveBackground(BACKGROUND_SLOTS - 1),
            SettingsAction::CheckForUpdates,
            SettingsAction::InstallUpdate,
            SettingsAction::SkipUpdate,
            SettingsAction::RemindLater,
            SettingsAction::OpenReleasePage,
            SettingsAction::SaveRepository,
            SettingsAction::RelaunchNow,
            SettingsAction::PageFrames(PageFrames::Auto),
            SettingsAction::PageFrames(PageFrames::Gpu),
            SettingsAction::PageFrames(PageFrames::Software),
            SettingsAction::GpuCard(GpuChoice::Auto),
            SettingsAction::GpuCard(GpuChoice::Low),
            SettingsAction::GpuCard(GpuChoice::High),
            SettingsAction::CopyDiagnostics,
            SettingsAction::RestartAxion,
            SettingsAction::Category(Category::Hardware),
            SettingsAction::Category(Category::About),
        ] {
            assert_eq!(SettingsAction::from_id(action.id()), Some(action));
        }
        // Clear of the other overlays' ids (the lock screen starts at 9000).
        assert!(SettingsAction::RemoveBackground(BACKGROUND_SLOTS - 1).id().0 < 9000);
        // The About page's ids follow the background picker's.
        assert_eq!(
            SettingsAction::from_id(ActionId(ACTION_BASE + REMOVE_BACKGROUND_BASE + BACKGROUND_SLOTS as u32)),
            Some(SettingsAction::CheckForUpdates)
        );
        assert_eq!(SettingsAction::from_id(ActionId(ACTION_BASE + UPDATE_BASE + UPDATE_ACTIONS)), None);
        assert!(SettingsAction::RelaunchNow.id().0 < 9000);
        let crop = cover_crop((200, 100), Vec2::new(100.0, 100.0));
        assert!(
            (crop.size.x - 0.5).abs() < 1e-4 && (crop.origin.x - 0.25).abs() < 1e-4 && crop.size.y == 1.0
        );
        assert_eq!(shortcut_source_name(KeyAction::FocusAddressBar), "Focus address bar");
        // Same-group actions are contiguous, so every heading appears once.
        let mut seen = Vec::new();
        for action in shortcut_actions() {
            let group = shortcut_group(action);
            if seen.last() != Some(&group) {
                assert!(!seen.contains(&group), "{group} split");
                seen.push(group);
            }
        }
    }
}
