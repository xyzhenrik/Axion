//! The browser chrome from the design ("Main window", "Responsive variants"): a floating glass
//! tab bar with the address field, and the sidebar, docked in wide windows and floating over
//! the page otherwise.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config, SidebarMode};
use axion_ui::taffy::prelude::*;
use axion_ui::{
    ActionId, Align, Background, CAPSULE, GlassLevel, Paint, Shadow, TextStyle, Ui, UiEvent, Widget,
};
use glam::Vec2;

use crate::favicons::{Favicons, favicon_node};
use crate::tabs::Tabs;
use crate::url::display_host;

/// What a click in the chrome means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    ToggleSidebar,
    Back,
    Forward,
    ReloadOrStop,
    Shield,
    EditAddress,
    NewTab,
    Overview,
    SelectTab(usize),
    CloseTab(usize),
}

const SELECT_TAB_BASE: u32 = 1000;
const CLOSE_TAB_BASE: u32 = 2000;
/// Feedback fill behind the active sidebar row (design: white at 60%).
const ACTIVE_ROW_OPACITY: f32 = 0.6;

impl ChromeAction {
    fn id(self) -> ActionId {
        ActionId(match self {
            ChromeAction::ToggleSidebar => 1,
            ChromeAction::Back => 2,
            ChromeAction::Forward => 3,
            ChromeAction::ReloadOrStop => 4,
            ChromeAction::Shield => 5,
            ChromeAction::EditAddress => 6,
            ChromeAction::NewTab => 7,
            ChromeAction::Overview => 8,
            ChromeAction::SelectTab(i) => SELECT_TAB_BASE + i as u32,
            ChromeAction::CloseTab(i) => CLOSE_TAB_BASE + i as u32,
        })
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        Some(match id.0 {
            1 => ChromeAction::ToggleSidebar,
            2 => ChromeAction::Back,
            3 => ChromeAction::Forward,
            4 => ChromeAction::ReloadOrStop,
            5 => ChromeAction::Shield,
            6 => ChromeAction::EditAddress,
            7 => ChromeAction::NewTab,
            8 => ChromeAction::Overview,
            n if (CLOSE_TAB_BASE..CLOSE_TAB_BASE + 1000).contains(&n) => {
                ChromeAction::CloseTab((n - CLOSE_TAB_BASE) as usize)
            }
            n if (SELECT_TAB_BASE..CLOSE_TAB_BASE).contains(&n) => {
                ChromeAction::SelectTab((n - SELECT_TAB_BASE) as usize)
            }
            _ => return None,
        })
    }
}

/// The three layouts from the design's responsive boards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Sidebar docked beside the page.
    Wide,
    /// Sidebar floats over the page.
    Regular,
    /// Tabs collapse into one compact capsule.
    Narrow,
}

pub struct Chrome {
    /// Whether the sidebar shows while docked (wide windows); remembered across resizes.
    docked_open: bool,
    /// Whether the floating sidebar is shown (narrower windows); closes when the page is used.
    overlay_open: bool,
    docked: bool,
    mode: Mode,
    page_fullscreen: bool,
    sidebar: NodeId,
    sidebar_list: NodeId,
    bar: NodeId,
    sidebar_button: NodeId,
    back: NodeId,
    forward: NodeId,
    address_label: NodeId,
    pub address_field: NodeId,
    address: NodeId,
    reload_icon: NodeId,
    progress: NodeId,
    pills: NodeId,
    separator: NodeId,
    tab_count: NodeId,
    tab_count_label: NodeId,
    overview: NodeId,
}

fn square(side: f32) -> Size<Dimension> {
    Size { width: length(side), height: length(side) }
}

fn center_row(gap: f32) -> Style {
    Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        justify_content: JustifyContent::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// A circular icon button (44 px in the bar, 34 in the address field). Returns the button
/// and its glyph.
fn icon_button(
    ui: &mut Ui,
    parent: NodeId,
    action: ChromeAction,
    glyph: Widget,
    side: f32,
) -> (NodeId, NodeId) {
    let button = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE),
        Style { size: square(side), ..center_row(0.0) },
    );
    let glyph = ui.add(button, glyph, Style::default());
    (button, glyph)
}

