//! The browser chrome, modelled on iPadOS 26 Safari:
//!
//! - a toolbar of three glass groups: navigation (sidebar toggle when the sidebar is closed,
//!   back, forward), one capsule holding all tabs as equal segments — the active tab is the
//!   light pill with close, Shields, address, reload and page menu — and actions (share, new
//!   tab, tab overview);
//! - a full-height sidebar: header buttons, the tab list, Private, and a "Saved" section with
//!   bookmarks, reading list and history. Docked in wide windows, floating otherwise.

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
    PageMenu,
    Share,
    NewTab,
    Overview,
    NewTabGroup,
    ToggleTabsSection,
    ToggleSavedSection,
    Private,
    Bookmarks,
    ReadingList,
    History,
    ClosePanel,
    SelectTab(usize),
    CloseTab(usize),
    /// An entry in the open bookmarks/history panel.
    OpenPanelItem(usize),
}

const SELECT_TAB_BASE: u32 = 1000;
const CLOSE_TAB_BASE: u32 = 2000;
const PANEL_ITEM_BASE: u32 = 7000;

impl ChromeAction {
    const SIMPLE: [ChromeAction; 18] = [
        ChromeAction::ToggleSidebar,
        ChromeAction::Back,
        ChromeAction::Forward,
        ChromeAction::ReloadOrStop,
        ChromeAction::Shield,
        ChromeAction::EditAddress,
        ChromeAction::PageMenu,
        ChromeAction::Share,
        ChromeAction::NewTab,
        ChromeAction::Overview,
        ChromeAction::NewTabGroup,
        ChromeAction::ToggleTabsSection,
        ChromeAction::ToggleSavedSection,
        ChromeAction::Private,
        ChromeAction::Bookmarks,
        ChromeAction::ReadingList,
        ChromeAction::History,
        ChromeAction::ClosePanel,
    ];

    fn id(self) -> ActionId {
        ActionId(match self {
            ChromeAction::SelectTab(i) => SELECT_TAB_BASE + i as u32,
            ChromeAction::CloseTab(i) => CLOSE_TAB_BASE + i as u32,
            ChromeAction::OpenPanelItem(i) => PANEL_ITEM_BASE + i as u32,
            simple => 1 + Self::SIMPLE.iter().position(|&a| a == simple).unwrap_or(0) as u32,
        })
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        match id.0 {
            n @ 1..=18 => Some(Self::SIMPLE[n as usize - 1]),
            n if (PANEL_ITEM_BASE..PANEL_ITEM_BASE + 1000).contains(&n) => {
                Some(ChromeAction::OpenPanelItem((n - PANEL_ITEM_BASE) as usize))
            }
            n if (SELECT_TAB_BASE..CLOSE_TAB_BASE).contains(&n) => {
                Some(ChromeAction::SelectTab((n - SELECT_TAB_BASE) as usize))
            }
            n if (CLOSE_TAB_BASE..CLOSE_TAB_BASE + 1000).contains(&n) => {
                Some(ChromeAction::CloseTab((n - CLOSE_TAB_BASE) as usize))
            }
            _ => None,
        }
    }
}

/// One row of a sidebar panel.
pub struct PanelItem {
    pub title: String,
    pub url: String,
    /// Second line (host, or when it was visited).
    pub detail: String,
}

/// The three layouts from the design's responsive boards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Sidebar docked beside the page.
    Wide,
    /// Sidebar floats over the page.
    Regular,
    /// Only the active tab is shown in the toolbar.
    Narrow,
}

/// Gap between the toolbar's glass groups (wide enough that they don't flow together at rest).
const GROUP_GAP: f32 = 14.0;
const BUTTON: f32 = 44.0;
const SIDEBAR_BUTTON: f32 = 48.0;
const SEGMENT_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 42.0;
/// The active tab's segment grows this much more than the others and never gets narrower
/// than its minimum, so the address stays readable with many tabs.
const ACTIVE_SEGMENT_GROW: f32 = 1.5;
const ACTIVE_SEGMENT_MIN: f32 = 300.0;
const PANEL_ROW_HEIGHT: f32 = 52.0;
/// Indentation of tab rows under "N Tabs".
const TAB_INDENT: f32 = 18.0;

pub struct Chrome {
    mode: Mode,
    docked: bool,
    /// Sidebar shown while docked (remembered across resizes).
    docked_open: bool,
    /// Floating sidebar shown (narrower windows); closes when the page is used.
    overlay_open: bool,
    page_fullscreen: bool,
    tabs_section_open: bool,
    saved_section_open: bool,

