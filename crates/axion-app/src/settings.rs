//! The settings sheet (design: "Settings"): a large glass sheet over the dimmed browser with
//! categories on the left. Every change is written to `config.toml` at once.

use std::path::PathBuf;

use axion_compositor::Icon;
use axion_config::{Action as KeyAction, Color, Config, Keymap, ShieldLevel, TabBarStyle, ThemeMode};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

mod search;

const ACTION_BASE: u32 = 8000;
const SHEET_MAX: Vec2 = Vec2::new(920.0, 660.0);
const SHEET_MARGIN: f32 = 32.0;
const SHEET_RADIUS: f32 = 30.0;
const NAV_WIDTH: f32 = 230.0;
const ROW_HEIGHT: f32 = 46.0;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// Width of the value label beside a slider.
const VALUE_WIDTH: f32 = 150.0;
const SLIDER_WIDTH: f32 = 180.0;
/// Grouped list (Safari's settings): rounded cards of rows with hairline separators.
const GROUP_RADIUS: f32 = 14.0;
const GROUP_INSET: f32 = 16.0;
const GROUP_GAP: f32 = 22.0;
const BACKDROP_DIM: f32 = 0.28;

pub const SEARCH_ENGINES: [(&str, &str); 5] = [
    ("DuckDuckGo", "https://duckduckgo.com/?q={}"),
    ("Google", "https://www.google.com/search?q={}"),
    ("Brave", "https://search.brave.com/search?q={}"),
    ("Startpage", "https://www.startpage.com/sp/search?query={}"),
    ("Ecosia", "https://www.ecosia.org/search?q={}"),
];

/// Accent colours from the design system.
const ACCENTS: [u32; 6] = [0x2E6BFF, 0x5B5BF0, 0xE2457A, 0xF2742B, 0x1F9D57, 0x5E636E];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    General,
    Appearance,
    LiquidGlass,
    Tabs,
    Media,
    Keyboard,
    Shields,
    Privacy,
    Passwords,
}

impl Category {
    const ALL: [Category; 9] = [
        Category::General,
        Category::Appearance,
        Category::LiquidGlass,
        Category::Tabs,
        Category::Media,
        Category::Keyboard,
        Category::Shields,
        Category::Privacy,
        Category::Passwords,
    ];

    fn title(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Appearance => "Appearance",
            Category::LiquidGlass => "Liquid Glass",
            Category::Tabs => "Tabs & Memory",
            Category::Media => "Media",
            Category::Keyboard => "Keyboard",
            Category::Shields => "Shields",
            Category::Privacy => "Privacy",
            Category::Passwords => "Passwords",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Category::General => Icon::Settings,
            Category::Appearance => Icon::Sidebar,
            Category::LiquidGlass => Icon::Overview,
            Category::Tabs => Icon::TabGroup,
            Category::Media => Icon::Media,
            Category::Keyboard => Icon::Menu,
            Category::Shields => Icon::Shield,
            Category::Privacy => Icon::Lock,
            Category::Passwords => Icon::Key,
        }
    }
}

