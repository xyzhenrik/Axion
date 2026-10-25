//! The browser chrome, modelled on iPadOS 26 Safari:
//!
//! - a toolbar of three glass groups: navigation (sidebar toggle when the sidebar is closed,
//!   back, forward), one capsule holding all tabs as equal segments — the active tab is the
//!   light pill with close, Shields, address, reload and page menu — and actions (share, new
//!   tab, tab overview);
//! - a full-height sidebar: header buttons, the tab list, Private, and a "Saved" section with
//!   bookmarks, reading list and history. Docked in wide windows, floating otherwise.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config, SidebarMode, TabBarStyle};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Align, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

use crate::favicons::{Favicons, favicon_node};
use crate::tabs::Tabs;
use crate::url::display_host;

/// The Now Playing card's content.
#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub subtitle: String,
    pub playing: bool,
}

const NOW_PLAYING_HEIGHT: f32 = 60.0;
/// The separate tab bar's row of tabs, and its gap to the toolbar.
const TAB_STRIP_HEIGHT: f32 = 38.0;
/// Tint of the capsule under the tabs that aren't open.
const TAB_CAPSULE_TINT: f32 = 0.06;
const TAB_STRIP_GAP: f32 = 8.0;
/// The pill with the site's name while the bars are hidden.
const MINI_HEIGHT: f32 = 28.0;
const MINI_TOP: f32 = 6.0;
/// The address field's widest size with the separate tab bar.
const SEPARATE_ADDRESS_MAX: f32 = 680.0;
const ADDRESS_FAVICON: f32 = 18.0;
const NOW_PLAYING_RADIUS: f32 = 20.0;

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
    /// The speaker in the address field: mutes or unmutes the active tab.
    MuteTab,
    /// The small pill shown while the bars are hidden: brings them back.
    ExpandBars,
    /// The Now Playing card: open its tab.
    NowPlaying,
    NowPlayingToggle,
    SelectTab(usize),
    CloseTab(usize),
    /// An entry in the open bookmarks/history panel.
    OpenPanelItem(usize),
    /// The × of a panel entry (deletes it from history).
    DeletePanelItem(usize),
    /// The panel header's clear button ("Clear History…").
    ClearPanel,
}

const SELECT_TAB_BASE: u32 = 1000;
const CLOSE_TAB_BASE: u32 = 2000;
const PANEL_ITEM_BASE: u32 = 7000;
const DELETE_PANEL_ITEM_BASE: u32 = 10_000;

impl ChromeAction {
    const SIMPLE: [ChromeAction; 23] = [
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
        ChromeAction::NowPlaying,
        ChromeAction::NowPlayingToggle,
        ChromeAction::MuteTab,
        ChromeAction::ExpandBars,
        ChromeAction::ClearPanel,
    ];