    sidebar: NodeId,
    sections: NodeId,
    panel: NodeId,
    panel_title: NodeId,
    panel_list: NodeId,
    tab_group_label: NodeId,
    tab_group_chevron: NodeId,
    sidebar_tabs: NodeId,
    saved_chevron: NodeId,
    saved_list: NodeId,

    bar: NodeId,
    bar_sidebar_button: NodeId,
    back: NodeId,
    forward: NodeId,
    tab_capsule: NodeId,
    /// The active tab's segment; persistent so typing survives tab-list rebuilds.
    address: NodeId,
    address_close: NodeId,
    address_label: NodeId,
    pub address_field: NodeId,
    reload_icon: NodeId,
    page_menu: NodeId,
    progress: NodeId,
    /// Inactive tab segments, rebuilt by `sync_tabs`.
    pills: Vec<NodeId>,
    overview: NodeId,
    tab_count: NodeId,
    tab_count_label: NodeId,
}

fn square(side: f32) -> Size<Dimension> {
    Size { width: length(side), height: length(side) }
}

fn row(gap: f32) -> Style {
    Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

fn column(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        gap: Size { width: length(0.0), height: length(gap) },
        ..Default::default()
    }
}

fn padding(left: f32, right: f32, top: f32, bottom: f32) -> Rect<LengthPercentage> {
    Rect { left: length(left), right: length(right), top: length(top), bottom: length(bottom) }
}

fn shrinking_label() -> Style {
    Style {
        flex_grow: 1.0,
        flex_shrink: 1.0,
        min_size: Size { width: length(0.0), height: auto() },
        ..Default::default()
    }
}

/// A round icon button. Returns the button and its glyph.
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
        Style { size: square(side), justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let glyph = ui.add(button, glyph, Style::default());
    (button, glyph)
}

/// Safari's ⓧ: a small filled circle with a cross, closing a tab.
fn close_dot(ui: &mut Ui, parent: NodeId, action: ChromeAction, hit: f32) -> NodeId {
    let button = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE),
        Style { size: square(hit), justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let dot = ui.add(
        button,
        Widget::container().with_background(Background::Fill(Paint::Label, 0.85)).with_radius(CAPSULE),
        Style { size: square(20.0), justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let cross =
        Widget::icon(Icon::Close, 11.0).with_icon_weight(IconWeight::Heavy).with_paint(Paint::Surface);
    ui.add(dot, cross, Style::default());
    button
}

/// A glass capsule grouping toolbar buttons.
fn glass_group(ui: &mut Ui, parent: NodeId) -> NodeId {
    ui.add(
        parent,
        Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
        Style {
            size: Size { width: auto(), height: length(56.0) },
            padding: padding(6.0, 6.0, 0.0, 0.0),
            ..row(2.0)
        },
    )
}

/// A favicon stand-in: a rounded square in a colour derived from the host (as in the design).
pub fn favicon_color(url: &str) -> Color {
    const COLORS: [u32; 8] = [0xF2742B, 0x2E6BFF, 0x7C5CFF, 0x1F9D57, 0xE2457A, 0x1C5FA6, 0xD98A06, 0x8E3FA6];
    let host = display_host(url);
    let hash = host.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    Color::rgb(COLORS[hash as usize % COLORS.len()])
}

/// A sidebar row: accent icon, label, optional trailing glyph. Returns (label, trailing).
fn sidebar_row(
    ui: &mut Ui,
    parent: NodeId,
    action: ChromeAction,
    icon: Icon,
    text: &str,
    trailing: Option<Icon>,
) -> (NodeId, Option<NodeId>) {
    let row_node = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE),
        Style {
            size: Size { width: auto(), height: length(ROW_HEIGHT) },
            padding: padding(10.0, 12.0, 0.0, 0.0),
            ..row(12.0)
        },
    );
    ui.add(row_node, Widget::icon(icon, 22.0).with_paint(Paint::Accent), Style::default());
    let label = ui.add_label(row_node, text, TextStyle::Body, shrinking_label());
    let trailing = trailing
        .map(|t| ui.add(row_node, Widget::icon(t, 16.0).with_paint(Paint::Secondary), Style::default()));
    (label, trailing)
}

