//! The settings sheet (design: "Settings"): a large glass sheet over the dimmed browser with
//! categories on the left. Every change is written to `config.toml` at once.

use axion_compositor::Icon;
use axion_config::{Action as KeyAction, Color, Config, Keymap, ShieldLevel, ThemeMode};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

const ACTION_BASE: u32 = 8000;
const SHEET_MAX: Vec2 = Vec2::new(920.0, 660.0);
const SHEET_MARGIN: f32 = 32.0;
const SHEET_RADIUS: f32 = 30.0;
const NAV_WIDTH: f32 = 230.0;
const ROW_HEIGHT: f32 = 52.0;
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
}

impl Category {
    const ALL: [Category; 8] = [
        Category::General,
        Category::Appearance,
        Category::LiquidGlass,
        Category::Tabs,
        Category::Media,
        Category::Keyboard,
        Category::Shields,
        Category::Privacy,
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
    OwnControls,
    ReplaceSitePlayers,
    OwnFullscreenControls,
    HideControlsAfter,
    HardwareDecoding,
    DesktopIntegration,
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Setting::TintFromPage => "Window takes the page's colour",
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
            Setting::LockAfter => "Lock after (0 – 10 minutes in background)",
            Setting::HideAdElements => "Hide empty ad spaces",
            Setting::BlockPopups => "Block popups opened without a click",
            Setting::BlockRedirects => "Block redirects to ad and tracker sites",
            Setting::StripTracking => "Remove tracking parameters from links",
            Setting::ThirdPartyCookies => "Block third-party cookies",
            Setting::FreezeAfter => "Freeze background tabs after (never – 60 minutes)",
            Setting::DiscardAfter => "Unload background tabs after (never – 8 hours)",
            Setting::LimitMemory => "Unload tabs when memory runs over the budget",
            Setting::MemoryBudget => "Memory budget (automatic – all memory)",
            Setting::KeepMediaAwake => "Keep tabs playing audio awake",
            Setting::RestoreSession => "Reopen last session's tabs at start",
            Setting::OwnControls => "Axion's controls for page videos",
            Setting::ReplaceSitePlayers => "Also instead of sites' own players (not YouTube, Twitch…)",
            Setting::OwnFullscreenControls => "Axion's controls in fullscreen, also over site players",
            Setting::HideControlsAfter => "Hide controls while playing after (1 – 10 seconds)",
            Setting::HardwareDecoding => "Hardware video decoding (after restart)",
            Setting::DesktopIntegration => "Desktop media controls and media keys (after restart)",
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
                | Setting::OwnControls
                | Setting::ReplaceSitePlayers
                | Setting::OwnFullscreenControls
                | Setting::HardwareDecoding
                | Setting::DesktopIntegration
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
            Setting::MemoryBudget => (0.0, system_memory_mb()),
            _ => (0.0, 1.0),
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
            Setting::OwnControls => return c.media.own_controls as u8 as f32,
            Setting::ReplaceSitePlayers => return c.media.replace_site_players as u8 as f32,
            Setting::OwnFullscreenControls => return c.media.own_fullscreen_controls as u8 as f32,
            Setting::HardwareDecoding => return c.media.hardware_decoding as u8 as f32,
            Setting::DesktopIntegration => return c.media.desktop_integration as u8 as f32,
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
            Setting::OwnControls => c.media.own_controls = on,
            Setting::ReplaceSitePlayers => c.media.replace_site_players = on,
            Setting::OwnFullscreenControls => c.media.own_fullscreen_controls = on,
            Setting::HardwareDecoding => c.media.hardware_decoding = on,
            Setting::DesktopIntegration => c.media.desktop_integration = on,
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
}

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
                },
        )
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        let n = id.0.checked_sub(ACTION_BASE)?;
        Some(match n {
            0 => SettingsAction::Close,
            10..=17 => SettingsAction::Category(Category::ALL[(n - 10) as usize]),
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
}

pub struct SettingsSheet {
    root: NodeId,
    sheet: NodeId,
    nav: Vec<(Category, NodeId)>,
    content: NodeId,
    category: Category,
    controls: Vec<(NodeId, Setting)>,
    homepage: Option<NodeId>,
    password: Option<NodeId>,
    has_password: bool,
    /// The browser's memory use, shown on the Tabs page.
    memory_used: Option<u64>,
    confirm_clear: bool,
    open: bool,
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
        let content = ui.add(
            sheet,
            Widget::container(),
            Style {
                flex_grow: 1.0,
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
            category: Category::General,
            controls: Vec::new(),
            homepage: None,
            password: None,
            has_password: false,
            memory_used: None,
            confirm_clear: false,
            open: false,
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

    pub fn homepage_field(&self) -> Option<NodeId> {
        self.homepage
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

    /// Shows this category next time the sheet opens (or now, if open).
    pub fn set_category(&mut self, category: Category) {
        self.category = category;
    }

    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
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
                w.background = if category == self.category {
                    Background::Fill(Paint::Accent, 0.14)
                } else {
                    Background::None
                };
            }
        }
        ui.clear_children(self.content);
        self.controls.clear();
        self.homepage = None;
        self.password = None;
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
                for setting in
                    [Setting::TintFromPage, Setting::AdaptToPage, Setting::TextScale, Setting::PageCorner]
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
                for setting in [
                    Setting::OwnControls,
                    Setting::ReplaceSitePlayers,
                    Setting::OwnFullscreenControls,
                    Setting::HideControlsAfter,
                    Setting::HardwareDecoding,
                    Setting::DesktopIntegration,
                ] {
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
        }
    }

    fn row(&self, ui: &mut Ui, label: &str) -> NodeId {
        self.row_with_height(ui, label, ROW_HEIGHT)
    }

    fn row_with_height(&self, ui: &mut Ui, label: &str, height: f32) -> NodeId {
        let row_node = ui.add(
            self.content,
            Widget::container(),
            Style { size: Size { width: auto(), height: length(height) }, ..row(10.0) },
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
            ui.add(
                row_node,
                Widget::slider(value),
                Style { size: Size { width: length(260.0), height: auto() }, ..Default::default() },
            )
        };
        self.controls.push((node, setting));
    }

    /// A toggle or slider moved.
    pub fn value_changed(&mut self, node: NodeId, value: f32, config: &mut Config) -> Outcome {
        match self.controls.iter().find(|(n, _)| *n == node) {
            Some(&(_, setting)) => {
                setting.set(config, value);
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

fn system_memory_mb() -> f32 {
    axion_tabs::system_memory().map_or(16384.0, |b| (b / (1024 * 1024)) as f32)
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
        ] {
            assert_eq!(SettingsAction::from_id(action.id()), Some(action));
        }
        assert_eq!(humanize("focus_address_bar"), "Focus address bar");
    }
}