    fn id(self) -> ActionId {
        ActionId(match self {
            ChromeAction::SelectTab(i) => SELECT_TAB_BASE + i as u32,
            ChromeAction::CloseTab(i) => CLOSE_TAB_BASE + i as u32,
            ChromeAction::OpenPanelItem(i) => PANEL_ITEM_BASE + i as u32,
            ChromeAction::DeletePanelItem(i) => DELETE_PANEL_ITEM_BASE + i as u32,
            simple => 1 + Self::SIMPLE.iter().position(|&a| a == simple).unwrap_or(0) as u32,
        })
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        match id.0 {
            n @ 1..=23 => Some(Self::SIMPLE[n as usize - 1]),
            n if (DELETE_PANEL_ITEM_BASE..DELETE_PANEL_ITEM_BASE + 1000).contains(&n) => {
                Some(ChromeAction::DeletePanelItem((n - DELETE_PANEL_ITEM_BASE) as usize))
            }
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
    /// A heading shown above this row (e.g. the day in history).
    pub section: Option<String>,
    /// The row has a × that sends [`ChromeAction::DeletePanelItem`].
    pub deletable: bool,
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
const BUTTON: f32 = 38.0;
const SIDEBAR_BUTTON: f32 = 48.0;
const SEGMENT_HEIGHT: f32 = 36.0;
/// The toolbar's capsules (navigation, tabs, actions).
const BAR_CAPSULE: f32 = 46.0;
const ROW_HEIGHT: f32 = 42.0;
/// The active tab's segment grows this much more than the others and never gets narrower
/// than its minimum, so the address stays readable with many tabs.
const ACTIVE_SEGMENT_GROW: f32 = 1.5;
const ACTIVE_SEGMENT_MIN: f32 = 300.0;
const PANEL_ROW_HEIGHT: f32 = 52.0;
/// Extra distance the sidebar slides past the window edge, so its shadow leaves too.
const SLIDE_OVERSHOOT: f32 = 24.0;
const SLIDE_SETTLED: f32 = 0.3;
/// Indentation of tab rows under "N Tabs".
const TAB_INDENT: f32 = 18.0;

pub struct Chrome {
    now_playing: NodeId,
    now_playing_title: NodeId,
    now_playing_subtitle: NodeId,
    now_playing_icon: NodeId,
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
    /// Horizontal offset of the sidebar (0 = in place), animated.
    slide: Animated<f32>,
    private_row: NodeId,
    private_lock: NodeId,
    /// Private tabs are the current group (listed under "Private" instead of "N Tabs").
    private_mode: bool,
    /// How many normal tabs there are while private tabs are shown.
    normal_count: usize,
    locked: bool,
    sections: NodeId,
    panel: NodeId,
    panel_title: NodeId,
    /// The panel header's clear button and its label ("Clear History…").
    panel_clear: NodeId,
    panel_clear_label: NodeId,
    panel_viewport: NodeId,
    panel_scroll: f32,
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
    /// Where the address segment goes with the separate tab bar.
    address_slot: NodeId,
    /// The separate tab bar's row of tabs.
    tab_strip: NodeId,
    tab_bar: TabBarStyle,
    mini: NodeId,
    mini_label: NodeId,
    /// The bars are hidden (scrolled away) …
    collapsed: bool,
    /// … animated 0 (shown) → 1 (hidden).
    collapse: Animated<f32>,
    /// The bar's resting top inset (logical px), for the collapse animation.
    bar_top: f32,
    /// The active tab's segment; persistent so typing survives tab-list rebuilds.
    address: NodeId,
    address_close: NodeId,
    address_label: NodeId,
    pub address_field: NodeId,
    reload_icon: NodeId,
    page_menu: NodeId,
    shield: NodeId,
    shield_glyph: NodeId,
    address_favicon: NodeId,
    audio_button: NodeId,
    audio_icon: NodeId,
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
            size: Size { width: auto(), height: length(BAR_CAPSULE) },
            padding: padding(4.0, 4.0, 0.0, 0.0),
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
        let (private_label, private_lock) =
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

        // Fills the sidebar below its header, so the list scrolls inside it.
        let panel = ui.add(
            sidebar,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0),
                min_size: Size { width: auto(), height: length(0.0) },
                ..column(2.0)
            },
        );
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
        let panel_clear = ui.add(
            panel_header,
            Widget::container().with_action(ChromeAction::ClearPanel.id()).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(32.0) },
                padding: padding(12.0, 12.0, 0.0, 0.0),
                flex_shrink: 0.0,
                ..row(0.0)
            },
        );
        let panel_clear_label = ui.add_label(panel_clear, "", TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(panel_clear_label) {
            w.set_paint(Paint::Accent);
        }
        // Long lists (history) scroll inside a clipped viewport.
        let panel_viewport = ui.add(
            panel,
            Widget::container().with_clip(),
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0),
                flex_direction: FlexDirection::Column,
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
        );
        let panel_list = ui.add(
            panel_viewport,
            Widget::container(),
            Style { flex_shrink: 0.0, size: Size { width: percent(1.0), height: auto() }, ..column(2.0) },
        );
        ui.set_visible(panel, false);

        // Now Playing: at the bottom of the sidebar while a tab plays media.
        let now_playing = ui.add(
            sidebar,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_radius(NOW_PLAYING_RADIUS)
                .with_action(ChromeAction::NowPlaying.id()),
            Style {
                margin: Rect { left: length(0.0), right: length(0.0), top: auto(), bottom: length(0.0) },
                size: Size { width: auto(), height: length(NOW_PLAYING_HEIGHT) },
                padding: padding(14.0, 8.0, 0.0, 0.0),
                ..row(12.0)
            },
        );
        ui.add(now_playing, Widget::icon(Icon::Media, 22.0).with_paint(Paint::Accent), Style::default());
        let titles = ui.add(
            now_playing,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..column(0.0)
            },
        );
        let now_playing_title = ui.add_label(titles, "", TextStyle::Callout, shrinking_label());
        let now_playing_subtitle = ui.add_label(titles, "", TextStyle::Footnote, shrinking_label());
        if let Some(w) = ui.widget_mut(now_playing_subtitle) {
            w.set_paint(Paint::Secondary);
        }
        let (_, now_playing_icon) = icon_button(
            ui,
            now_playing,
            ChromeAction::NowPlayingToggle,
            Widget::icon(Icon::Pause, 20.0),
            40.0,
        );
        ui.set_visible(now_playing, false);

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

        // Separate tab bar: the address segment sits here, centred between the groups.
        let address_slot = ui.add(
            bar,
            Widget::container(),
            Style { flex_grow: 1.0, flex_shrink: 1.0, justify_content: JustifyContent::CENTER, ..row(0.0) },
        );
        ui.set_visible(address_slot, false);
        // The other tabs sit on a plain tinted capsule; only the open tab (the address) is glass,
        // lying on top of them.
        let tab_capsule = ui.add(
            bar,
            Widget::container()
                .with_background(Background::Fill(Paint::Label, TAB_CAPSULE_TINT))
                .with_radius(CAPSULE),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(160.0), height: auto() },
                size: Size { width: auto(), height: length(BAR_CAPSULE) },
                padding: padding(5.0, 5.0, 0.0, 0.0),
                ..row(4.0)
            },
        );
        let address = ui.add(
            tab_capsule,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Regular))
                .with_radius(CAPSULE)
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
        let (shield, shield_glyph) = icon_button(ui, address, ChromeAction::Shield, shield_glyph, 30.0);
        // The site's icon, like on the other tabs' segments.
        let address_favicon = ui.add(
            address,
            Widget::container(),
            Style { size: square(ADDRESS_FAVICON), flex_shrink: 0.0, ..Default::default() },
        );
        // While the tab plays sound (Safari: a blue speaker in the address field).
        let (audio_button, audio_icon) = icon_button(
            ui,
            address,
            ChromeAction::MuteTab,
            Widget::icon(Icon::Volume, 18.0).with_paint(Paint::Accent),
            30.0,
        );
        ui.set_visible(audio_button, false);
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
                inset: Rect { left: length(16.0), right: length(16.0), top: auto(), bottom: length(0.0) },
                size: Size { width: auto(), height: length(3.0) },
                ..Default::default()
            },
        );

        let actions = glass_group(ui, bar);
        // Separate tab bar: the row of tabs under the toolbar.
        let tab_strip = ui.add(
            root,
            Widget::container()
                .with_background(Background::Fill(Paint::Label, TAB_CAPSULE_TINT))
                .with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(TAB_STRIP_HEIGHT) },
                padding: padding(4.0, 4.0, 0.0, 0.0),
                ..row(4.0)
            },
        );
        ui.set_visible(tab_strip, false);
        // While scrolling down the bars give way to this pill with the site's name (Safari).
        let mini = ui.add(
            root,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_radius(CAPSULE)
                .with_action(ChromeAction::ExpandBars.id()),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(MINI_HEIGHT) },
                padding: padding(16.0, 16.0, 0.0, 0.0),
                ..row(0.0)
            },
        );
        let mini_label = ui.add_label(mini, "", TextStyle::Footnote, Style::default());
        ui.set_visible(mini, false);
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

        // The tab row is drawn before the toolbar, so the toolbar's popovers (page menu,
        // suggestions, Shields) lie above it.
        let mut planes = ui.children(root);
        if let (Some(strip_at), Some(bar_at)) =
            (planes.iter().position(|&n| n == tab_strip), planes.iter().position(|&n| n == bar))
            && strip_at > bar_at
        {
            planes.remove(strip_at);
            planes.insert(bar_at, tab_strip);
            ui.set_children(root, &planes);
        }
        let mut chrome = Self {
            now_playing,
            now_playing_title,
            now_playing_subtitle,
            now_playing_icon,
            mode: Mode::Wide,
            docked: true,
            docked_open: layout.sidebar_visible,
            overlay_open: false,
            page_fullscreen: false,
            tabs_section_open: true,
            saved_section_open: true,
            sidebar,
            slide: Animated::new(0.0, Spring::SMOOTH),
            private_row: ui.parent(private_label).unwrap_or(sidebar),
            private_lock: private_lock.unwrap_or(sidebar),
            private_mode: false,
            normal_count: 0,
            locked: false,
            sections,
            panel,
            panel_title,
            panel_clear,
            panel_clear_label,
            panel_viewport,
            panel_scroll: 0.0,
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
            address_slot,
            tab_strip,
            tab_bar: TabBarStyle::Compact,
            mini,
            mini_label,
            collapsed: false,
            collapse: Animated::new(0.0, Spring::SMOOTH),
            bar_top: 0.0,
            address,
            address_close,
            address_label,
            address_field,
            reload_icon,
            page_menu,
            shield,
            shield_glyph,
            address_favicon,
            audio_button,
            audio_icon,
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

    /// How much of a docked sidebar is in view right now (logical px; follows its slide).
    fn docked_sidebar_width(&self, config: &Config) -> f32 {
        if !self.docked || self.page_fullscreen {
            return 0.0;
        }
        let width = config.layout.sidebar_width;
        (width + self.slide.value).clamp(0.0, width)
    }

    /// The sidebar is sliding (the page and toolbar follow it every frame).
    pub fn is_sliding(&self) -> bool {
        !self.slide.is_settled(SLIDE_SETTLED)
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
    /// `clear` is the label of the header's clear button (`None`: no button). `keep_scroll`
    /// keeps the list where it was (after deleting an entry).
    pub fn show_panel(
        &mut self,
        ui: &mut Ui,
        title: &str,
        items: &[PanelItem],
        favicons: &Favicons,
        clear: Option<&str>,
        keep_scroll: bool,
    ) {
        ui.set_text(self.panel_title, title);
        ui.set_visible(self.panel_clear, clear.is_some());
        self.set_panel_clear_label(ui, clear.unwrap_or_default());
        if !keep_scroll {
            self.set_panel_scroll(ui, 0.0);
        }
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
            if let Some(section) = &item.section {
                let heading = ui.add_label(
                    self.panel_list,
                    section,
                    TextStyle::BodyEmphasis,
                    Style {
                        padding: padding(10.0, 10.0, if i == 0 { 4.0 } else { 14.0 }, 4.0),
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                );
                if let Some(w) = ui.widget_mut(heading) {
                    w.set_paint(Paint::Secondary);
                }
            }
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
            if item.deletable {
                let glyph = Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary);
                icon_button(ui, row_node, ChromeAction::DeletePanelItem(i), glyph, 30.0);
            }
        }
        ui.set_visible(self.sections, false);
        ui.set_visible(self.panel, true);
    }

    pub fn set_panel_clear_label(&self, ui: &mut Ui, text: &str) {
        ui.set_text(self.panel_clear_label, text);
    }

    pub fn is_panel_open(&self, ui: &Ui) -> bool {
        ui.is_visible(self.panel) && ui.is_visible(self.sidebar)
    }

    /// Whether `p` (logical px) is over the panel's list.
    pub fn panel_contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.is_panel_open(ui) && ui.frame(self.panel_viewport).contains(p)
    }

    /// Scrolls the panel's list by `dy` logical px (wheel: positive scrolls up).
    pub fn scroll_panel(&mut self, ui: &mut Ui, dy: f32) {
        let max = (ui.frame(self.panel_list).size.y - ui.frame(self.panel_viewport).size.y).max(0.0);
        let scroll = (self.panel_scroll - dy).clamp(0.0, max);
        self.set_panel_scroll(ui, scroll);
    }

    fn set_panel_scroll(&mut self, ui: &mut Ui, scroll: f32) {
        self.panel_scroll = scroll;
        let mut style = ui.style(self.panel_list);
        style.margin =
            Rect { left: length(0.0), right: length(0.0), top: length(-scroll), bottom: length(0.0) };
        ui.set_style(self.panel_list, style);
    }

    pub fn close_panel(&mut self, ui: &mut Ui) {
        ui.set_visible(self.panel, false);
        ui.set_visible(self.sections, true);
    }

    /// Marks private mode in the sidebar, and whether a password lock exists (lock icon).
    /// The number of normal tabs, shown on "N Tabs" while private tabs are open.
    pub fn set_normal_count(&mut self, count: usize) {
        self.normal_count = count;
    }

    pub fn set_private(&mut self, ui: &mut Ui, private: bool, has_lock: bool) {
        self.private_mode = private;
        if let Some(w) = ui.widget_mut(self.private_row) {
            w.background = if private { Background::Fill(Paint::Label, 0.08) } else { Background::None };
        }
        ui.set_visible(self.private_lock, has_lock);
    }

    /// While private browsing is locked, tab titles and the address stay hidden.
    pub fn set_locked(&mut self, ui: &mut Ui, locked: bool) {
        self.locked = locked;
        ui.set_visible(self.sidebar_tabs, !locked && self.tabs_section_open);
        for &pill in &self.pills {
            ui.set_visible(pill, !locked && self.mode != Mode::Narrow);
        }
        if locked {
            ui.set_text(self.address_label, "Private Browsing");
        }
    }

    /// Hides the bars (scrolling down) or shows them again. Returns whether that changed, so
    /// the page can be resized.
    pub fn set_collapsed(&mut self, collapsed: bool, config: &Config) -> bool {
        if collapsed == self.collapsed {
            return false;
        }
        self.collapsed = collapsed;
        let motion = &config.motion;
        self.collapse.spring = if config.glass.reduce_motion {
            Spring::new(0.2, 1.0)
        } else {
            Spring::new(motion.smooth.response * motion.speed, motion.smooth.damping)
        };
        self.collapse.target = if collapsed { 1.0 } else { 0.0 };
        true
    }

    /// The site shown in the pill while the bars are hidden.
    pub fn set_mini_host(&self, ui: &mut Ui, host: &str) {
        ui.set_text(self.mini_label, host);
    }

    fn animate_collapse(&mut self, ui: &mut Ui, dt: f32, centre_x: f32) {
        self.collapse.step(dt);
        let k = self.collapse.value.clamp(0.0, 1.0);
        let shift = k * (self.bar_top + 2.0 * (TAB_STRIP_HEIGHT + TAB_STRIP_GAP) + BAR_CAPSULE);
        for (node, top) in
            [(self.bar, self.bar_top), (self.tab_strip, self.bar_top + BAR_CAPSULE + TAB_STRIP_GAP)]
        {
            let mut style = ui.style(node);
            style.inset.top = length(top - shift);
            ui.set_style(node, style);
            ui.set_opacity(node, 1.0 - k);
        }
        let visible = k > 0.01 && !self.page_fullscreen;
        ui.set_visible(self.mini, visible);
        if visible {
            let width = ui.frame(self.mini).size.x;
            let mut style = ui.style(self.mini);
            style.inset = Rect {
                left: length((centre_x - width / 2.0).max(0.0)),
                right: auto(),
                top: length(MINI_TOP),
                bottom: auto(),
            };
            ui.set_style(self.mini, style);
            ui.set_opacity(self.mini, k);
        }
    }

    /// Advances the sidebar's slide and the bars' collapse. Call every frame.
    /// `centre_x`: the page's horizontal centre, where the pill sits while the bars are hidden.
    pub fn animate_all(&mut self, ui: &mut Ui, dt: f32, centre_x: f32) {
        if !self.collapse.is_settled(0.001) || self.collapsed {
            self.animate_collapse(ui, dt, centre_x);
        }
        self.animate(ui, dt);
    }

    /// Advances the sidebar's slide. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if self.slide.is_settled(SLIDE_SETTLED) {
            if self.slide.target < 0.0 && ui.is_visible(self.sidebar) {
                ui.set_visible(self.sidebar, false);
            }
            return;
        }
        self.slide.step(dt);
        let mut side = ui.style(self.sidebar);
        side.inset.left = length(self.slide.value);
        ui.set_style(self.sidebar, side);
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

        // Docked, the toolbar starts where the (possibly sliding) sidebar ends.
        let bar_left = if self.docked { self.docked_sidebar_width(config).max(0.0) + m } else { m };
        let mut bar_style = ui.style(self.bar);
        self.bar_top = m;
        bar_style.inset = Rect { left: length(bar_left), right: length(m), top: length(m), bottom: auto() };
        bar_style.display = if self.page_fullscreen { Display::None } else { Display::Flex };
        ui.set_style(self.bar, bar_style);

        // Separate tab bar: the address alone in the toolbar, the tabs in a row below.
        self.tab_bar = l.tab_bar;
        let separate = l.tab_bar == TabBarStyle::Separate;
        ui.set_visible(self.tab_capsule, !separate);
        ui.set_visible(self.address_slot, separate);
        ui.set_visible(self.tab_strip, separate && !self.page_fullscreen);
        let mut strip = ui.style(self.tab_strip);
        strip.inset = Rect {
            left: length(bar_left),
            right: length(m),
            top: length(m + l.bar_height + TAB_STRIP_GAP),
            bottom: auto(),
        };
        ui.set_style(self.tab_strip, strip);
        let mut address = ui.style(self.address);
        if separate {
            address.flex_grow = 1.0;
            address.max_size = Size { width: length(SEPARATE_ADDRESS_MAX), height: auto() };
            address.min_size = Size { width: length(0.0), height: auto() };
        } else {
            address.flex_grow = ACTIVE_SEGMENT_GROW;
            address.max_size = Size { width: auto(), height: auto() };
            address.min_size = Size { width: length(ACTIVE_SEGMENT_MIN), height: auto() };
        }
        ui.set_style(self.address, address);

        // The sidebar slides in and out on a spring (see `animate`).
        let hidden_x = -(l.sidebar_width + SLIDE_OVERSHOOT);
        if show_sidebar && !ui.is_visible(self.sidebar) {
            self.slide.snap(hidden_x);
        }
        self.slide.target = if show_sidebar { 0.0 } else { hidden_x };
        let motion = &config.motion;
        self.slide.spring = if config.glass.reduce_motion {
            Spring::new(0.2, 1.0)
        } else {
            Spring::new(motion.smooth.response * motion.speed, motion.smooth.damping)
        };
        let mut side = ui.style(self.sidebar);
        side.size = Size { width: length(l.sidebar_width), height: auto() };
        side.inset =
            Rect { left: length(self.slide.value), right: auto(), top: length(0.0), bottom: length(0.0) };
        side.display =
            if show_sidebar || ui.is_visible(self.sidebar) { Display::Flex } else { Display::None };
        ui.set_style(self.sidebar, side);
        if let Some(w) = ui.widget_mut(self.sidebar) {
            // Docked it's a panel beside the page; floating it needs more separation.
            w.background =
                Background::Glass(if self.docked { GlassLevel::Regular } else { GlassLevel::Thick });
        }

        let narrow = self.mode == Mode::Narrow || self.locked;
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
        // The page's left edge follows the sidebar as it slides.
        let left = self.docked_sidebar_width(config) + l.page_margin;
        let strip = if l.tab_bar == TabBarStyle::Separate { TAB_STRIP_HEIGHT + TAB_STRIP_GAP } else { 0.0 };
        let top = if self.collapsed {
            MINI_TOP + MINI_HEIGHT + l.page_margin / 2.0
        } else {
            l.margin + l.bar_height + strip + l.margin / 2.0
        };
        let origin = Vec2::new(left, top);
        let size = window - origin - Vec2::splat(l.page_margin);
        Frame { origin, size: size.max(Vec2::ONE) }
    }

    /// The toolbar node: popovers that should flow out of the bar's glass live in it.
    pub fn bar_node(&self) -> NodeId {
        self.bar
    }

    /// The active tab segment's frame (logical px), for attaching the suggestion list.
    pub fn address_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.address)
    }

    /// The page menu button's frame (logical px), for anchoring the menu.
    pub fn page_menu_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.page_menu)
    }

    /// Shows the Now Playing card (`None` hides it).
    pub fn set_now_playing(&self, ui: &mut Ui, now: Option<&NowPlaying>) {
        ui.set_visible(self.now_playing, now.is_some());
        let Some(now) = now else { return };
        ui.set_text(self.now_playing_title, &now.title);
        ui.set_text(self.now_playing_subtitle, &now.subtitle);
        ui.set_icon(self.now_playing_icon, if now.playing { Icon::Pause } else { Icon::Play });
    }

    pub fn shield_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.shield)
    }

    /// The address field's speaker: shown while the active tab plays sound or is muted.
    pub fn set_audio(&self, ui: &mut Ui, playing: bool, muted: bool) {
        ui.set_visible(self.audio_button, (playing || muted) && !self.locked);
        ui.set_icon(self.audio_icon, if muted { Icon::Muted } else { Icon::Volume });
    }

    /// Green while Shields protect the page, grey when they are off for its site.
    pub fn set_shield_active(&self, ui: &mut Ui, active: bool) {
        if let Some(w) = ui.widget_mut(self.shield_glyph) {
            w.set_paint(if active { Paint::Shield } else { Paint::Secondary });
        }
    }

    pub fn page_fullscreen(&self) -> bool {
        self.page_fullscreen
    }

    pub fn set_page_fullscreen(&mut self, fullscreen: bool) {
        self.page_fullscreen = fullscreen;
    }

    /// Rebuilds the tab segments and sidebar rows from the tab model.
    pub fn sync_tabs(&mut self, ui: &mut Ui, tabs: &Tabs, favicons: &Favicons) {
        for pill in std::mem::take(&mut self.pills) {
            ui.remove(pill);
        }
        let narrow = self.mode == Mode::Narrow;
        let separate = self.tab_bar == TabBarStyle::Separate;
        let mut order = Vec::with_capacity(tabs.len());
        for (i, tab) in tabs.all().iter().enumerate() {
            let active = i == tabs.active_index();
            if active && !separate {
                // Compact: the persistent address segment sits at the active tab's position.
                order.push(self.address);
                continue;
            }
            let parent = if separate { self.tab_strip } else { self.tab_capsule };
            let background = if active { Background::Glass(GlassLevel::Regular) } else { Background::None };
            let height = if separate { TAB_STRIP_HEIGHT - 8.0 } else { SEGMENT_HEIGHT };
            let pill = ui.add(
                parent,
                Widget::container()
                    .with_action(ChromeAction::SelectTab(i).id())
                    .with_radius(CAPSULE)
                    .with_background(background),
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    flex_basis: length(0.0),
                    min_size: Size { width: length(56.0), height: auto() },
                    size: Size { width: auto(), height: length(height) },
                    padding: padding(if active { 6.0 } else { 14.0 }, 14.0, 0.0, 0.0),
                    justify_content: if separate { JustifyContent::CENTER } else { JustifyContent::START },
                    ..row(9.0)
                },
            );
            if active {
                close_dot(ui, pill, ChromeAction::CloseTab(i), 24.0);
            }
            favicon_node(ui, pill, &tab.url, 16.0, favicons);
            let style = if active { TextStyle::BodyEmphasis } else { TextStyle::Callout };
            ui.add_label(pill, &tab.label(), style, shrinking_label());
            ui.set_visible(pill, !narrow || separate);
            self.pills.push(pill);
            order.push(pill);
        }
        if separate {
            ui.set_children(self.tab_capsule, &[]);
            ui.set_children(self.tab_strip, &order);
            ui.set_children(self.address_slot, &[self.address]);
        } else {
            ui.set_children(self.address_slot, &[]);
            ui.set_children(self.tab_capsule, &order);
        }
        if let Some(w) = ui.widget_mut(self.address_close) {
            w.action = Some(ChromeAction::CloseTab(tabs.active_index()).id());
        }
        ui.set_visible(self.address_close, !separate);
        ui.clear_children(self.address_favicon);
        if let Some(tab) = tabs.active() {
            favicon_node(ui, self.address_favicon, &tab.url, ADDRESS_FAVICON, favicons);
        }
        ui.set_visible(self.address_favicon, !self.locked);

        // Sidebar: each group's tabs are listed under its row — under "N Tabs" normally, under
        // "Private" in private browsing (where "N Tabs" counts the normal tabs and switches back).
        let count = if self.private_mode { self.normal_count } else { tabs.len() };
        ui.set_text(self.tab_group_label, &format!("{count} {}", if count == 1 { "Tab" } else { "Tabs" }));
        if let Some(header) = ui.parent(self.tab_group_label) {
            let mut order: Vec<NodeId> =
                ui.children(self.sections).into_iter().filter(|&n| n != self.sidebar_tabs).collect();
            let after = if self.private_mode { self.private_row } else { header };
            if let Some(at) = order.iter().position(|&n| n == after) {
                order.insert(at + 1, self.sidebar_tabs);
                ui.set_children(self.sections, &order);
            }
        }
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
        let host = if self.locked { "Private Browsing".to_owned() } else { display_host(&tab.url) };
        ui.set_text(self.address_label, &host);
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
