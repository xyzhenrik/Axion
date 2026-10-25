//! The settings sheet (design: "Settings"): a large glass sheet over the dimmed browser with
//! categories on the left. Every change is written to `config.toml` at once.

use axion_compositor::Icon;
use axion_config::{Action as KeyAction, Color, Config, Keymap, ThemeMode};
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
    Keyboard,
    Privacy,
}

impl Category {
    const ALL: [Category; 5] = [
        Category::General,
        Category::Appearance,
        Category::LiquidGlass,
        Category::Keyboard,
        Category::Privacy,
    ];

    fn title(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Appearance => "Appearance",
            Category::LiquidGlass => "Liquid Glass",
            Category::Keyboard => "Keyboard",
            Category::Privacy => "Privacy",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Category::General => Icon::Settings,
            Category::Appearance => Icon::Sidebar,
            Category::LiquidGlass => Icon::Overview,
            Category::Keyboard => Icon::Menu,
            Category::Privacy => Icon::Shield,
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
        }
    }

    fn is_toggle(self) -> bool {
        matches!(
            self,
            Setting::TintFromPage
                | Setting::AdaptToPage
                | Setting::ReduceTransparency
                | Setting::ReduceMotion
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
}

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
                },
        )
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        let n = id.0.checked_sub(ACTION_BASE)?;
        Some(match n {
            0 => SettingsAction::Close,
            10..=14 => SettingsAction::Category(Category::ALL[(n - 10) as usize]),
            20 => SettingsAction::Theme(ThemeMode::Auto),
            21 => SettingsAction::Theme(ThemeMode::Light),
            22 => SettingsAction::Theme(ThemeMode::Dark),
            30..=39 if ((n - 30) as usize) < SEARCH_ENGINES.len() => {
                SettingsAction::SearchEngine((n - 30) as usize)
            }
            40..=49 if ((n - 40) as usize) < ACCENTS.len() => SettingsAction::Accent((n - 40) as usize),
            50 => SettingsAction::ClearHistory,
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
}

pub struct SettingsSheet {
    root: NodeId,
    sheet: NodeId,
    nav: Vec<(Category, NodeId)>,
    content: NodeId,
    category: Category,
    controls: Vec<(NodeId, Setting)>,
    homepage: Option<NodeId>,
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

    pub fn homepage_field(&self) -> Option<NodeId> {
        self.homepage
    }

    pub fn open(&mut self, ui: &mut Ui, config: &Config, keymap: &Keymap, window: Vec2) {
        self.open = true;
        ui.set_visible(self.root, true);
        self.resize(ui, window);
        self.show(ui, config, keymap);
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
                let options: Vec<(&str, SettingsAction)> = SEARCH_ENGINES
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| (*name, SettingsAction::SearchEngine(i)))
                    .collect();
                segmented(ui, row_node, &options, current);
            }
            Category::Appearance => {
                let row_node = self.row(ui, "Appearance");
                let themes = [
                    ("Automatic", SettingsAction::Theme(ThemeMode::Auto)),
                    ("Light", SettingsAction::Theme(ThemeMode::Light)),
                    ("Dark", SettingsAction::Theme(ThemeMode::Dark)),
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
            Category::Privacy => {
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

/// A segmented control: options as capsules, the current one filled.
fn segmented(ui: &mut Ui, parent: NodeId, options: &[(&str, SettingsAction)], current: Option<usize>) {
    let group = ui.add(
        parent,
        Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
        Style { size: Size { width: auto(), height: length(36.0) }, padding: padding(3.0, 3.0), ..row(2.0) },
    );
    for (i, (label, action)) in options.iter().enumerate() {
        let selected = Some(i) == current;
        let segment =
            ui.add(
                group,
                Widget::container().with_action(action.id()).with_radius(CAPSULE).with_background(
                    if selected { Background::Fill(Paint::Surface, 1.0) } else { Background::None },
                ),
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
    ]
}

/// "focus_address_bar" → "Focus address bar".
fn humanize(name: &str) -> String {
    let text = name.replace('_', " ");
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
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
            assert!((setting.get(&config) - 0.5).abs() < 0.05, "{setting:?}");
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
        ] {
            assert_eq!(SettingsAction::from_id(action.id()), Some(action));
        }
        assert_eq!(humanize("focus_address_bar"), "Focus address bar");
    }
}