/// A setting edited with a toggle or slider; sliders map 0–1 onto the setting's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    TintFromPage,
    AdaptToPage,
    TextScale,
    PageCorner,
    ReduceTransparency,
    ReduceMotion,
    Refraction,
    Blur,
    Dispersion,
    Squash,
    MergeDistance,
    LockPrivateTabs,
    LockAfter,
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
    HideBarOnScroll,
    OpenInBackground,
    OwnFullscreenControls,
    HideControlsAfter,
    HardwareDecoding,
    DesktopIntegration,
    OfferToSavePasswords,
    AutofillPasswords,
    PasswordsLockAfter,
    ClearClipboardAfter,
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Setting::TintFromPage => "Colour in tab bar",
            Setting::AdaptToPage => "Light or dark chrome to match the page",
            Setting::TextScale => "Text size",
            Setting::PageCorner => "Page corner radius",
            Setting::ReduceTransparency => "Reduce transparency",
            Setting::ReduceMotion => "Reduce motion",
            Setting::Refraction => "Refraction",
            Setting::Blur => "Blur",
            Setting::Dispersion => "Colour dispersion",
            Setting::Squash => "Squash when moving",
            Setting::MergeDistance => "Merge distance",
            Setting::LockPrivateTabs => "Lock private tabs with a password",
            Setting::LockAfter => "Lock after time in background",
            Setting::HideAdElements => "Hide empty ad spaces",
            Setting::BlockPopups => "Block popups opened without a click",
            Setting::BlockRedirects => "Block redirects to ad and tracker sites",
            Setting::StripTracking => "Remove tracking parameters from links",
            Setting::ThirdPartyCookies => "Block third-party cookies",
            Setting::FreezeAfter => "Freeze background tabs after",
            Setting::DiscardAfter => "Unload background tabs after",
            Setting::LimitMemory => "Unload tabs when memory runs over the budget",
            Setting::MemoryBudget => "Memory budget",
            Setting::KeepMediaAwake => "Keep tabs playing audio awake",
            Setting::RestoreSession => "Reopen last session's tabs at start",
            Setting::HideBarOnScroll => "Hide tab bar when scrolling",
            Setting::OpenInBackground => "Open new tabs in background",
            Setting::OwnFullscreenControls => "Axion's player in fullscreen",
            Setting::HideControlsAfter => "Hide controls while playing after",
            Setting::HardwareDecoding => "Hardware video decoding (after restart)",
            Setting::DesktopIntegration => "Desktop media controls and media keys (after restart)",
            Setting::OfferToSavePasswords => "Offer to save passwords",
            Setting::AutofillPasswords => "Offer to fill saved passwords",
            Setting::PasswordsLockAfter => "Lock Passwords after",
            Setting::ClearClipboardAfter => "Clear copied passwords from the clipboard after",
        }
    }

    fn is_toggle(self) -> bool {
        matches!(
            self,
            Setting::TintFromPage
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
                | Setting::HideBarOnScroll
                | Setting::OpenInBackground
                | Setting::OwnFullscreenControls
                | Setting::HardwareDecoding
                | Setting::DesktopIntegration
                | Setting::OfferToSavePasswords
                | Setting::AutofillPasswords
        )
    }

    /// Slider range (min, max).
    fn range(self) -> (f32, f32) {
        match self {
            Setting::TextScale => (0.85, 1.3),
            Setting::PageCorner => (0.0, 24.0),
            Setting::Refraction => (0.0, 1.0),
            // Blur is a factor on the default radii of all three thicknesses.
            Setting::Blur => (0.0, 2.0),
            Setting::Dispersion => (0.0, 0.4),
            Setting::Squash => (0.0, 0.2),
            Setting::MergeDistance => (0.0, 40.0),
            Setting::LockAfter => (0.0, 600.0),
            Setting::FreezeAfter => (0.0, 60.0),
            Setting::HideControlsAfter => (1.0, 10.0),
            Setting::DiscardAfter => (0.0, 480.0),
            Setting::MemoryBudget => (0.0, memory_slider_max_mb()),
            Setting::PasswordsLockAfter => (0.0, 120.0),
            Setting::ClearClipboardAfter => (0.0, 300.0),
            _ => (0.0, 1.0),
        }
    }

    /// The slider's current value for people: "Never", "30 min", "2 h", "100 %", …
    fn display(self, c: &Config) -> String {
        let percent = |v: f32| format!("{:.0} %", v * 100.0);
        let minutes = |m: f32| match m {
            m if m <= 0.0 => "Never".to_owned(),
            m if m < 60.0 => format!("{m:.0} min"),
            m => format!("{} h", trim_number(m / 60.0)),
        };
        let defaults = Config::default();
        match self {
            Setting::TextScale => percent(c.appearance.text_scale),
            Setting::PageCorner => format!("{:.0} px", c.layout.page_corner_radius),
            Setting::Refraction => percent(c.glass.light.refraction),
            Setting::Blur => {
                percent(c.glass.light.blur.regular / defaults.glass.light.blur.regular.max(0.001))
            }
            Setting::Dispersion => percent(c.glass.light.dispersion),
            Setting::Squash => percent(c.glass.squash),
            Setting::MergeDistance => format!("{:.0} px", c.glass.merge_distance),
            Setting::LockAfter => match c.privacy.lock_after_seconds {
                s if s <= 0.0 => "Immediately".to_owned(),
                s if s < 60.0 => format!("{s:.0} s"),
                s => format!("{} min", trim_number(s / 60.0)),
            },
            Setting::FreezeAfter => minutes(c.tabs.freeze_after_minutes),
            Setting::DiscardAfter => minutes(c.tabs.discard_after_minutes),
            Setting::MemoryBudget => match c.tabs.memory_budget_mb {
                0 => {
                    let mut auto = c.clone();
                    auto.tabs.limit_memory = true;
                    crate::lifecycle::memory_budget(&auto)
                        .map_or("Automatic".to_owned(), |b| format!("Automatic ({:.0} GB)", b as f64 / GIB))
                }
                mb => format_bytes(u64::from(mb) * 1024 * 1024),
            },
            Setting::HideControlsAfter => format!("{} s", trim_number(c.media.hide_controls_after_seconds)),
            Setting::PasswordsLockAfter => minutes(c.passwords.auto_lock_minutes),
            Setting::ClearClipboardAfter => match c.passwords.clear_clipboard_seconds {
                s if s <= 0.0 => "Never".to_owned(),
                s if s < 60.0 => format!("{s:.0} s"),
                s => format!("{} min", trim_number(s / 60.0)),
            },
            _ => String::new(),
        }
    }

    fn get(self, c: &Config) -> f32 {
        let defaults = Config::default();
        let raw = match self {
            Setting::TintFromPage => return c.appearance.tint_from_page as u8 as f32,
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
            Setting::HideBarOnScroll => return c.layout.hide_bar_on_scroll as u8 as f32,
            Setting::OpenInBackground => return c.tabs.open_links_in_background as u8 as f32,
            Setting::OwnFullscreenControls => return c.media.fullscreen_player as u8 as f32,
            Setting::HardwareDecoding => return c.media.hardware_decoding as u8 as f32,
            Setting::DesktopIntegration => return c.media.desktop_integration as u8 as f32,
            Setting::OfferToSavePasswords => return c.passwords.offer_to_save as u8 as f32,
            Setting::AutofillPasswords => return c.passwords.autofill as u8 as f32,
            Setting::PasswordsLockAfter => c.passwords.auto_lock_minutes,
            Setting::ClearClipboardAfter => c.passwords.clear_clipboard_seconds,
            Setting::HideControlsAfter => c.media.hide_controls_after_seconds,
            Setting::FreezeAfter => c.tabs.freeze_after_minutes,
            Setting::DiscardAfter => c.tabs.discard_after_minutes,
            Setting::MemoryBudget => c.tabs.memory_budget_mb as f32,
            Setting::LockAfter => c.privacy.lock_after_seconds,
            Setting::TextScale => c.appearance.text_scale,
            Setting::PageCorner => c.layout.page_corner_radius,
            Setting::Refraction => c.glass.light.refraction,
            Setting::Blur => c.glass.light.blur.regular / defaults.glass.light.blur.regular,
            Setting::Dispersion => c.glass.light.dispersion,
            Setting::Squash => c.glass.squash,
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
            Setting::TintFromPage => c.appearance.tint_from_page = on,
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
            Setting::HideBarOnScroll => c.layout.hide_bar_on_scroll = on,
            Setting::OpenInBackground => c.tabs.open_links_in_background = on,
            Setting::OwnFullscreenControls => c.media.fullscreen_player = on,
            Setting::HardwareDecoding => c.media.hardware_decoding = on,
            Setting::DesktopIntegration => c.media.desktop_integration = on,
            Setting::OfferToSavePasswords => c.passwords.offer_to_save = on,
            Setting::AutofillPasswords => c.passwords.autofill = on,
            Setting::PasswordsLockAfter => c.passwords.auto_lock_minutes = v.round(),
            Setting::ClearClipboardAfter => c.passwords.clear_clipboard_seconds = (v / 5.0).round() * 5.0,
            Setting::HideControlsAfter => c.media.hide_controls_after_seconds = (v * 2.0).round() / 2.0,
            Setting::FreezeAfter => c.tabs.freeze_after_minutes = v.round(),
            Setting::DiscardAfter => c.tabs.discard_after_minutes = (v / 5.0).round() * 5.0,
            Setting::MemoryBudget => c.tabs.memory_budget_mb = ((v / 256.0).round() * 256.0) as u32,
            Setting::LockAfter => c.privacy.lock_after_seconds = (v / 10.0).round() * 10.0,
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
    SearchEngine(usize),
    Accent(usize),
    ClearHistory,
    SetPassword,
    RemovePassword,
    ShieldLevel(ShieldLevel),
    UpdateFilterLists,
    TabBar(TabBarStyle),
    CloseAfter(u32),
    AddTrustedSite,
    /// Index into `media.trusted_player_sites`.
    RemoveTrustedSite(usize),
    OpenPasswords,
    ImportPasswords,
    ExportPasswords,
}

/// Remove buttons of the trusted player sites (one id per row).
const TRUSTED_SITE_BASE: u32 = 200;

const CLOSE_AFTER: [(&str, u32); 4] =
    [("Manually", 0), ("After a Day", 1), ("After a Week", 7), ("After a Month", 30)];

const SHIELD_LEVELS: [(&str, ShieldLevel); 3] =
    [("Off", ShieldLevel::Off), ("Standard", ShieldLevel::Standard), ("Aggressive", ShieldLevel::Aggressive)];

impl SettingsAction {
    fn id(self) -> ActionId {
        ActionId(
            ACTION_BASE
                + match self {
                    SettingsAction::Close => 0,
                    SettingsAction::Category(c) => {
                        10 + Category::ALL.iter().position(|&x| x == c).unwrap_or(0) as u32
                    }
                    SettingsAction::Theme(ThemeMode::Auto) => 20,
                    SettingsAction::Theme(ThemeMode::Light) => 21,
                    SettingsAction::Theme(ThemeMode::Dark) => 22,
                    SettingsAction::SearchEngine(i) => 30 + i as u32,
                    SettingsAction::Accent(i) => 40 + i as u32,
                    SettingsAction::ClearHistory => 50,
                    SettingsAction::SetPassword => 51,
                    SettingsAction::RemovePassword => 52,
                    SettingsAction::ShieldLevel(level) => {
                        60 + SHIELD_LEVELS.iter().position(|(_, l)| *l == level).unwrap_or(0) as u32
                    }
                    SettingsAction::UpdateFilterLists => 63,
                    SettingsAction::TabBar(TabBarStyle::Separate) => 70,
                    SettingsAction::TabBar(TabBarStyle::Compact) => 71,
                    SettingsAction::CloseAfter(days) => {
                        72 + CLOSE_AFTER.iter().position(|(_, d)| *d == days).unwrap_or(0) as u32
                    }
                    SettingsAction::AddTrustedSite => 80,
                    SettingsAction::OpenPasswords => 90,
                    SettingsAction::ImportPasswords => 91,
                    SettingsAction::ExportPasswords => 92,
                    SettingsAction::RemoveTrustedSite(i) => TRUSTED_SITE_BASE + i as u32,
                },
        )
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        let n = id.0.checked_sub(ACTION_BASE)?;
        Some(match n {
            0 => SettingsAction::Close,
            10..=18 => SettingsAction::Category(Category::ALL[(n - 10) as usize]),
            20 => SettingsAction::Theme(ThemeMode::Auto),
            21 => SettingsAction::Theme(ThemeMode::Light),
            22 => SettingsAction::Theme(ThemeMode::Dark),
            30..=39 if ((n - 30) as usize) < SEARCH_ENGINES.len() => {
                SettingsAction::SearchEngine((n - 30) as usize)
            }
            40..=49 if ((n - 40) as usize) < ACCENTS.len() => SettingsAction::Accent((n - 40) as usize),
            50 => SettingsAction::ClearHistory,
            51 => SettingsAction::SetPassword,
            52 => SettingsAction::RemovePassword,
            60..=62 => SettingsAction::ShieldLevel(SHIELD_LEVELS[(n - 60) as usize].1),
            63 => SettingsAction::UpdateFilterLists,
            70 => SettingsAction::TabBar(TabBarStyle::Separate),
            71 => SettingsAction::TabBar(TabBarStyle::Compact),
            72..=75 => SettingsAction::CloseAfter(CLOSE_AFTER[(n - 72) as usize].1),
            80 => SettingsAction::AddTrustedSite,
            90 => SettingsAction::OpenPasswords,
            91 => SettingsAction::ImportPasswords,
            92 => SettingsAction::ExportPasswords,
            n if (TRUSTED_SITE_BASE..TRUSTED_SITE_BASE + 800).contains(&n) => {
                SettingsAction::RemoveTrustedSite((n - TRUSTED_SITE_BASE) as usize)
            }
            _ => return None,
        })
    }
}

/// What the browser should do after a click or edit in the sheet.
pub enum Outcome {
    Nothing,
    /// The configuration changed; save and apply it.
    Changed,
    Close,
    ClearHistory,
    SetPassword(String),
    RemovePassword,
    UpdateFilterLists,
    OpenPasswords,
    ImportPasswords(PathBuf),
    ExportPasswords(PathBuf),
}

pub struct SettingsSheet {
    root: NodeId,
    sheet: NodeId,
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
    /// The "add a trusted player site" field (Media page).
    trusted_site: Option<NodeId>,
    password: Option<NodeId>,
    has_password: bool,
    /// The import/export file field (Passwords page) and the last import/export result.
    passwords_file: Option<NodeId>,
    passwords_status: Option<String>,
    /// The browser's memory use, shown on the Tabs page.
    memory_used: Option<u64>,
    confirm_clear: bool,
    open: bool,
    /// The search field above the categories, and what it held when the page was built.
    search: NodeId,
    query: String,
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
            "Settings",
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
            "Search",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let mut nav = Vec::new();
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
            nav.push((category, item));
        }
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
        Self {
            root,
            sheet,
            nav,
            content,
            viewport,
            scroll: 0.0,
            shown_category: Category::General,
            category: Category::General,
            controls: Vec::new(),
            values: Vec::new(),
            homepage: None,
            trusted_site: None,
            password: None,
            has_password: false,
            passwords_file: None,
            passwords_status: None,
            memory_used: None,
            group: None,
            confirm_clear: false,
            open: false,
            search,
            query: String::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether `p` (logical px) is on the sheet itself (backdrop clicks outside close it).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.sheet).contains(p)
    }

    /// Whether a private browsing password exists (changes the Privacy page).
    pub fn set_has_password(&mut self, has_password: bool) {
        self.has_password = has_password;
    }

    pub fn password_field(&self) -> Option<NodeId> {
        self.password
    }

    pub fn trusted_site_field(&self) -> Option<NodeId> {
        self.trusted_site
    }

    pub fn homepage_field(&self) -> Option<NodeId> {
        self.homepage
    }

    /// Shows the result of the last password import or export (Passwords page).
    pub fn set_passwords_status(&mut self, status: String) {
        self.passwords_status = Some(status);
    }

    pub fn open(&mut self, ui: &mut Ui, config: &Config, keymap: &Keymap, window: Vec2) {
        self.open = true;
        ui.set_visible(self.root, true);
        self.resize(ui, window);
        self.show(ui, config, keymap);
    }

    pub fn set_memory_used(&mut self, used: Option<u64>) {
        self.memory_used = used;
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

    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
        self.query.clear();
        ui.set_field_text(self.search, "");
        self.confirm_clear = false;
        ui.set_visible(self.root, false);
    }

    pub fn resize(&mut self, ui: &mut Ui, window: Vec2) {
        let size = (window - Vec2::splat(2.0 * SHEET_MARGIN)).min(SHEET_MAX).max(Vec2::new(480.0, 360.0));
        let origin = ((window - size) / 2.0).max(Vec2::ZERO);
        let mut style = ui.style(self.sheet);
        style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
        style.size = Size { width: length(size.x), height: length(size.y) };
        ui.set_style(self.sheet, style);
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
        self.trusted_site = None;
        self.password = None;
        self.passwords_file = None;
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
                let row_node = self.row(ui, "Homepage");
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
                    "https://…",
                    TextStyle::Body,
                    Style { size: Size { width: length(320.0), height: length(22.0) }, ..Default::default() },
                );
                ui.set_field_text(field, &config.general.homepage);
                self.homepage = Some(field);
                let row_node = self.row(ui, "Search engine");
                let current = SEARCH_ENGINES.iter().position(|(_, url)| *url == config.general.search_url);
                let options: Vec<(&str, ActionId)> = SEARCH_ENGINES
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| (*name, SettingsAction::SearchEngine(i).id()))
                    .collect();
                segmented(ui, row_node, &options, current);
            }
            Category::Appearance => {
                let row_node = self.row(ui, "Appearance");
                let themes = [
                    ("Automatic", SettingsAction::Theme(ThemeMode::Auto).id()),
                    ("Light", SettingsAction::Theme(ThemeMode::Light).id()),
                    ("Dark", SettingsAction::Theme(ThemeMode::Dark).id()),
                ];
                let current = [ThemeMode::Auto, ThemeMode::Light, ThemeMode::Dark]
                    .iter()
                    .position(|&m| m == config.appearance.theme);
                segmented(ui, row_node, &themes, current);
                let row_node = self.row(ui, "Accent colour");
                for (i, &hex) in ACCENTS.iter().enumerate() {
                    let selected = config.appearance.accent == Color::rgb(hex);
                    let swatch = ui.add(
                        row_node,
                        Widget::container()
                            .with_action(SettingsAction::Accent(i).id())
                            .with_radius(CAPSULE)
                            .with_background(if selected {
                                Background::Fill(Paint::Label, 0.9)
                            } else {
                                Background::None
                            }),
                        Style {
                            size: Size { width: length(30.0), height: length(30.0) },
                            justify_content: JustifyContent::CENTER,
                            ..row(0.0)
                        },
                    );
                    ui.add(
                        swatch,
                        Widget::container()
                            .with_background(Background::Fill(Paint::Fixed(Color::rgb(hex)), 1.0))
                            .with_radius(CAPSULE),
                        Style {
                            size: Size { width: length(24.0), height: length(24.0) },
                            ..Default::default()
                        },
                    );
                }
                for setting in [Setting::AdaptToPage, Setting::TextScale, Setting::PageCorner] {
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
                    Setting::MergeDistance,
                ] {
                    self.control(ui, config, setting);
                }
            }
            Category::Keyboard => {
                let note = ui.add_label(
                    self.content,
                    "Change shortcuts in ~/.config/axion/keymap.toml — edits apply immediately.",
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
                for action in shortcut_actions() {
                    let chords: Vec<String> =
                        keymap.chords_for(action).iter().map(ToString::to_string).collect();
                    let row_node = self.row_with_height(ui, &humanize(&action.to_string()), 34.0);
                    let label =
                        ui.add_label(row_node, &chords.join("   "), TextStyle::Callout, Style::default());
                    if let Some(w) = ui.widget_mut(label) {
                        w.set_paint(Paint::Secondary);
                    }
                }
            }
            Category::Tabs => {
                self.tab_bar_choice(ui, config);
                for setting in [Setting::TintFromPage, Setting::HideBarOnScroll, Setting::OpenInBackground] {
                    self.control(ui, config, setting);
                }
                let row_node = self.row(ui, "Close tabs");
                let options: Vec<(&str, ActionId)> = CLOSE_AFTER
                    .iter()
                    .map(|(name, days)| (*name, SettingsAction::CloseAfter(*days).id()))
                    .collect();
                let current = CLOSE_AFTER.iter().position(|(_, d)| *d == config.tabs.close_after_days);
                segmented(ui, row_node, &options, current);
                self.note(ui, "Axion can close tabs you haven't looked at for a while.");
                self.section(ui, "Memory");
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
                let budget = crate::lifecycle::memory_budget(config).map_or("none".into(), format_bytes);
                let used = self.memory_used.map_or("not measured yet".into(), format_bytes);
                let row_node = self.row(ui, &format!("In use now: {used} · budget: {budget}"));
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
                self.note(ui, "Pages keep their own player; in fullscreen, Axion's takes over.");
                self.section(ui, "Sites that keep their own player");
                for (i, site) in config.media.trusted_player_sites.iter().enumerate() {
                    let row_node = self.row(ui, site);
                    let remove = ui.add(
                        row_node,
                        Widget::container()
                            .with_action(SettingsAction::RemoveTrustedSite(i).id())
                            .with_radius(CAPSULE)
                            .with_background(Background::Fill(Paint::Label, 0.08)),
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
                let row_node = self.row(ui, "Add a site");
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
                pill_button(ui, row_node, "Add", SettingsAction::AddTrustedSite, false);
                self.note(ui, "Also in fullscreen, with their subdomains. Applies to pages loaded next.");
                self.section(ui, "Playback");
                for setting in [Setting::HardwareDecoding, Setting::DesktopIntegration] {
                    self.control(ui, config, setting);
                }
            }
            Category::Shields => {
                let row_node = self.row(ui, "Shields on sites you haven't set");
                let options: Vec<(&str, ActionId)> = SHIELD_LEVELS
                    .iter()
                    .map(|(name, l)| (*name, SettingsAction::ShieldLevel(*l).id()))
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
                let label = format!(
                    "Filter lists ({}, refreshed every {hours:.0} hours)",
                    config.shields.lists.len()
                );
                let row_node = self.row(ui, &label);
                pill_button(ui, row_node, "Update Now", SettingsAction::UpdateFilterLists, false);
            }
            Category::Privacy => {
                let label =
                    if self.has_password { "Change private tabs password" } else { "Private tabs password" };
                let row_node = self.row(ui, label);
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
                    "New password",
                    TextStyle::Body,
                    Style { size: Size { width: length(200.0), height: length(22.0) }, ..Default::default() },
                );
                ui.set_secure(field, true);
                self.password = Some(field);
                pill_button(ui, row_node, "Save", SettingsAction::SetPassword, false);
                if self.has_password {
                    pill_button(ui, row_node, "Remove", SettingsAction::RemovePassword, true);
                }
                for setting in [Setting::LockPrivateTabs, Setting::LockAfter] {
                    self.control(ui, config, setting);
                }
                let row_node = self.row(ui, "Browsing history");
                let text = if self.confirm_clear { "Click again to clear" } else { "Clear History…" };
                let button = ui.add(
                    row_node,
                    Widget::container()
                        .with_action(SettingsAction::ClearHistory.id())
                        .with_radius(CAPSULE)
                        .with_background(Background::Fill(
                            Paint::Fixed(Color::rgb(0xE2453A)),
                            if self.confirm_clear { 1.0 } else { 0.14 },
                        )),
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
            }
            Category::Passwords => {
                let row_node = self.row(ui, "Saved passwords, cards and addresses");
                pill_button(ui, row_node, "Open Passwords", SettingsAction::OpenPasswords, false);
                for setting in [Setting::OfferToSavePasswords, Setting::AutofillPasswords] {
                    self.control(ui, config, setting);
                }
                self.section(ui, "Security");
                for setting in [Setting::PasswordsLockAfter, Setting::ClearClipboardAfter] {
                    self.control(ui, config, setting);
                }
                self.note(ui, "Everything is encrypted with your master password, which Axion never stores.");
                self.section(ui, "Import and export");
                let row_node = self.row(ui, "File");
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
                let row_node = self.row(ui, "");
                pill_button(ui, row_node, "Import CSV", SettingsAction::ImportPasswords, false);
                pill_button(ui, row_node, "Export Encrypted Copy", SettingsAction::ExportPasswords, false);
                let note = self.passwords_status.clone().unwrap_or_else(|| {
                    "Import reads CSV exports of Chrome, Firefox and Bitwarden. The export is an encrypted copy that opens with the same master password.".to_owned()
                });
                self.note(ui, &note);
            }
        }
    }

    fn row(&mut self, ui: &mut Ui, label: &str) -> NodeId {
        self.row_with_height(ui, label, ROW_HEIGHT)
    }

    /// The settings matching the search, grouped by page: toggles and sliders work right
    /// here, everything else has a button that opens its page.
    fn show_results(&mut self, ui: &mut Ui, config: &Config) {
        ui.add_label(
            self.content,
            "Search Results",
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
                &format!("No settings match “{}”.", self.query),
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
                        let row_node = self.row(ui, title);
                        pill_button(ui, row_node, "Show", SettingsAction::Category(category), false);
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
        for (style, name) in [(TabBarStyle::Separate, "Separate"), (TabBarStyle::Compact, "Compact")] {
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
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
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
        let row_node = ui.add(
            parent,
            Widget::container(),
            Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row(10.0) },
        );
        ui.add_label(row_node, label, TextStyle::Body, grow_label());
        row_node
    }

    fn control(&mut self, ui: &mut Ui, config: &Config, setting: Setting) {
        let row_node = self.row(ui, setting.label());
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
                    size: Size { width: length(VALUE_WIDTH), height: auto() },
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
                self.confirm_clear = false;
                Outcome::Nothing
            }
            SettingsAction::Theme(mode) => {
                config.appearance.theme = mode;
                Outcome::Changed
            }
            SettingsAction::SearchEngine(i) => {
                config.general.search_url = SEARCH_ENGINES[i].1.to_owned();
                Outcome::Changed
            }
            SettingsAction::Accent(i) => {
                config.appearance.accent = Color::rgb(ACCENTS[i]);
                Outcome::Changed
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
                let password = self.password.and_then(|f| ui.field_text(f)).unwrap_or_default().to_owned();
                if password.is_empty() {
                    return Outcome::Nothing;
                }
                self.has_password = true;
                self.show(ui, config, keymap);
                return Outcome::SetPassword(password);
            }
            SettingsAction::RemovePassword => {
                self.has_password = false;
                self.show(ui, config, keymap);
                return Outcome::RemovePassword;
            }
            SettingsAction::OpenPasswords => return Outcome::OpenPasswords,
            SettingsAction::ImportPasswords | SettingsAction::ExportPasswords => {
                let text =
                    self.passwords_file.and_then(|f| ui.field_text(f)).unwrap_or_default().trim().to_owned();
                if text.is_empty() {
                    self.passwords_status = Some("Enter the file's path first.".to_owned());
                    self.show(ui, config, keymap);
                    return Outcome::Nothing;
                }
                let path = expand_home(&text);
                return if action == SettingsAction::ImportPasswords {
                    Outcome::ImportPasswords(path)
                } else {
                    Outcome::ExportPasswords(path)
                };
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

/// "~/x" → the home directory's "x".
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// A small capsule button (accent tint, or red for destructive ones).
fn pill_button(ui: &mut Ui, parent: NodeId, text: &str, action: SettingsAction, destructive: bool) {
    let color = if destructive { Paint::Fixed(Color::rgb(0xD0362B)) } else { Paint::Accent };
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action.id())
            .with_radius(CAPSULE)
            .with_background(Background::Fill(color, 0.14)),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(16.0, 0.0), ..row(0.0) },
    );
    let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(color);
    }
}