impl Chrome {
    pub fn build(ui: &mut Ui, config: &Config) -> Self {
        let root = ui.root();
        let layout = &config.layout;

        // ---- sidebar ----
        let sidebar = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Column,
                padding: padding(14.0, 14.0, 16.0, 16.0),
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );
        let header = ui.add(
            sidebar,
            Widget::container(),
            Style {
                justify_content: JustifyContent::END,
                size: Size { width: auto(), height: length(layout.bar_height) },
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(14.0),
                },
                ..row(GROUP_GAP)
            },
        );
        for (action, icon) in
            [(ChromeAction::NewTabGroup, Icon::NewTabGroup), (ChromeAction::ToggleSidebar, Icon::Sidebar)]
        {
            let button = ui.add(
                header,
                Widget::container()
                    .with_background(Background::Glass(GlassLevel::Thin))
                    .with_radius(CAPSULE)
                    .with_action(action.id()),
                Style { size: square(SIDEBAR_BUTTON), justify_content: JustifyContent::CENTER, ..row(0.0) },
            );
            ui.add(button, Widget::icon(icon, 22.0), Style::default());
        }
        // The normal sidebar content; a panel (bookmarks, history) replaces it when open.
        let sections = ui.add(sidebar, Widget::container(), column(2.0));
        let (tab_group_label, tab_group_chevron) = sidebar_row(
            ui,
            sections,
            ChromeAction::ToggleTabsSection,
            Icon::TabGroup,
            "Tabs",
            Some(Icon::ChevronDown),
        );
        let sidebar_tabs = ui.add(
            sections,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );
        sidebar_row(ui, sections, ChromeAction::Private, Icon::Hand, "Private", Some(Icon::Lock));
        let saved_header = ui.add(
            sections,
            Widget::container().with_action(ChromeAction::ToggleSavedSection.id()).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(ROW_HEIGHT) },
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(18.0),
                    bottom: length(0.0),
                },
                padding: padding(10.0, 12.0, 0.0, 0.0),
                ..row(8.0)
            },
        );
        let saved_title = ui.add_label(saved_header, "Saved", TextStyle::Title2, shrinking_label());
        if let Some(w) = ui.widget_mut(saved_title) {
            w.set_paint(Paint::Secondary);
        }
        let saved_chevron = ui.add(
            saved_header,
            Widget::icon(Icon::ChevronDown, 16.0).with_paint(Paint::Secondary),
            Style::default(),
        );
        let saved_list = ui.add(
            sections,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(2.0) },
                ..Default::default()
            },
        );
        sidebar_row(ui, saved_list, ChromeAction::Bookmarks, Icon::Book, "Bookmarks", None);
        sidebar_row(ui, saved_list, ChromeAction::ReadingList, Icon::Glasses, "Reading List", None);
        sidebar_row(ui, saved_list, ChromeAction::History, Icon::History, "History", None);

        let panel = ui.add(sidebar, Widget::container(), column(2.0));
        let panel_header = ui.add(
            panel,
            Widget::container(),
            Style { size: Size { width: auto(), height: length(ROW_HEIGHT) }, ..row(6.0) },
        );
        icon_button(
            ui,
            panel_header,
            ChromeAction::ClosePanel,
            Widget::icon(Icon::Back, 20.0).with_paint(Paint::Accent),
            36.0,
        );
        let panel_title = ui.add_label(panel_header, "", TextStyle::Title2, shrinking_label());
        let panel_list = ui.add(panel, Widget::container(), column(2.0));
        ui.set_visible(panel, false);

        // ---- toolbar ----
        let bar = ui.add(
            root,
            Widget::container(),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(layout.bar_height) },
                ..row(GROUP_GAP)
            },
        );
        let nav = glass_group(ui, bar);
        let (bar_sidebar_button, _) =
            icon_button(ui, nav, ChromeAction::ToggleSidebar, Widget::icon(Icon::Sidebar, 21.0), BUTTON);
        let (back, _) = icon_button(ui, nav, ChromeAction::Back, Widget::icon(Icon::Back, 21.0), BUTTON);
        let (forward, _) =
            icon_button(ui, nav, ChromeAction::Forward, Widget::icon(Icon::Forward, 21.0), BUTTON);

        let tab_capsule = ui.add(
            bar,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(160.0), height: auto() },
                size: Size { width: auto(), height: length(56.0) },
                padding: padding(6.0, 6.0, 0.0, 0.0),
                ..row(4.0)
            },
        );
        let address = ui.add(
            tab_capsule,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, 0.92))
                .with_radius(CAPSULE)
                .with_shadow(Shadow { offset_y: 2.0, blur: 10.0, opacity: 0.10 })
                .with_action(ChromeAction::EditAddress.id()),
            Style {
                flex_grow: ACTIVE_SEGMENT_GROW,
                flex_shrink: 1.0,
                flex_basis: length(0.0),
                min_size: Size { width: length(ACTIVE_SEGMENT_MIN), height: auto() },
                size: Size { width: auto(), height: length(SEGMENT_HEIGHT) },
                padding: padding(6.0, 6.0, 0.0, 0.0),
                ..row(4.0)
            },
        );
        let address_close = close_dot(ui, address, ChromeAction::CloseTab(0), 32.0);
        let shield_glyph = Widget::icon(Icon::Shield, 18.0).with_paint(Paint::Shield);
        icon_button(ui, address, ChromeAction::Shield, shield_glyph, 30.0);
        let address_label = ui.add_label(address, "", TextStyle::BodyEmphasis, shrinking_label());
        ui.set_label_align(address_label, Align::Start);
        let address_field = ui.add_text_field(
            address,
            "Search or enter website name",
            TextStyle::Body,
            Style { size: Size { width: auto(), height: length(22.0) }, ..shrinking_label() },
        );
        ui.set_visible(address_field, false);
        let (_, reload_icon) =
            icon_button(ui, address, ChromeAction::ReloadOrStop, Widget::icon(Icon::Reload, 19.0), 34.0);
        let (page_menu, _) =
            icon_button(ui, address, ChromeAction::PageMenu, Widget::icon(Icon::Menu, 20.0), 34.0);
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

        let actions = glass_group(ui, bar);
        icon_button(ui, actions, ChromeAction::Share, Widget::icon(Icon::Share, 21.0), BUTTON);
        icon_button(ui, actions, ChromeAction::NewTab, Widget::icon(Icon::Plus, 22.0), BUTTON);
        let (overview, _) =
            icon_button(ui, actions, ChromeAction::Overview, Widget::icon(Icon::Overview, 20.0), BUTTON);
        // Narrow windows: the overview button shows the tab count instead.
        let tab_count = ui.add(
            actions,
            Widget::container().with_action(ChromeAction::Overview.id()).with_radius(CAPSULE),
            Style { size: square(BUTTON), justify_content: JustifyContent::CENTER, ..row(0.0) },
        );
        let count_box = ui.add(
            tab_count,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.10)).with_radius(7.0),
            Style { size: square(22.0), justify_content: JustifyContent::CENTER, ..row(0.0) },
        );
        let tab_count_label = ui.add_label(count_box, "1", TextStyle::Caption, Style::default());

        let mut chrome = Self {
            mode: Mode::Wide,
            docked: true,
            docked_open: layout.sidebar_visible,
            overlay_open: false,
            page_fullscreen: false,
            tabs_section_open: true,
            saved_section_open: true,
            sidebar,
            sections,
            panel,
            panel_title,
            panel_list,
            tab_group_label,
            tab_group_chevron: tab_group_chevron.unwrap_or(sidebar),
            sidebar_tabs,
            saved_chevron,
            saved_list,
            bar,
            bar_sidebar_button,
            back,
            forward,
            tab_capsule,
            address,
            address_close,
            address_label,
            address_field,
            reload_icon,
            page_menu,
            progress,
            pills: Vec::new(),
            overview,
            tab_count,
            tab_count_label,
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

    /// Closes the floating sidebar (when the user goes back to the page). Returns whether it was open.
    pub fn dismiss_overlay(&mut self) -> bool {
        let was_open = !self.docked && self.overlay_open;
        self.overlay_open = false;
        was_open
    }

    /// Collapses or expands a sidebar section ("N Tabs" or "Saved").
    pub fn toggle_section(&mut self, ui: &mut Ui, tabs_section: bool) {
        let (open, list, chevron) = if tabs_section {
            self.tabs_section_open = !self.tabs_section_open;
            (self.tabs_section_open, self.sidebar_tabs, self.tab_group_chevron)
        } else {
            self.saved_section_open = !self.saved_section_open;
            (self.saved_section_open, self.saved_list, self.saved_chevron)
        };
        ui.set_visible(list, open);
        if let Some(w) = ui.widget_mut(chevron) {
            w.set_icon(if open { Icon::ChevronDown } else { Icon::ChevronRight });
        }
    }

    /// Shows a list (bookmarks, history) in place of the sidebar's sections. Rows send
    /// [`ChromeAction::OpenPanelItem`] with their index.
    pub fn show_panel(&mut self, ui: &mut Ui, title: &str, items: &[PanelItem], favicons: &Favicons) {
        ui.set_text(self.panel_title, title);
        ui.clear_children(self.panel_list);
        if items.is_empty() {
            let empty = ui.add_label(
                self.panel_list,
                "Nothing here yet",
                TextStyle::Body,
                Style { padding: padding(12.0, 12.0, 12.0, 12.0), ..Default::default() },
            );
            if let Some(w) = ui.widget_mut(empty) {
                w.set_paint(Paint::Secondary);
            }
        }
        for (i, item) in items.iter().enumerate() {
            let row_node = ui.add(
                self.panel_list,
                Widget::container().with_action(ChromeAction::OpenPanelItem(i).id()).with_radius(14.0),
                Style {
                    size: Size { width: auto(), height: length(PANEL_ROW_HEIGHT) },
                    padding: padding(10.0, 10.0, 0.0, 0.0),
                    ..row(12.0)
                },
            );
            favicon_node(ui, row_node, &item.url, 20.0, favicons);
            let text = ui.add(
                row_node,
                Widget::container(),
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..column(0.0)
                },
            );
            let title =
                if item.title.trim().is_empty() { display_host(&item.url) } else { item.title.clone() };
            ui.add_label(text, &title, TextStyle::Body, shrinking_label());
            let detail = ui.add_label(text, &item.detail, TextStyle::Footnote, shrinking_label());
            if let Some(w) = ui.widget_mut(detail) {
                w.set_paint(Paint::Secondary);
            }
        }
        ui.set_visible(self.sections, false);
        ui.set_visible(self.panel, true);
    }

    pub fn close_panel(&mut self, ui: &mut Ui) {
        ui.set_visible(self.panel, false);
        ui.set_visible(self.sections, true);
    }

    /// Positions the toolbar and sidebar for the window size (logical px).
    pub fn apply_layout(&mut self, ui: &mut Ui, config: &Config, window: Vec2) {
        let l = &config.layout;
        self.mode = if window.x >= l.wide_from {
            Mode::Wide
        } else if window.x >= l.narrow_below {
            Mode::Regular
        } else {
            Mode::Narrow
        };
        self.docked = self.sidebar_docked(config);
        let show_sidebar = self.sidebar_open() && !self.page_fullscreen;
        let m = l.margin;

        let bar_left = if self.docked && show_sidebar { l.sidebar_width + m } else { m };
        let mut bar_style = ui.style(self.bar);
        bar_style.inset = Rect { left: length(bar_left), right: length(m), top: length(m), bottom: auto() };
        bar_style.display = if self.page_fullscreen { Display::None } else { Display::Flex };
        ui.set_style(self.bar, bar_style);

        let mut side = ui.style(self.sidebar);
        side.size = Size { width: length(l.sidebar_width), height: auto() };
        side.inset = Rect { left: length(0.0), right: auto(), top: length(0.0), bottom: length(0.0) };
        side.display = if show_sidebar { Display::Flex } else { Display::None };
        ui.set_style(self.sidebar, side);
        if let Some(w) = ui.widget_mut(self.sidebar) {
            // Docked it's a panel beside the page; floating it needs more separation.
            w.background =
                Background::Glass(if self.docked { GlassLevel::Regular } else { GlassLevel::Thick });
        }

        let narrow = self.mode == Mode::Narrow;
        // With the sidebar open, its own header holds the toggle.
        ui.set_visible(self.bar_sidebar_button, !show_sidebar);
        ui.set_visible(self.forward, !narrow);
        for &pill in &self.pills {
            ui.set_visible(pill, !narrow);
        }
        ui.set_visible(self.overview, !narrow);
        ui.set_visible(self.tab_count, narrow);
    }

    /// The page's rectangle (logical px): below the toolbar, beside a docked sidebar, with a
    /// small margin to the window edges so its rounded corners read as a card.
    pub fn page_rect(&self, config: &Config, window: Vec2) -> Frame {
        if self.page_fullscreen {
            return Frame { origin: Vec2::ZERO, size: window };
        }
        let l = &config.layout;
        let left = if self.sidebar_docked(config) && self.docked_open {
            l.sidebar_width + l.page_margin
        } else {
            l.page_margin
        };
        let top = l.margin + l.bar_height + l.margin / 2.0;
        let origin = Vec2::new(left, top);
        let size = window - origin - Vec2::splat(l.page_margin);
        Frame { origin, size: size.max(Vec2::ONE) }
    }

    /// The active tab segment's frame (logical px), for attaching the suggestion list.
    pub fn address_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.address)
    }

    /// The page menu button's frame (logical px), for anchoring the menu.
    pub fn page_menu_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.page_menu)
    }

    pub fn page_fullscreen(&self) -> bool {
        self.page_fullscreen
    }

    pub fn set_page_fullscreen(&mut self, fullscreen: bool) {
        self.page_fullscreen = fullscreen;
    }

    /// Rebuilds the tab segments and sidebar rows from the tab model.
    pub fn sync_tabs(&mut self, ui: &mut Ui, tabs: &Tabs, favicons: &Favicons) {
        // Toolbar: one segment per inactive tab; the persistent address segment sits at the
        // active tab's position.
        for pill in std::mem::take(&mut self.pills) {
            ui.remove(pill);
        }
        let narrow = self.mode == Mode::Narrow;
        let mut order = Vec::with_capacity(tabs.len());
        for (i, tab) in tabs.all().iter().enumerate() {
            if i == tabs.active_index() {
                order.push(self.address);
                continue;
            }
            let pill = ui.add(
                self.tab_capsule,
                Widget::container().with_action(ChromeAction::SelectTab(i).id()).with_radius(CAPSULE),
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    flex_basis: length(0.0),
                    min_size: Size { width: length(56.0), height: auto() },
                    size: Size { width: auto(), height: length(SEGMENT_HEIGHT) },
                    padding: padding(14.0, 14.0, 0.0, 0.0),
                    ..row(9.0)
                },
            );
            favicon_node(ui, pill, &tab.url, 18.0, favicons);
            ui.add_label(pill, &tab.label(), TextStyle::Callout, shrinking_label());
            ui.set_visible(pill, !narrow);
            self.pills.push(pill);
            order.push(pill);
        }
        ui.set_children(self.tab_capsule, &order);
        if let Some(w) = ui.widget_mut(self.address_close) {
            w.action = Some(ChromeAction::CloseTab(tabs.active_index()).id());
        }

        // Sidebar: "N Tabs" and one row per tab; the active one is a pill with a close button.
        let count = tabs.len();
        ui.set_text(self.tab_group_label, &format!("{count} {}", if count == 1 { "Tab" } else { "Tabs" }));
        ui.clear_children(self.sidebar_tabs);
        for (i, tab) in tabs.all().iter().enumerate() {
            let active = i == tabs.active_index();
            let background = if active { Background::Fill(Paint::Label, 0.08) } else { Background::None };
            let row_node = ui.add(
                self.sidebar_tabs,
                Widget::container()
                    .with_action(ChromeAction::SelectTab(i).id())
                    .with_radius(CAPSULE)
                    .with_background(background),
                Style {
                    size: Size { width: auto(), height: length(ROW_HEIGHT) },
                    padding: padding(TAB_INDENT, 12.0, 0.0, 0.0),
                    ..row(12.0)
                },
            );
            if active {
                close_dot(ui, row_node, ChromeAction::CloseTab(i), 24.0);
            } else {
                // Same width as the close button, so titles line up.
                let slot = ui.add(
                    row_node,
                    Widget::container(),
                    Style { size: square(24.0), justify_content: JustifyContent::CENTER, ..row(0.0) },
                );
                favicon_node(ui, slot, &tab.url, 20.0, favicons);
            }
            let style = if active { TextStyle::BodyEmphasis } else { TextStyle::Body };
            ui.add_label(row_node, &tab.label(), style, shrinking_label());
        }
        ui.set_text(self.tab_count_label, &count.to_string());
        self.sync_active(ui, tabs);
    }

    /// Updates the active segment (address, back/forward, reload/stop, progress).
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

    /// Switches the active segment between showing the host and editing the full URL.
    pub fn set_editing(&mut self, ui: &mut Ui, editing: bool) -> Vec<UiEvent> {
        ui.set_visible(self.address_label, !editing);
        ui.set_visible(self.address_field, editing);
        if editing { ui.focus(self.address_field) } else { ui.blur() }
    }
}