/// A favicon stand-in: a rounded square in a colour derived from the host (as in the design).
pub fn favicon_color(url: &str) -> Color {
    const COLORS: [u32; 8] = [0xF2742B, 0x2E6BFF, 0x7C5CFF, 0x1F9D57, 0xE2457A, 0x1C5FA6, 0xD98A06, 0x8E3FA6];
    let host = display_host(url);
    let hash = host.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    Color::rgb(COLORS[hash as usize % COLORS.len()])
}

impl Chrome {
    pub fn build(ui: &mut Ui, config: &Config) -> Self {
        let root = ui.root();
        let radii = &config.radii;
        let layout = &config.layout;

        // Sidebar first, so the bar draws above it where they meet.
        let sidebar = ui.add(
            root,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Regular))
                .with_radius(radii.panel),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(12.0),
                    right: length(12.0),
                    top: length(14.0),
                    bottom: length(14.0),
                },
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );
        ui.add_label(
            sidebar,
            "Tabs",
            TextStyle::Caption,
            Style {
                padding: Rect {
                    left: length(12.0),
                    right: length(12.0),
                    top: length(4.0),
                    bottom: length(6.0),
                },
                ..Default::default()
            },
        );
        let sidebar_list = ui.add(
            sidebar,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );

        let bar = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(layout.bar_height) },
                padding: Rect {
                    left: length(7.0),
                    right: length(7.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..center_row(2.0)
            },
        );
        let (sidebar_button, _) =
            icon_button(ui, bar, ChromeAction::ToggleSidebar, Widget::icon(Icon::Sidebar, 21.0), 44.0);
        let (back, _) = icon_button(ui, bar, ChromeAction::Back, Widget::icon(Icon::Back, 21.0), 44.0);
        let (forward, _) =
            icon_button(ui, bar, ChromeAction::Forward, Widget::icon(Icon::Forward, 21.0), 44.0);

        // The active tab, expanded into the address field.
        let address = ui.add(
            bar,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, 0.88))
                .with_radius(CAPSULE)
                .with_shadow(Shadow { offset_y: 2.0, blur: 10.0, opacity: 0.10 })
                .with_action(ChromeAction::EditAddress.id()),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(120.0), height: auto() },
                size: Size { width: auto(), height: length(44.0) },
                margin: Rect { left: length(6.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                padding: Rect {
                    left: length(8.0),
                    right: length(6.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..center_row(8.0)
            },
        );
        let shield_glyph = Widget::icon(Icon::Shield, 19.0).with_paint(Paint::Shield);
        icon_button(ui, address, ChromeAction::Shield, shield_glyph, 34.0);
        let address_label = ui.add_label(
            address,
            "",
            TextStyle::BodyEmphasis,
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        ui.set_label_align(address_label, Align::Center);
        let address_field = ui.add_text_field(
            address,
            "Search or enter website name",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        ui.set_visible(address_field, false);
        let (_, reload_icon) =
            icon_button(ui, address, ChromeAction::ReloadOrStop, Widget::icon(Icon::Reload, 18.0), 34.0);
        let progress = ui.add(
            address,
            Widget::progress(Paint::Accent),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(22.0), right: length(22.0), top: auto(), bottom: length(1.0) },
                size: Size { width: auto(), height: length(2.5) },
                ..Default::default()
            },
        );

        let pills = ui.add(
            bar,
            Widget::container(),
            Style {
                flex_shrink: 1.0,
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Visible,
                },
                min_size: Size { width: length(0.0), height: auto() },
                ..center_row(2.0)
            },
        );
        let separator = ui.add(
            bar,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.14)),
            Style {
                size: Size { width: length(1.0), height: length(24.0) },
                margin: Rect { left: length(4.0), right: length(4.0), top: length(0.0), bottom: length(0.0) },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        icon_button(ui, bar, ChromeAction::NewTab, Widget::icon(Icon::Plus, 21.0), 44.0);
        // Narrow layout: tab overview as a box with the tab count.
        let tab_count = ui.add(
            bar,
            Widget::container().with_action(ChromeAction::Overview.id()).with_radius(CAPSULE),
            Style { size: square(44.0), ..center_row(0.0) },
        );
        let count_box = ui.add(
            tab_count,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.10)).with_radius(7.0),
            Style { size: square(22.0), ..center_row(0.0) },
        );
        let tab_count_label = ui.add_label(count_box, "1", TextStyle::Caption, Style::default());
        let (overview, _) =
            icon_button(ui, bar, ChromeAction::Overview, Widget::icon(Icon::Overview, 20.0), 44.0);

        let mut chrome = Self {
            docked_open: layout.sidebar_visible,
            overlay_open: false,
            docked: true,
            mode: Mode::Wide,
            page_fullscreen: false,
            sidebar,
            sidebar_list,
            bar,
            sidebar_button,
            back,
            forward,
            address_label,
            address_field,
            address,
            reload_icon,
            progress,
            pills,
            separator,
            tab_count,
            tab_count_label,
            overview,
        };
        chrome.apply_layout(ui, config, Vec2::new(1440.0, 900.0));
        chrome
    }

    fn sidebar_docked(&self, config: &Config) -> bool {
        match config.layout.sidebar {
            SidebarMode::Docked => self.mode != Mode::Narrow,
            SidebarMode::Overlay => false,
            SidebarMode::Auto => self.mode == Mode::Wide,
        }
    }

    pub fn sidebar_open(&self) -> bool {
        if self.docked { self.docked_open } else { self.overlay_open }
    }

    pub fn toggle_sidebar(&mut self) {
        if self.docked {
            self.docked_open = !self.docked_open;
        } else {
            self.overlay_open = !self.overlay_open;
        }
    }

    /// Closes the floating sidebar (e.g. when the user goes back to the page). Returns whether
    /// anything changed.
    pub fn dismiss_overlay(&mut self) -> bool {
        let was_open = !self.docked && self.overlay_open;
        self.overlay_open = false;
        was_open
    }

    /// Positions the bar and sidebar for the window size (logical px).
    pub fn apply_layout(&mut self, ui: &mut Ui, config: &Config, window: Vec2) {
        let l = &config.layout;
        self.mode = if window.x >= l.wide_from {
            Mode::Wide
        } else if window.x >= l.narrow_below {
            Mode::Regular
        } else {
            Mode::Narrow
        };
        let docked = self.sidebar_docked(config);
        self.docked = docked;
        tracing::debug!(?window, mode = ?self.mode, docked, docked_open = self.docked_open, overlay_open = self.overlay_open, "chrome layout");
        let show_sidebar = self.sidebar_open() && !self.page_fullscreen;
        let m = l.margin;

        let bar_left = if docked && show_sidebar { m + l.sidebar_width + m } else { m };
        let mut bar_style = ui.style(self.bar);
        bar_style.inset = Rect { left: length(bar_left), right: length(m), top: length(m), bottom: auto() };
        bar_style.display = if self.page_fullscreen { Display::None } else { Display::Flex };
        ui.set_style(self.bar, bar_style);

        let mut side = ui.style(self.sidebar);
        side.size = Size { width: length(l.sidebar_width), height: auto() };
        let top = if docked { m } else { m + l.bar_height + 12.0 };
        side.inset = Rect { left: length(m), right: auto(), top: length(top), bottom: length(m) };
        side.display = if show_sidebar { Display::Flex } else { Display::None };
        ui.set_style(self.sidebar, side);
        if let Some(w) = ui.widget_mut(self.sidebar) {
            // Docked, it's a panel beside the page; floating, it needs more separation.
            w.background = Background::Glass(if docked { GlassLevel::Regular } else { GlassLevel::Thick });
        }

        let narrow = self.mode == Mode::Narrow;
        ui.set_visible(self.forward, !narrow);
        ui.set_visible(self.pills, !narrow);
        ui.set_visible(self.separator, !narrow);
        ui.set_visible(self.overview, !narrow);
        ui.set_visible(self.tab_count, narrow);
        if let Some(w) = ui.widget_mut(self.sidebar_button) {
            w.selected = self.sidebar_open();
        }
    }

    /// The page's rectangle (logical px): below the bar, beside a docked sidebar, with a small
    /// margin to the window edges so its rounded corners read as a card.
    pub fn page_rect(&self, config: &Config, window: Vec2) -> Frame {
        if self.page_fullscreen {
            return Frame { origin: Vec2::ZERO, size: window };
        }
        let l = &config.layout;
        let left = if self.sidebar_docked(config) && self.docked_open {
            l.margin + l.sidebar_width + l.margin / 2.0
        } else {
            l.page_margin
        };
        let top = l.margin + l.bar_height + l.margin / 2.0;
        let origin = Vec2::new(left, top);
        let size = window - origin - Vec2::splat(l.page_margin);
        Frame { origin, size: size.max(Vec2::ONE) }
    }

    /// The address field's frame (logical px), for attaching the suggestion list.
    pub fn address_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.address)
    }

    pub fn page_fullscreen(&self) -> bool {
        self.page_fullscreen
    }

    pub fn set_page_fullscreen(&mut self, fullscreen: bool) {
        self.page_fullscreen = fullscreen;
    }

    /// Rebuilds the tab lists (sidebar rows, bar pills) from the tab model.
    pub fn sync_tabs(&mut self, ui: &mut Ui, tabs: &Tabs, favicons: &Favicons) {
        ui.clear_children(self.sidebar_list);
        ui.clear_children(self.pills);
        let tab_width = ui.theme().config().layout.tab_width;
        for (i, tab) in tabs.all().iter().enumerate() {
            let active = i == tabs.active_index();

            // Sidebar row.
            let row_bg =
                if active { Background::Fill(Paint::Surface, ACTIVE_ROW_OPACITY) } else { Background::None };
            let row = ui.add(
                self.sidebar_list,
                Widget::container()
                    .with_action(ChromeAction::SelectTab(i).id())
                    .with_radius(12.0)
                    .with_background(row_bg),
                Style {
                    size: Size { width: auto(), height: length(40.0) },
                    padding: Rect {
                        left: length(12.0),
                        right: length(4.0),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    justify_content: JustifyContent::START,
                    ..center_row(12.0)
                },
            );
            favicon_node(ui, row, &tab.url, 18.0, favicons);
            ui.add_label(
                row,
                &tab.label(),
                if active { TextStyle::BodyEmphasis } else { TextStyle::Body },
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
            let close_glyph = Widget::icon(Icon::Close, 14.0)
                .with_icon_weight(IconWeight::Bold)
                .with_paint(Paint::Secondary);
            icon_button(ui, row, ChromeAction::CloseTab(i), close_glyph, 32.0);

            // Bar pill for the other tabs (the active one is the address field).
            if !active {
                let pill = ui.add(
                    self.pills,
                    Widget::container().with_action(ChromeAction::SelectTab(i).id()).with_radius(CAPSULE),
                    Style {
                        size: Size { width: length(tab_width), height: length(44.0) },
                        flex_shrink: 1.0,
                        min_size: Size { width: length(60.0), height: auto() },
                        padding: Rect {
                            left: length(14.0),
                            right: length(14.0),
                            top: length(0.0),
                            bottom: length(0.0),
                        },
                        justify_content: JustifyContent::START,
                        ..center_row(9.0)
                    },
                );
                favicon_node(ui, pill, &tab.url, 16.0, favicons);
                let label = ui.add_label(
                    pill,
                    &tab.label(),
                    TextStyle::Callout,
                    Style {
                        flex_grow: 1.0,
                        flex_shrink: 1.0,
                        min_size: Size { width: length(0.0), height: auto() },
                        ..Default::default()
                    },
                );
                if let Some(w) = ui.widget_mut(label) {
                    w.set_paint(Paint::Secondary);
                }
            }
        }
        ui.set_text(self.tab_count_label, &tabs.len().to_string());
        self.sync_active(ui, tabs);
    }

    /// Updates the bar for the active tab (address, back/forward, reload/stop, progress).
    pub fn sync_active(&mut self, ui: &mut Ui, tabs: &Tabs) {
        let Some(tab) = tabs.active() else { return };
        ui.set_text(self.address_label, &display_host(&tab.url));
        if ui.focused() != Some(self.address_field) {
            ui.set_field_text(self.address_field, &tab.url);
        }
        if let Some(w) = ui.widget_mut(self.back) {
            w.enabled = tab.can_go_back;
        }
        if let Some(w) = ui.widget_mut(self.forward) {
            w.enabled = tab.can_go_forward;
        }
        if let Some(w) = ui.widget_mut(self.reload_icon) {
            w.set_icon(if tab.loading { Icon::Stop } else { Icon::Reload });
        }
        ui.set_progress(self.progress, if tab.loading { tab.progress.max(0.08) as f32 } else { 1.0 });
    }

    /// Switches the address field between showing the host and editing the full URL.
    pub fn set_editing(&mut self, ui: &mut Ui, editing: bool) -> Vec<UiEvent> {
        ui.set_visible(self.address_label, !editing);
        ui.set_visible(self.address_field, editing);
        if editing { ui.focus(self.address_field) } else { ui.blur() }
    }
}