/// A segmented control: options as capsules, the current one filled.
pub fn segmented(ui: &mut Ui, parent: NodeId, options: &[(&str, ActionId)], current: Option<usize>) {
    let group = ui.add(
        parent,
        Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(3.0, 3.0), ..row(2.0) },
    );
    for (i, (label, action)) in options.iter().enumerate() {
        let selected = Some(i) == current;
        let segment = ui.add(
            group,
            Widget::container().with_action(*action).with_radius(CAPSULE).with_background(if selected {
                Background::Fill(Paint::Surface, 1.0)
            } else {
                Background::None
            }),
            Style {
                size: Size { width: auto(), height: length(30.0) },
                padding: padding(14.0, 0.0),
                ..row(0.0)
            },
        );
        let style = if selected { TextStyle::BodyEmphasis } else { TextStyle::Body };
        ui.add_label(segment, label, style, Style::default());
    }
}

fn shortcut_actions() -> Vec<KeyAction> {
    use KeyAction::*;
    vec![
        FocusAddressBar,
        NewTab,
        CloseTab,
        ReopenClosedTab,
        NextTab,
        PreviousTab,
        Reload,
        HardReload,
        Back,
        Forward,
        ToggleSidebar,
        TabOverview,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        OpenSettings,
        ToggleFullscreen,
        Quit,
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

/// "focus_address_bar" → "Focus address bar".
fn humanize(name: &str) -> String {
    let text = name.replace('_', " ");
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
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
        for setting in
            [Setting::TextScale, Setting::Refraction, Setting::Blur, Setting::Squash, Setting::MergeDistance]
        {
            setting.set(&mut config, 0.5);
            // Values are rounded to sensible steps (text size to 0.05), so allow one step.
            assert!((setting.get(&config) - 0.5).abs() < 0.12, "{setting:?}: {}", setting.get(&config));
        }
        Setting::ReduceMotion.set(&mut config, 1.0);
        assert!(config.glass.reduce_motion);
    }

    #[test]
    fn action_ids_round_trip() {
        for action in [
            SettingsAction::Close,
            SettingsAction::Category(Category::Privacy),
            SettingsAction::Theme(ThemeMode::Dark),
            SettingsAction::SearchEngine(4),
            SettingsAction::Accent(5),
            SettingsAction::ClearHistory,
            SettingsAction::ShieldLevel(ShieldLevel::Aggressive),
            SettingsAction::UpdateFilterLists,
            SettingsAction::AddTrustedSite,
            SettingsAction::RemoveTrustedSite(7),
            SettingsAction::Category(Category::Passwords),
            SettingsAction::ExportPasswords,
        ] {
            assert_eq!(SettingsAction::from_id(action.id()), Some(action));
        }
        assert_eq!(humanize("focus_address_bar"), "Focus address bar");
    }
}
