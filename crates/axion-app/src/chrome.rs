//! The browser chrome, modelled on iPadOS 26 Safari:
//!
//! - a toolbar of three glass groups: navigation (sidebar toggle when the sidebar is closed,
//!   back, forward), one capsule holding all tabs as equal segments — the active tab is the
//!   light pill with close, Shields, address, reload and page menu — and actions (share, new
//!   tab, tab overview);
//! - a full-height sidebar: header buttons, the tab list, Private, and a "Saved" section with
//!   bookmarks, reading list and history; Now Playing and the account button sit at its
//!   bottom. Docked in wide windows, floating otherwise.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config, SidebarMode, TabBarStyle};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Align, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

use axion_engine::ViewId;

use crate::favicons::{Favicons, favicon_node};
use crate::tabs::{Tab, Tabs};
use crate::url::display_host;

/// The Now Playing card's content.
#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub subtitle: String,
    pub playing: bool,
}

const NOW_PLAYING_HEIGHT: f32 = 60.0;
/// The account button at the bottom of the sidebar, and its avatar.
const ACCOUNT_HEIGHT: f32 = 48.0;
const ACCOUNT_AVATAR: f32 = 30.0;
/// The separate tab bar's row of tabs, and its gap to the toolbar.
const TAB_STRIP_HEIGHT: f32 = 38.0;
/// Tint of the capsule under the tabs that aren't open.
const TAB_CAPSULE_TINT: f32 = 0.06;
const TAB_STRIP_GAP: f32 = 8.0;
/// Below this the F11 fullscreen transition counts as settled.
const PRESENTATION_SETTLED: f32 = 0.001;
/// The address field's widest size with the separate tab bar.
const SEPARATE_ADDRESS_MAX: f32 = 680.0;
/// The loading line's distance from the address field's ends.
const PROGRESS_INSET: f32 = 16.0;
const ADDRESS_FAVICON: f32 = 18.0;
const NOW_PLAYING_RADIUS: f32 = 20.0;
const DOWNLOAD_BAR_WIDTH: f32 = 18.0;
const PANEL_PROGRESS_HEIGHT: f32 = 4.0;
/// Extra height of a panel row with a progress bar.
const PANEL_PROGRESS_ROOM: f32 = 12.0;
/// Between two tabs (separate tab bar and compact capsule), and the strip's inner padding.
const TAB_GAP: f32 = 4.0;
const TAB_STRIP_PADDING: f32 = 4.0;
const TAB_CAPSULE_PADDING: f32 = 5.0;
/// Tabs never get narrower than this, even when the preferred width doesn't fit (then they
/// overflow the bar).
const TAB_HARD_MIN: f32 = 56.0;
/// The hairline between two tabs of the separate tab bar.
const TAB_SEPARATOR_HEIGHT: f32 = 18.0;
/// Its strength (label colour at this opacity, so it reads on light and dark chrome).
const TAB_SEPARATOR_ALPHA: f32 = 0.22;
/// The bookmarks bar under the toolbar (and the separate tab bar), its items and their gap.
const BOOKMARKS_BAR_HEIGHT: f32 = 34.0;
const BOOKMARK_ITEM_HEIGHT: f32 = 26.0;
const BOOKMARK_MAX_WIDTH: f32 = 180.0;
const BOOKMARK_GAP: f32 = 2.0;
const BOOKMARKS_BAR_PADDING: f32 = 5.0;
const BOOKMARK_FAVICON: f32 = 16.0;

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
    /// The Now Playing card: open its tab.
    NowPlaying,
    /// The bookmarks bar's chevron: the bookmarks that don't fit, as a menu.
    BookmarksOverflow,
    /// The toolbar's home button: opens the homepage.
    Home,
    /// A bookmark of the bookmarks bar (index into the bar's list).
    OpenBookmark(usize),
    NowPlayingToggle,
    SelectTab(usize),
    /// A tab of the other group (private while browsing normally, or the reverse): switches
    /// group and selects it.
    SelectOtherTab(usize),
    CloseTab(usize),
    /// An entry in the open bookmarks/history panel.
    OpenPanelItem(usize),
    /// The × of a panel entry (deletes it from history).
    DeletePanelItem(usize),
    /// The panel header's clear button ("Clear History…").
    ClearPanel,
    /// The sidebar's Downloads row.
    Downloads,
    /// The toolbar's download button (opens the downloads popover).
    DownloadsButton,
    /// The `k`th trailing button of panel entry `i` (a download's pause, open …).
    PanelButton(usize, usize),
    /// Opens the Passwords sheet.
    Passwords,
    /// The account button: opens the account menu.
    Account,
    /// The chevron of "N Tabs": collapses or expands the normal tabs' list.
    CollapseTabs,
    /// The chevron of "Private" (while unlocked): collapses or expands the private list.
    CollapsePrivate,
}

/// The sidebar's sections that fold away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Tabs,
    Private,
    Saved,
}

/// Where tabs are shown, for dragging them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The sidebar's lists (both groups: drops can move a tab to the other group).
    Sidebar,
    /// The toolbar's tab capsule or the separate tab bar's row (the shown group only).
    Bar,
}

/// A tab as one of the chrome's draggable items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabItem {
    pub node: NodeId,
    pub view: ViewId,
    /// Its group (`true`: private).
    pub private: bool,
    /// Its index in that group.
    pub index: usize,
    pub surface: Surface,
}

/// The sidebar's lists as laid out when a drag starts (logical px): a drop's place is decided
/// against these, so the rows sliding aside don't move the targets.
#[derive(Clone, Debug, Default)]
pub struct SidebarLayout {
    /// The "Private" row, and each list's rows: (view, frame).
    pub private_header: Frame,
    pub normal: Vec<(ViewId, Frame)>,
    pub private: Vec<(ViewId, Frame)>,
    /// Whether the lists are shown (folded away, or private tabs locked).
    pub normal_open: bool,
    pub private_open: bool,
}

const SELECT_TAB_BASE: u32 = 1000;
const CLOSE_TAB_BASE: u32 = 2000;
const PANEL_ITEM_BASE: u32 = 7000;
const DELETE_PANEL_ITEM_BASE: u32 = 10_000;
const SELECT_OTHER_TAB_BASE: u32 = 12_000;
const PANEL_BUTTON_BASE: u32 = 13_000;
const BOOKMARK_BASE: u32 = 18_000;
/// Trailing buttons a panel entry can have.
pub const PANEL_BUTTONS: usize = 4;

impl ChromeAction {
    const SIMPLE: [ChromeAction; 30] = [
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
        ChromeAction::ClearPanel,
        ChromeAction::Downloads,
        ChromeAction::DownloadsButton,
        ChromeAction::Passwords,
        ChromeAction::Account,
        ChromeAction::CollapseTabs,
        ChromeAction::CollapsePrivate,
        ChromeAction::BookmarksOverflow,
        ChromeAction::Home,
    ];

    pub fn id(self) -> ActionId {
        ActionId(match self {
            ChromeAction::SelectTab(i) => SELECT_TAB_BASE + i as u32,
            ChromeAction::CloseTab(i) => CLOSE_TAB_BASE + i as u32,
            ChromeAction::SelectOtherTab(i) => SELECT_OTHER_TAB_BASE + i as u32,
            ChromeAction::OpenPanelItem(i) => PANEL_ITEM_BASE + i as u32,
            ChromeAction::DeletePanelItem(i) => DELETE_PANEL_ITEM_BASE + i as u32,
            ChromeAction::PanelButton(i, k) => PANEL_BUTTON_BASE + (i * PANEL_BUTTONS + k) as u32,
            ChromeAction::OpenBookmark(i) => BOOKMARK_BASE + i as u32,
            simple => 1 + Self::SIMPLE.iter().position(|&a| a == simple).unwrap_or(0) as u32,
        })
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        match id.0 {
            n if (1..=Self::SIMPLE.len() as u32).contains(&n) => Some(Self::SIMPLE[n as usize - 1]),
            n if (BOOKMARK_BASE..BOOKMARK_BASE + 1000).contains(&n) => {
                Some(ChromeAction::OpenBookmark((n - BOOKMARK_BASE) as usize))
            }
            n if (PANEL_BUTTON_BASE..PANEL_BUTTON_BASE + 1000 * PANEL_BUTTONS as u32).contains(&n) => {
                let k = (n - PANEL_BUTTON_BASE) as usize;
                Some(ChromeAction::PanelButton(k / PANEL_BUTTONS, k % PANEL_BUTTONS))
            }
            n if (SELECT_OTHER_TAB_BASE..SELECT_OTHER_TAB_BASE + 1000).contains(&n) => {
                Some(ChromeAction::SelectOtherTab((n - SELECT_OTHER_TAB_BASE) as usize))
            }
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
#[derive(Default)]
pub struct PanelItem {
    pub title: String,
    pub url: String,
    /// Second line (host, or when it was visited).
    pub detail: String,
    /// A heading shown above this row (e.g. the day in history).
    pub section: Option<String>,
    /// The row has a × that sends [`ChromeAction::DeletePanelItem`].
    pub deletable: bool,
    /// Shown instead of the URL's favicon (a download's file).
    pub icon: Option<Icon>,
    /// A progress bar under the detail (0–1; a download's).
    pub progress: Option<f32>,
    /// Trailing buttons, sending [`ChromeAction::PanelButton`] (at most [`PANEL_BUTTONS`]).
    pub buttons: Vec<Icon>,
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
const TAB_INDENT: f32 = 10.0;
/// A tab row's close button: a small dot beside the favicon (its slot is kept on every row so
/// titles line up).
const ROW_CLOSE_SLOT: f32 = 20.0;
const ROW_CLOSE_DOT: f32 = 16.0;
const ROW_FAVICON: f32 = 20.0;
/// The chevron buttons of "N Tabs" and "Private".
const CHEVRON_BUTTON: f32 = 28.0;

pub struct Chrome {
    account: NodeId,
    account_avatar: NodeId,
    account_initials: NodeId,
    account_name: NodeId,
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
    /// The chevron of "Private", shown instead of the lock while private tabs are unlocked.
    private_chevron: NodeId,
    private_chevron_icon: NodeId,
    private_section_open: bool,
    /// The tab rows of the sidebar and the bar, as built by `sync_tabs`.
    items: Vec<TabItem>,
    /// The row under the pointer, lit with a glass highlight (`update_hover`).
    hovered_row: Option<NodeId>,
    /// Private tabs are the current group (listed under "Private" instead of "N Tabs").
    private_mode: bool,
    /// How many normal tabs there are while private tabs are shown.
    normal_count: usize,
    locked: bool,
    /// Private tab titles are hidden (a lock is set and locked), in either mode.
    private_hidden: bool,
    sections: NodeId,
    sections_viewport: NodeId,
    sections_scroll: f32,
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
    /// The current group's tabs, under its row.
    sidebar_tabs: NodeId,
    /// The other group's tabs, under its row (normal tabs under "N Tabs" in private browsing).
    sidebar_other: NodeId,
    saved_chevron: NodeId,
    saved_list: NodeId,

    bar: NodeId,
    bar_sidebar_button: NodeId,
    back: NodeId,
    forward: NodeId,
    home_button: NodeId,
    tab_capsule: NodeId,
    /// Where the address segment goes with the separate tab bar.
    address_slot: NodeId,
    /// The separate tab bar's row of tabs.
    tab_strip: NodeId,
    tab_bar: TabBarStyle,
    /// Whole-page fullscreen (F11): all of the chrome gives way to the page …
    presentation: bool,
    /// … animated 0 (chrome shown) → 1 (only the page).
    presentation_k: Animated<f32>,
    /// The bar's resting top inset (logical px), for the fullscreen transition.
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
    /// The toolbar's download button, its progress bar and the bar's fill.
    downloads: NodeId,
    download_track: NodeId,
    download_fill: NodeId,
    /// What the download button last showed (visible, progress in %).
    download_state: Option<(bool, Option<u8>)>,
    tab_widths: TabWidths,
    bookmarks: BookmarkRow,
}

/// Tab segment widths: every tab of the bar is equally wide (the preferred width, shrinking
/// equally when they don't all fit), and changes spring there.
struct TabWidths {
    /// Segments that follow the shared width (not the compact address segment), and whether
    /// each is pinned (pinned tabs have their own, fixed width).
    pills: Vec<(NodeId, bool)>,
    width: Animated<f32>,
    /// The width was set once (the first one snaps instead of springing from nothing).
    ready: bool,
    applied: f32,
    preferred: f32,
    min: f32,
    pinned: f32,
    reduce_motion: bool,
    /// The separate tab bar's tabs: (segment, the hairline before it, active).
    separators: Vec<(NodeId, Option<NodeId>, bool)>,
    /// The hovered node the hairlines were last shown for (`None`: not yet).
    separator_hover: Option<Option<NodeId>>,
    /// Where the separate tab bar may lie (left edge, width; logical px): it is only as wide
    /// as its tabs, centred in this span.
    strip_area: (f32, f32),
    /// The strip's applied left edge and width.
    strip_applied: (f32, f32),
}

/// The bookmarks bar: favicons and titles on a glass capsule, what doesn't fit behind a
/// chevron.
struct BookmarkRow {
    bar: NodeId,
    list: NodeId,
    more: NodeId,
    hint: NodeId,
    items: Vec<NodeId>,
    /// The items' natural widths, measured after they were built (`None` until then).
    widths: Option<Vec<f32>>,
    /// How many items fit (the rest are in the chevron's menu).
    fit: usize,
    /// What the items were built from (url, title, has a favicon), to rebuild only on change.
    signature: Vec<(String, String, bool)>,
    shown: bool,
    /// Where the bar lies (left edge, width; logical px).
    area: (f32, f32),
    /// The bar's applied left edge and width.
    applied: (f32, f32),
}

/// The width each of `count` equally wide tabs gets in `available` px (gaps between them
/// included): `preferred`, or less when they don't fit, but never under `min`.
pub fn equal_tab_width(available: f32, count: usize, gap: f32, preferred: f32, min: f32) -> f32 {
    if count == 0 {
        return preferred.max(min);
    }
    let share = (available - gap * (count - 1) as f32) / count as f32;
    share.min(preferred).max(min)
}

/// How many of the bookmarks bar's items (natural `widths`, `gap` apart) fit into `available`
/// px; when not all do, room for the chevron (`more` px) is kept.
pub fn bookmarks_that_fit(widths: &[f32], gap: f32, available: f32, more: f32) -> usize {
    let total: f32 = widths.iter().sum::<f32>() + gap * widths.len().saturating_sub(1) as f32;
    if total <= available {
        return widths.len();
    }
    let room = available - more - gap;
    let mut used = 0.0;
    for (i, w) in widths.iter().enumerate() {
        used += w + if i > 0 { gap } else { 0.0 };
        if used > room {
            return i;
        }
    }
    widths.len()
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
    close_dot_sized(ui, parent, action, hit, 20.0)
}

fn close_dot_sized(ui: &mut Ui, parent: NodeId, action: ChromeAction, hit: f32, dot: f32) -> NodeId {
    let button = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE),
        Style { size: square(hit), justify_content: JustifyContent::CENTER, flex_shrink: 0.0, ..row(0.0) },
    );
    let dot_node = ui.add(
        button,
        Widget::container().with_background(Background::Fill(Paint::Label, 0.85)).with_radius(CAPSULE),
        Style { size: square(dot), justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let cross =
        Widget::icon(Icon::Close, dot * 0.55).with_icon_weight(IconWeight::Heavy).with_paint(Paint::Surface);
    ui.add(dot_node, cross, Style::default());
    button
}

/// A group's row ("N Tabs", "Private"): accent icon and label, then a chevron button that
/// folds its list, and (Private only) a lock shown instead while locked. Returns (label,
/// chevron button, chevron icon, lock).
fn group_row(
    ui: &mut Ui,
    parent: NodeId,
    action: ChromeAction,
    collapse: ChromeAction,
    icon: Icon,
    text: &str,
    lock: bool,
) -> (NodeId, NodeId, NodeId, Option<NodeId>) {
    let row_node = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE),
        Style {
            size: Size { width: auto(), height: length(ROW_HEIGHT) },
            padding: padding(10.0, 6.0, 0.0, 0.0),
            ..row(12.0)
        },
    );
    ui.add(row_node, Widget::icon(icon, 22.0).with_paint(Paint::Accent), Style::default());
    let label = ui.add_label(row_node, text, TextStyle::Body, shrinking_label());
    let lock = lock.then(|| {
        let slot = ui.add(
            row_node,
            Widget::container(),
            Style { size: square(CHEVRON_BUTTON), justify_content: JustifyContent::CENTER, ..row(0.0) },
        );
        ui.add(slot, Widget::icon(Icon::Lock, 16.0).with_paint(Paint::Secondary), Style::default());
        slot
    });
    let glyph = Widget::icon(Icon::ChevronDown, 16.0).with_paint(Paint::Secondary);
    let (chevron, chevron_icon) = icon_button(ui, row_node, collapse, glyph, CHEVRON_BUTTON);
    (label, chevron, chevron_icon, lock)
}

/// A glass capsule grouping toolbar buttons: one element, so pressing any of its buttons
/// presses the whole capsule (see `Widget::press_whole`).
fn glass_group(ui: &mut Ui, parent: NodeId) -> NodeId {
    ui.add(
        parent,
        Widget::container()
            .with_background(Background::Glass(GlassLevel::Thin))
            .with_radius(CAPSULE)
            .pressed_whole(),
        Style {
            size: Size { width: auto(), height: length(BAR_CAPSULE) },
            padding: padding(4.0, 4.0, 0.0, 0.0),
            ..row(2.0)
        },
    )
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
        // The normal sidebar content; a panel (bookmarks, history) replaces it when open. It
        // scrolls in a clipped viewport, so the footer (account) stays pinned and rows slide
        // away behind its edge.
        let sections_viewport = ui.add(
            sidebar,
            Widget::container().with_clip(),
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0),
                min_size: Size { width: auto(), height: length(0.0) },
                flex_direction: FlexDirection::Column,
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
        );
        let sections = ui.add(
            sections_viewport,
            Widget::container(),
            Style { flex_shrink: 0.0, size: Size { width: percent(1.0), height: auto() }, ..column(2.0) },
        );
        let (tab_group_label, _, tab_group_chevron, _) = group_row(
            ui,
            sections,
            ChromeAction::ToggleTabsSection,
            ChromeAction::CollapseTabs,
            Icon::TabGroup,
            "Tabs",
            false,
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
        let sidebar_other = ui.add(sections, Widget::container(), ui.style(sidebar_tabs));
        let (private_label, private_chevron, private_chevron_icon, private_lock) = group_row(
            ui,
            sections,
            ChromeAction::Private,
            ChromeAction::CollapsePrivate,
            Icon::Hand,
            "Private",
            true,
        );
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
        sidebar_row(ui, saved_list, ChromeAction::Downloads, Icon::Download, "Downloads", None);
        sidebar_row(ui, saved_list, ChromeAction::Passwords, Icon::Key, "Passwords", None);

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

        // Pinned to the bottom of the sidebar: Now Playing (while a tab plays media) above the
        // account button.
        let footer = ui.add(
            sidebar,
            Widget::container(),
            Style {
                margin: Rect { left: length(0.0), right: length(0.0), top: auto(), bottom: length(0.0) },
                flex_shrink: 0.0,
                ..column(8.0)
            },
        );
        let now_playing = ui.add(
            footer,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_radius(NOW_PLAYING_RADIUS)
                .with_action(ChromeAction::NowPlaying.id())
                .pressed_whole(),
            Style {
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
        // The account button (like Claude's app): avatar with initials, then the name, on a
        // thin glass capsule lying on the sidebar's glass. The account menu flows out of it.
        let account = ui.add(
            footer,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_action(ChromeAction::Account.id())
                .with_radius(CAPSULE)
                .pressed_whole(),
            Style {
                size: Size { width: auto(), height: length(ACCOUNT_HEIGHT) },
                padding: padding(6.0, 12.0, 0.0, 0.0),
                ..row(10.0)
            },
        );
        let account_avatar = ui.add(
            account,
            Widget::container().with_background(Background::Fill(Paint::Accent, 1.0)).with_radius(CAPSULE),
            Style {
                size: square(ACCOUNT_AVATAR),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let account_initials = ui.add_label(account_avatar, "", TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(account_initials) {
            w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
        }
        let account_name = ui.add_label(account, "", TextStyle::BodyEmphasis, shrinking_label());
        ui.add(
            account,
            Widget::icon(Icon::ChevronUpDown, 15.0).with_paint(Paint::Secondary),
            Style::default(),
        );

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
        let (home_button, _) =
            icon_button(ui, nav, ChromeAction::Home, Widget::icon(Icon::Home, 20.0), BUTTON);

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
                // Its width is the room between the groups, whatever the tabs in it want.
                flex_basis: length(0.0),
                min_size: Size { width: length(160.0), height: auto() },
                size: Size { width: auto(), height: length(BAR_CAPSULE) },
                padding: padding(TAB_CAPSULE_PADDING, TAB_CAPSULE_PADDING, 0.0, 0.0),
                ..row(TAB_GAP)
            },
        );
        let address = ui.add(
            tab_capsule,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_radius(CAPSULE)
                .with_action(ChromeAction::EditAddress.id())
                .pressed_whole(),
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
            crate::home::ADDRESS_PLACEHOLDER,
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
                inset: Rect {
                    left: length(PROGRESS_INSET),
                    right: length(PROGRESS_INSET),
                    top: auto(),
                    bottom: length(0.0),
                },
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
                padding: padding(TAB_STRIP_PADDING, TAB_STRIP_PADDING, 0.0, 0.0),
                // Equal-width tabs sit in the middle of the strip until they fill it.
                justify_content: JustifyContent::CENTER,
                ..row(TAB_GAP)
            },
        );
        ui.set_visible(tab_strip, false);
        let bookmarks = BookmarkRow::build(ui, root);
        // While downloads run (Safari): an arrow over a thin progress bar; opens the list.
        let downloads = ui.add(
            actions,
            Widget::container().with_action(ChromeAction::DownloadsButton.id()).with_radius(CAPSULE),
            Style {
                size: square(BUTTON),
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..column(2.0)
            },
        );
        ui.add(downloads, Widget::icon(Icon::Download, 19.0), Style::default());
        let download_track = ui.add(
            downloads,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.15)).with_radius(CAPSULE),
            Style { size: Size { width: length(DOWNLOAD_BAR_WIDTH), height: length(3.0) }, ..row(0.0) },
        );
        let download_fill = ui.add(
            download_track,
            Widget::container().with_background(Background::Fill(Paint::Accent, 1.0)).with_radius(CAPSULE),
            Style { size: Size { width: percent(0.0), height: percent(1.0) }, ..Default::default() },
        );
        ui.set_visible(downloads, false);
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
        // The bookmarks bar before both, so their menus lie above it.
        let mut planes = ui.children(root);
        if let Some(at) = planes.iter().position(|&n| n == bookmarks.bar) {
            planes.remove(at);
            let strip_at = planes.iter().position(|&n| n == tab_strip).unwrap_or(0);
            planes.insert(strip_at, bookmarks.bar);
            ui.set_children(root, &planes);
        }
        let mut chrome = Self {
            account,
            account_avatar,
            account_initials,
            account_name,
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
            private_chevron,
            private_chevron_icon,
            private_section_open: true,
            items: Vec::new(),
            hovered_row: None,
            private_mode: false,
            normal_count: 0,
            locked: false,
            private_hidden: false,
            sections,
            sections_viewport,
            sections_scroll: 0.0,
            panel,
            panel_title,
            panel_clear,
            panel_clear_label,
            panel_viewport,
            panel_scroll: 0.0,
            panel_list,
            tab_group_label,
            tab_group_chevron,
            sidebar_tabs,
            sidebar_other,
            saved_chevron,
            saved_list,
            bar,
            bar_sidebar_button,
            back,
            forward,
            home_button,
            tab_capsule,
            address_slot,
            tab_strip,
            tab_bar: TabBarStyle::Compact,
            presentation: false,
            presentation_k: Animated::new(0.0, Spring::SMOOTH),
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
            downloads,
            download_track,
            download_fill,
            download_state: None,
            tab_widths: TabWidths {
                pills: Vec::new(),
                width: Animated::new(layout.tab_width, Spring::SMOOTH),
                ready: false,
                applied: -1.0,
                preferred: layout.tab_width,
                min: layout.tab_min_width,
                pinned: layout.pinned_tab_width,
                reduce_motion: false,
                separators: Vec::new(),
                separator_hover: None,
                strip_area: (0.0, 0.0),
                strip_applied: (-1.0, -1.0),
            },
            bookmarks,
        };
        chrome.apply_layout(ui, config, Vec2::new(1440.0, 900.0));
        chrome
    }

    fn sidebar_docked(&self, config: &Config) -> bool {
        match config.layout.sidebar {
            _ if config.layout.float_sidebar => false,
            SidebarMode::Docked => self.mode != Mode::Narrow,
            SidebarMode::Overlay => false,
            SidebarMode::Auto => self.mode == Mode::Wide,
        }
    }

    /// A floating sidebar lies over the bars (in small windows it covers the toolbar's left
    /// end, which must not draw over it); a docked one sits beside them, under their popovers.
    fn stack_sidebar(&self, ui: &mut Ui) {
        let Some(root) = ui.parent(self.sidebar) else { return };
        let mut order: Vec<NodeId> = ui.children(root).into_iter().filter(|&n| n != self.sidebar).collect();
        let bars = [self.bar, self.tab_strip, self.bookmarks.bar];
        let at = if self.docked {
            order.iter().position(|n| bars.contains(n)).unwrap_or(0)
        } else {
            order.iter().rposition(|n| bars.contains(n)).map_or(order.len(), |i| i + 1)
        };
        order.insert(at, self.sidebar);
        if order != ui.children(root) {
            ui.set_children(root, &order);
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

    /// The sidebar is sliding, or the chrome is moving for F11 fullscreen (the page and
    /// toolbar follow it every frame).
    pub fn is_sliding(&self) -> bool {
        !self.slide.is_settled(SLIDE_SETTLED) || !self.presentation_k.is_settled(PRESENTATION_SETTLED)
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

    /// Collapses or expands a sidebar section ("N Tabs", "Private" or "Saved").
    pub fn toggle_section(&mut self, ui: &mut Ui, section: Section) {
        let (open, chevron) = match section {
            Section::Tabs => {
                self.tabs_section_open = !self.tabs_section_open;
                (self.tabs_section_open, self.tab_group_chevron)
            }
            Section::Private => {
                self.private_section_open = !self.private_section_open;
                (self.private_section_open, self.private_chevron_icon)
            }
            Section::Saved => {
                self.saved_section_open = !self.saved_section_open;
                ui.set_visible(self.saved_list, self.saved_section_open);
                (self.saved_section_open, self.saved_chevron)
            }
        };
        self.update_tab_lists(ui);
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
                    size: Size {
                        width: auto(),
                        height: length(
                            PANEL_ROW_HEIGHT
                                + if item.progress.is_some() { PANEL_PROGRESS_ROOM } else { 0.0 },
                        ),
                    },
                    padding: padding(10.0, 10.0, 0.0, 0.0),
                    ..row(12.0)
                },
            );
            match item.icon {
                Some(icon) => {
                    ui.add(row_node, Widget::icon(icon, 22.0).with_paint(Paint::Accent), Style::default());
                }
                None => {
                    favicon_node(ui, row_node, &item.url, 20.0, favicons);
                }
            }
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
            if let Some(progress) = item.progress {
                let track = ui.add(
                    text,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Label, 0.12))
                        .with_radius(CAPSULE),
                    Style {
                        size: Size { width: percent(1.0), height: length(PANEL_PROGRESS_HEIGHT) },
                        margin: Rect {
                            left: length(0.0),
                            right: length(0.0),
                            top: length(3.0),
                            bottom: length(3.0),
                        },
                        flex_shrink: 0.0,
                        ..row(0.0)
                    },
                );
                ui.add(
                    track,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Accent, 1.0))
                        .with_radius(CAPSULE),
                    Style {
                        size: Size { width: percent(progress.clamp(0.0, 1.0)), height: percent(1.0) },
                        ..Default::default()
                    },
                );
            }
            let detail = ui.add_label(text, &item.detail, TextStyle::Footnote, shrinking_label());
            if let Some(w) = ui.widget_mut(detail) {
                w.set_paint(Paint::Secondary);
            }
            for (k, &icon) in item.buttons.iter().take(PANEL_BUTTONS).enumerate() {
                let glyph = Widget::icon(icon, 15.0).with_paint(Paint::Accent);
                icon_button(ui, row_node, ChromeAction::PanelButton(i, k), glyph, 30.0);
            }
            if item.deletable {
                let glyph = Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary);
                icon_button(ui, row_node, ChromeAction::DeletePanelItem(i), glyph, 30.0);
            }
        }
        ui.set_visible(self.sections_viewport, false);
        ui.set_visible(self.panel, true);
    }

    /// Shows the toolbar's download button with `progress` (0–1) on its bar, or hides it.
    /// `None` progress (size unknown) hides the bar.
    pub fn set_download_indicator(&mut self, ui: &mut Ui, shown: bool, progress: Option<f32>) {
        // Called every frame: only touch the tree on change (in 1 % steps).
        let state = (shown, progress.map(|p| (p.clamp(0.0, 1.0) * 100.0).round() as u8));
        if self.download_state == Some(state) {
            return;
        }
        self.download_state = Some(state);
        ui.set_visible(self.downloads, shown);
        ui.set_visible(self.download_track, progress.is_some());
        let mut style = ui.style(self.download_fill);
        style.size.width = percent(progress.unwrap_or(0.0).clamp(0.0, 1.0));
        ui.set_style(self.download_fill, style);
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

    /// Whether `p` (logical px) is over the sidebar's scrolling sections.
    pub fn sections_contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.is_visible(self.sidebar)
            && ui.is_visible(self.sections_viewport)
            && ui.frame(self.sections_viewport).contains(p)
    }

    /// Scrolls the sidebar's sections by `dy` logical px (wheel: positive scrolls up).
    pub fn scroll_sections(&mut self, ui: &mut Ui, dy: f32) {
        let max = (ui.frame(self.sections).size.y - ui.frame(self.sections_viewport).size.y).max(0.0);
        let scroll = (self.sections_scroll - dy).clamp(0.0, max);
        if scroll == self.sections_scroll {
            return;
        }
        self.sections_scroll = scroll;
        let mut style = ui.style(self.sections);
        style.margin = Rect {
            left: length(0.0),
            right: length(0.0),
            top: length(-self.sections_scroll),
            bottom: length(0.0),
        };
        ui.set_style(self.sections, style);
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
        ui.set_visible(self.sections_viewport, true);
    }

    /// Marks private mode in the sidebar, and whether a password lock exists (lock icon).
    /// The number of normal tabs, shown on "N Tabs" while private tabs are open.
    pub fn set_normal_count(&mut self, count: usize) {
        self.normal_count = count;
    }

    /// `hidden`: private tabs are locked away, so their titles are not listed and "Private"
    /// shows a lock; unlocked (or without a lock) it has a chevron that folds the list.
    pub fn set_private(&mut self, ui: &mut Ui, private: bool, has_lock: bool, hidden: bool) {
        self.private_mode = private;
        self.private_hidden = hidden;
        self.update_tab_lists(ui);
        if let Some(w) = ui.widget_mut(self.private_row) {
            w.background = if private { Background::Fill(Paint::Label, 0.08) } else { Background::None };
        }
        let locked = has_lock && hidden;
        ui.set_visible(self.private_lock, locked);
        ui.set_visible(self.private_chevron, !locked);
    }

    /// While private browsing is locked, tab titles and the address stay hidden.
    pub fn set_locked(&mut self, ui: &mut Ui, locked: bool) {
        self.locked = locked;
        self.update_tab_lists(ui);
        for &pill in &self.pills {
            ui.set_visible(pill, !locked && self.mode != Mode::Narrow);
        }
        if locked {
            ui.set_text(self.address_label, "Private Browsing");
        }
    }

    /// Whole-page fullscreen (F11) on or off: the bars slide up and fade, the sidebar slides
    /// away and the page grows to the whole window (or all of it comes back). Returns whether
    /// that changed; the caller relayouts (the sidebar's slide starts in `apply_layout`).
    pub fn set_presentation(&mut self, on: bool, config: &Config) -> bool {
        if on == self.presentation {
            return false;
        }
        self.presentation = on;
        let motion = &config.motion;
        self.presentation_k.spring = if config.glass.reduce_motion {
            Spring::new(0.2, 1.0)
        } else {
            Spring::new(motion.smooth.response * motion.speed, motion.smooth.damping)
        };
        self.presentation_k.target = if on { 1.0 } else { 0.0 };
        true
    }

    /// Whole-page fullscreen (F11) is on.
    pub fn presentation(&self) -> bool {
        self.presentation
    }

    /// Something is fullscreen — the whole page (F11) or an element of it (video): Axion's
    /// chrome is out of the way, so nothing but transient UI may cover the page.
    pub fn is_fullscreen(&self) -> bool {
        self.presentation || self.page_fullscreen
    }

    /// How far the chrome has given way to the page (0 shown … 1 page only).
    fn chrome_away(&self) -> f32 {
        if self.page_fullscreen { 1.0 } else { self.presentation_k.value.clamp(0.0, 1.0) }
    }

    /// F11 fullscreen has fully hidden the bars.
    fn chrome_gone(&self) -> bool {
        self.presentation && self.presentation_k.is_settled(PRESENTATION_SETTLED)
    }

    fn animate_presentation(&mut self, ui: &mut Ui, dt: f32) {
        self.presentation_k.step(dt);
        let k = self.presentation_k.value.clamp(0.0, 1.0);
        let shift = k * (self.bar_top + 2.0 * (TAB_STRIP_HEIGHT + TAB_STRIP_GAP) + BAR_CAPSULE);
        let gone = self.chrome_gone();
        let separate = self.tab_bar == TabBarStyle::Separate;
        for (node, top, shown) in [
            (self.bar, self.bar_top, true),
            (self.tab_strip, self.bar_top + BAR_CAPSULE + TAB_STRIP_GAP, separate),
        ] {
            let mut style = ui.style(node);
            style.inset.top = length(top - shift);
            ui.set_style(node, style);
            ui.set_opacity(node, 1.0 - k);
            // Gone entirely once out of sight: no glass is drawn over the page.
            ui.set_visible(node, shown && !self.page_fullscreen && !gone);
        }
    }

    /// Advances the sidebar's slide and the F11 fullscreen transition. Call every frame.
    pub fn animate_all(&mut self, ui: &mut Ui, dt: f32) {
        if !self.presentation_k.is_settled(PRESENTATION_SETTLED) || self.presentation {
            self.animate_presentation(ui, dt);
        }
        self.animate(ui, dt);
    }

    /// Keeps the loading line inside the address field. Its insets alone resolved against the
    /// field's unclamped width (the separate tab bar caps the field), so it ran past the end.
    fn fit_progress(&self, ui: &mut Ui) {
        let width = (ui.frame(self.address).size.x - 2.0 * PROGRESS_INSET).max(0.0);
        let mut style = ui.style(self.progress);
        let wanted = length(width);
        if style.size.width != wanted {
            style.size.width = wanted;
            style.inset.right = auto();
            ui.set_style(self.progress, style);
        }
    }

    /// Advances the sidebar's slide (and the tabs' widths, the bookmarks bar's fit). Call
    /// every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.animate_tab_widths(ui, dt);
        self.fit_bookmarks(ui);
        self.fit_progress(ui);
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
        self.stack_sidebar(ui);
        let show_sidebar = self.sidebar_open() && !self.is_fullscreen();
        let m = l.margin;

        // Docked, the toolbar starts where the (possibly sliding) sidebar ends.
        let bar_left = if self.docked { self.docked_sidebar_width(config).max(0.0) + m } else { m };
        let mut bar_style = ui.style(self.bar);
        self.bar_top = m;
        bar_style.inset = Rect { left: length(bar_left), right: length(m), top: length(m), bottom: auto() };
        bar_style.display = if self.page_fullscreen { Display::None } else { Display::Flex };
        ui.set_style(self.bar, bar_style);

        // Separate tab bar: the address alone in the toolbar, the tabs in a row below.
        if self.tab_bar != l.tab_bar {
            self.tab_widths.ready = false;
        }
        self.tab_bar = l.tab_bar;
        ui.set_visible(self.home_button, l.home_button);
        let separate = l.tab_bar == TabBarStyle::Separate;
        ui.set_visible(self.tab_capsule, !separate);
        ui.set_visible(self.address_slot, separate);
        ui.set_visible(self.tab_strip, separate && !self.page_fullscreen && !self.chrome_gone());
        let mut strip = ui.style(self.tab_strip);
        // Left and width follow the tabs (see `animate_tab_widths`).
        let area = (bar_left, (window.x - bar_left - m).max(0.0));
        if area != self.tab_widths.strip_area {
            self.tab_widths.strip_area = area;
            self.tab_widths.strip_applied = (-1.0, -1.0);
        }
        strip.inset.top = length(m + l.bar_height + TAB_STRIP_GAP);
        strip.inset.bottom = auto();
        strip.inset.right = auto();
        ui.set_style(self.tab_strip, strip);
        self.layout_bookmarks_bar(ui, config, bar_left, window.x);
        let widths = &mut self.tab_widths;
        widths.preferred = l.tab_width;
        widths.min = l.tab_min_width;
        widths.pinned = l.pinned_tab_width;
        widths.reduce_motion = config.glass.reduce_motion;
        widths.width.spring =
            Spring::new(config.motion.smooth.response * config.motion.speed, config.motion.smooth.damping);
        widths.applied = -1.0;
        let mut address = ui.style(self.address);
        if separate {
            address.flex_grow = 1.0;
            address.max_size = Size { width: length(SEPARATE_ADDRESS_MAX), height: auto() };
            address.min_size = Size { width: length(0.0), height: auto() };
            // On its own between the groups it is as tall as the buttons' capsules.
            address.size.height = length(BAR_CAPSULE);
        } else {
            address.size.height = length(SEGMENT_HEIGHT);
            // The other tabs are as wide as the tab width; the address takes the rest.
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
        // Floating, the sidebar is a rounded card at a margin from the window's edges.
        let gap = if self.docked { 0.0 } else { l.margin };
        self.slide.target = if show_sidebar { gap } else { hidden_x };
        let motion = &config.motion;
        self.slide.spring = if config.glass.reduce_motion {
            Spring::new(0.2, 1.0)
        } else {
            Spring::new(motion.smooth.response * motion.speed, motion.smooth.damping)
        };
        let mut side = ui.style(self.sidebar);
        side.size = Size { width: length(l.sidebar_width), height: auto() };
        side.inset =
            Rect { left: length(self.slide.value), right: auto(), top: length(gap), bottom: length(gap) };
        side.display =
            if show_sidebar || ui.is_visible(self.sidebar) { Display::Flex } else { Display::None };
        ui.set_style(self.sidebar, side);
        if let Some(w) = ui.widget_mut(self.sidebar) {
            // Docked it's a panel beside the page; floating it needs more separation.
            w.background =
                Background::Glass(if self.docked { GlassLevel::Regular } else { GlassLevel::Thick });
            w.radius = if self.docked { 0.0 } else { l.sidebar_corner_radius };
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
        self.card_rect(config, window, self.chrome_away(), self.docked_sidebar_width(config))
    }

    /// The page card for a docked sidebar `sidebar` wide, with the chrome `away` (0–1) gone
    /// (F11: the card's margins shrink away as the chrome leaves).
    fn card_rect(&self, config: &Config, window: Vec2, away: f32, sidebar: f32) -> Frame {
        if away >= 1.0 {
            return Frame { origin: Vec2::ZERO, size: window };
        }
        let l = &config.layout;
        // The page's left edge follows the sidebar as it slides.
        let left = sidebar + l.page_margin;
        let strip = if l.tab_bar == TabBarStyle::Separate { TAB_STRIP_HEIGHT + TAB_STRIP_GAP } else { 0.0 }
            + self.bookmarks_room();
        let top = l.margin + l.bar_height + strip + l.margin / 2.0;
        let keep = 1.0 - away;
        let origin = Vec2::new(left, top) * keep;
        let size = window - origin - Vec2::splat(l.page_margin * keep);
        Frame { origin, size: size.max(Vec2::ONE) }
    }

    /// The page card's corner radius (logical px): square while fullscreen.
    pub fn page_corner_radius(&self, config: &Config) -> f32 {
        config.layout.page_corner_radius * (1.0 - self.chrome_away())
    }

    /// The page's rectangle once the sidebar's slide and the F11 transition have settled (see
    /// [`Self::page_rect`]).
    pub fn settled_page_rect(&self, config: &Config, window: Vec2) -> Frame {
        if self.page_fullscreen || self.presentation {
            return Frame { origin: Vec2::ZERO, size: window };
        }
        let sidebar = if self.docked {
            let width = config.layout.sidebar_width;
            (width + self.slide.target).clamp(0.0, width)
        } else {
            0.0
        };
        self.card_rect(config, window, 0.0, sidebar)
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
    /// Shows the current account on the account button.
    pub fn set_account(&self, ui: &mut Ui, name: &str, initials: &str, color: Color) {
        ui.set_text(self.account_name, name);
        ui.set_text(self.account_initials, initials);
        if let Some(w) = ui.widget_mut(self.account_avatar) {
            w.background = Background::Fill(Paint::Fixed(color), 1.0);
        }
    }

    /// The account button (logical px), where the account menu rises from.
    pub fn account_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.account)
    }

    /// The Now Playing card (the account menu covers it).
    pub fn now_playing_node(&self) -> NodeId {
        self.now_playing
    }

    /// The sidebar, the plane the account menu's glass belongs to.
    pub fn sidebar_node(&self) -> NodeId {
        self.sidebar
    }

    pub fn set_now_playing(&self, ui: &mut Ui, now: Option<&NowPlaying>) {
        ui.set_visible(self.now_playing, now.is_some());
        let Some(now) = now else { return };
        ui.set_text(self.now_playing_title, &now.title);
        ui.set_text(self.now_playing_subtitle, &now.subtitle);
        ui.set_icon(self.now_playing_icon, if now.playing { Icon::Pause } else { Icon::Play });
    }

    /// The toolbar's download button (logical px), for anchoring the downloads popover.
    pub fn downloads_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.downloads)
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

    /// The normal and the private tabs' lists.
    fn group_lists(&self) -> (NodeId, NodeId) {
        if self.private_mode {
            (self.sidebar_other, self.sidebar_tabs)
        } else {
            (self.sidebar_tabs, self.sidebar_other)
        }
    }

    fn private_list_open(&self) -> bool {
        self.private_section_open && !self.private_hidden && !self.locked
    }

    /// Shows the normal tabs' list while "N Tabs" is expanded and the private one while
    /// "Private" is expanded and unlocked.
    fn update_tab_lists(&self, ui: &mut Ui) {
        let (normal, private) = self.group_lists();
        ui.set_visible(normal, self.tabs_section_open);
        ui.set_visible(private, self.private_list_open());
    }

    /// The tab item `node` is (a sidebar row, a bar segment, or the address segment that is the
    /// active tab in the compact bar), if it is one.
    pub fn tab_item(&self, node: NodeId) -> Option<TabItem> {
        self.items.iter().copied().find(|item| item.node == node)
    }

    /// The items of `surface`, in order.
    pub fn tab_items(&self, surface: Surface) -> Vec<TabItem> {
        self.items.iter().copied().filter(|i| i.surface == surface).collect()
    }

    /// The sidebar's lists as laid out now (see [`SidebarLayout`]).
    pub fn sidebar_layout(&self, ui: &Ui) -> SidebarLayout {
        let rows = |private: bool| -> Vec<(ViewId, Frame)> {
            self.items
                .iter()
                .filter(|i| i.surface == Surface::Sidebar && i.private == private)
                .map(|i| (i.view, ui.frame(i.node)))
                .collect()
        };
        SidebarLayout {
            private_header: ui.frame(self.private_row),
            normal: rows(false),
            private: rows(true),
            normal_open: self.tabs_section_open,
            private_open: self.private_list_open(),
        }
    }

    /// While a tab is dragged: puts its row (or segment) at `target` (`(private, index)` among
    /// the other items; `None` leaves it where it was) and hides it, so the others make room.
    /// Cleared by the next `sync_tabs`.
    pub fn preview_drag(
        &mut self,
        ui: &mut Ui,
        view: ViewId,
        surface: Surface,
        target: Option<(bool, usize)>,
    ) {
        let Some(source) = self.items.iter().copied().find(|i| i.view == view && i.surface == surface) else {
            return;
        };
        ui.set_opacity(source.node, 0.0);
        let Some((private, index)) = target else { return };
        match surface {
            Surface::Sidebar => {
                let (normal, private_list) = self.group_lists();
                let into = if private { private_list } else { normal };
                let from = ui.parent(source.node).unwrap_or(into);
                if from != into {
                    let rest: Vec<NodeId> =
                        ui.children(from).into_iter().filter(|&n| n != source.node).collect();
                    ui.set_children(from, &rest);
                }
                let mut order: Vec<NodeId> =
                    ui.children(into).into_iter().filter(|&n| n != source.node).collect();
                order.insert(index.min(order.len()), source.node);
                ui.set_children(into, &order);
            }
            Surface::Bar => {
                let Some(parent) = ui.parent(source.node) else { return };
                let mut order: Vec<NodeId> =
                    ui.children(parent).into_iter().filter(|&n| n != source.node).collect();
                order.insert(index.min(order.len()), source.node);
                ui.set_children(parent, &order);
            }
        }
    }

    /// The nodes that move when tabs are reordered, keyed so they can be followed across the
    /// rebuilds of `sync_tabs`: tab rows by view, the sidebar's other rows by node.
    pub fn moving_nodes(&self, ui: &Ui) -> Vec<(MoveKey, NodeId)> {
        let mut out: Vec<(MoveKey, NodeId)> =
            self.items.iter().map(|i| (MoveKey::Tab(i.view, i.surface), i.node)).collect();
        let lists = [self.sidebar_tabs, self.sidebar_other];
        for node in ui.children(self.sections) {
            if !lists.contains(&node) {
                out.push((MoveKey::Node(node), node));
            }
        }
        out
    }

    /// Lights the tab row under the pointer with a glass highlight (rows have no flat hover
    /// fill). Call every frame with the UI's hovered node.
    pub fn update_hover(&mut self, ui: &mut Ui, hovered: Option<NodeId>) {
        self.update_separators(ui, hovered);
        let row = hovered.filter(|&n| {
            self.items.iter().any(|i| i.node == n && i.surface == Surface::Sidebar)
                && ui.widget(n).is_some_and(|w| w.background != Background::Glass(GlassLevel::Thin))
        });
        if row == self.hovered_row {
            return;
        }
        if let Some(old) = self.hovered_row.take()
            && let Some(w) = ui.widget_mut(old)
            && w.background == Background::Glass(GlassLevel::Lens)
        {
            w.background = Background::None;
        }
        if let Some(new) = row
            && let Some(w) = ui.widget_mut(new)
        {
            w.background = Background::Glass(GlassLevel::Lens);
        }
        self.hovered_row = row;
    }

    /// Rebuilds the tab segments and sidebar rows from the tab model. `other` is the group
    /// not shown (listed in the sidebar only).
    pub fn sync_tabs(&mut self, ui: &mut Ui, tabs: &Tabs, other: &Tabs, favicons: &Favicons) {
        for pill in std::mem::take(&mut self.pills) {
            ui.remove(pill);
        }
        self.items.clear();
        self.hovered_row = None;
        self.tab_widths.pills.clear();
        self.tab_widths.separators.clear();
        self.tab_widths.separator_hover = None;
        let narrow = self.mode == Mode::Narrow;
        let separate = self.tab_bar == TabBarStyle::Separate;
        let mut order = Vec::with_capacity(tabs.len());
        let private = self.private_mode;
        ui.set_opacity(self.address, 1.0);
        for (i, tab) in tabs.all().iter().enumerate() {
            let active = i == tabs.active_index();
            if active && !separate {
                // Compact: the persistent address segment sits at the active tab's position.
                order.push(self.address);
                if !self.locked && !narrow {
                    let item = TabItem {
                        node: self.address,
                        view: tab.view,
                        private,
                        index: i,
                        surface: Surface::Bar,
                    };
                    self.items.push(item);
                }
                continue;
            }
            let parent = if separate { self.tab_strip } else { self.tab_capsule };
            let background = if active { Background::Glass(GlassLevel::Thin) } else { Background::None };
            let height = if separate { TAB_STRIP_HEIGHT - 2.0 * TAB_STRIP_PADDING } else { SEGMENT_HEIGHT };
            // Every tab is equally wide (see `animate_tab_widths`); pinned ones are just their
            // favicon.
            let width = if tab.pinned { self.tab_widths.pinned } else { self.tab_widths.width.value };
            let pill = ui.add(
                parent,
                Widget::container()
                    .with_action(ChromeAction::SelectTab(i).id())
                    .with_radius(CAPSULE)
                    .with_background(background)
                    .pressed_whole(),
                Style {
                    flex_grow: 0.0,
                    flex_shrink: if tab.pinned { 0.0 } else { 1.0 },
                    min_size: Size { width: length(TAB_HARD_MIN.min(width)), height: auto() },
                    size: Size { width: length(width), height: length(height) },
                    padding: if tab.pinned {
                        padding(0.0, 0.0, 0.0, 0.0)
                    } else {
                        padding(if active && separate { 6.0 } else { 14.0 }, 14.0, 0.0, 0.0)
                    },
                    justify_content: if separate || tab.pinned {
                        JustifyContent::CENTER
                    } else {
                        JustifyContent::START
                    },
                    ..row(9.0)
                },
            );
            if active && separate && !tab.pinned {
                close_dot(ui, pill, ChromeAction::CloseTab(i), 24.0);
            }
            favicon_node(ui, pill, &tab.url, 16.0, favicons);
            if !tab.pinned {
                let style = if active { TextStyle::BodyEmphasis } else { TextStyle::Callout };
                ui.add_label(pill, &tab.label(), style, shrinking_label());
            }
            if separate {
                // A hairline before every tab but the first (shown by `update_separators`).
                let separator = (!order.is_empty()).then(|| {
                    ui.add(
                        pill,
                        Widget::container()
                            .with_background(Background::Fill(Paint::Label, TAB_SEPARATOR_ALPHA)),
                        Style {
                            position: Position::Absolute,
                            inset: Rect {
                                left: length(-TAB_GAP / 2.0 - 0.5),
                                right: auto(),
                                top: length((height - TAB_SEPARATOR_HEIGHT) / 2.0),
                                bottom: auto(),
                            },
                            size: Size { width: length(1.0), height: length(TAB_SEPARATOR_HEIGHT) },
                            ..Default::default()
                        },
                    )
                });
                self.tab_widths.separators.push((pill, separator, active));
            }
            self.tab_widths.pills.push((pill, tab.pinned));
            ui.set_visible(pill, !narrow || separate);
            self.pills.push(pill);
            order.push(pill);
            if !self.locked && (separate || !narrow) {
                self.items.push(TabItem {
                    node: pill,
                    view: tab.view,
                    private,
                    index: i,
                    surface: Surface::Bar,
                });
            }
        }
        if separate {
            ui.set_children(self.tab_capsule, &[]);
            // The strip is also a plane for menus (tab context menus): their glass stays, after
            // the tabs.
            let menus = ui.children(self.tab_strip).into_iter().filter(|n| !order.contains(n));
            let children: Vec<NodeId> = order.iter().copied().chain(menus).collect();
            ui.set_children(self.tab_strip, &children);
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

        // Sidebar: each group's tabs are listed under its row: the normal ones under "N Tabs",
        // the private ones under "Private". Rows of the other group switch to it.
        let count = if self.private_mode { self.normal_count } else { tabs.len() };
        ui.set_text(self.tab_group_label, &format!("{count} {}", if count == 1 { "Tab" } else { "Tabs" }));
        if let Some(header) = ui.parent(self.tab_group_label) {
            let (normal, private) = if self.private_mode {
                (self.sidebar_other, self.sidebar_tabs)
            } else {
                (self.sidebar_tabs, self.sidebar_other)
            };
            let mut order: Vec<NodeId> =
                ui.children(self.sections).into_iter().filter(|&n| n != normal && n != private).collect();
            if let Some(at) = order.iter().position(|&n| n == header) {
                order.insert(at + 1, normal);
            }
            if let Some(at) = order.iter().position(|&n| n == self.private_row) {
                order.insert(at + 1, private);
            }
            ui.set_children(self.sections, &order);
        }
        ui.clear_children(self.sidebar_tabs);
        // Locked private tabs aren't listed at all (not even hidden): nothing to drag or read.
        let hide_current = private && (self.private_hidden || self.locked);
        let hide_other = !private && self.private_hidden;
        for (i, tab) in tabs.all().iter().enumerate() {
            if hide_current {
                break;
            }
            let active = i == tabs.active_index();
            let node =
                sidebar_tab_row(ui, self.sidebar_tabs, tab, active, ChromeAction::SelectTab(i), favicons);
            self.items.push(TabItem { node, view: tab.view, private, index: i, surface: Surface::Sidebar });
        }
        ui.clear_children(self.sidebar_other);
        for (i, tab) in other.all().iter().enumerate() {
            if hide_other {
                break;
            }
            let action = ChromeAction::SelectOtherTab(i);
            let node = sidebar_tab_row(ui, self.sidebar_other, tab, false, action, favicons);
            self.items.push(TabItem {
                node,
                view: tab.view,
                private: !private,
                index: i,
                surface: Surface::Sidebar,
            });
        }
        self.update_tab_lists(ui);
        ui.set_text(self.tab_count_label, &count.to_string());
        self.sync_active(ui, tabs);
    }

    /// Updates the active segment (address, back/forward, reload/stop, progress).
    pub fn sync_active(&mut self, ui: &mut Ui, tabs: &Tabs) {
        let Some(tab) = tabs.active() else { return };
        let home = crate::home::is_home_url(&tab.url);
        let host = match () {
            _ if self.locked => "Private Browsing".to_owned(),
            _ if home => crate::home::ADDRESS_PLACEHOLDER.to_owned(),
            _ => display_host(&tab.url),
        };
        ui.set_text(self.address_label, &host);
        // On the start page it reads like the field's placeholder: grey, the same words.
        if let Some(w) = ui.widget_mut(self.address_label) {
            w.set_paint(if home && !self.locked { Paint::Secondary } else { Paint::Label });
        }
        if ui.focused() != Some(self.address_field) {
            // The start page's address stays out of the field (typing starts fresh).
            ui.set_field_text(self.address_field, if home { "" } else { &tab.url });
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

/// How [`Chrome::moving_nodes`] names a node across rebuilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveKey {
    Tab(ViewId, Surface),
    Node(NodeId),
}

/// One tab in a sidebar list: the active one is a thin glass capsule with a small close button
/// beside its favicon; the others light up with glass under the pointer (`update_hover`).
fn sidebar_tab_row(
    ui: &mut Ui,
    parent: NodeId,
    tab: &Tab,
    active: bool,
    select: ChromeAction,
    favicons: &Favicons,
) -> NodeId {
    let background = if active { Background::Glass(GlassLevel::Thin) } else { Background::None };
    let row_node = ui.add(
        parent,
        Widget::container()
            .with_action(select.id())
            .with_radius(CAPSULE)
            .with_background(background)
            .without_feedback(),
        Style {
            size: Size { width: auto(), height: length(ROW_HEIGHT) },
            padding: padding(TAB_INDENT, 12.0, 0.0, 0.0),
            ..row(6.0)
        },
    );
    match select {
        ChromeAction::SelectTab(i) if active => {
            close_dot_sized(ui, row_node, ChromeAction::CloseTab(i), ROW_CLOSE_SLOT, ROW_CLOSE_DOT);
        }
        _ => {
            // The close button's room, so favicons and titles line up.
            ui.add(
                row_node,
                Widget::container(),
                Style { size: square(ROW_CLOSE_SLOT), flex_shrink: 0.0, ..row(0.0) },
            );
        }
    }
    favicon_node(ui, row_node, &tab.url, ROW_FAVICON, favicons);
    let style = if active { TextStyle::BodyEmphasis } else { TextStyle::Body };
    let label = ui.add_label(row_node, &tab.label(), style, shrinking_label());
    let mut layout = ui.style(label);
    layout.margin = Rect { left: length(6.0), right: length(0.0), top: length(0.0), bottom: length(0.0) };
    ui.set_style(label, layout);
    if tab.pinned {
        ui.add(row_node, Widget::icon(Icon::Pin, 14.0).with_paint(Paint::Secondary), Style::default());
    }
    row_node
}

impl BookmarkRow {
    /// Puts the bar at `width` (at most `span`), centred in the span starting at `left`.
    fn place(&mut self, ui: &mut Ui, left: f32, span: f32, width: f32) {
        let width = width.min(span);
        let placed = (left + (span - width) / 2.0, width);
        if (placed.0 - self.applied.0).abs() < 0.05 && (placed.1 - self.applied.1).abs() < 0.05 {
            return;
        }
        self.applied = placed;
        let mut style = ui.style(self.bar);
        style.inset.left = length(placed.0);
        style.size.width = length(placed.1);
        ui.set_style(self.bar, style);
    }

    fn build(ui: &mut Ui, root: NodeId) -> Self {
        let bar = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(BOOKMARKS_BAR_HEIGHT) },
                padding: padding(BOOKMARKS_BAR_PADDING, BOOKMARKS_BAR_PADDING, 0.0, 0.0),
                ..row(BOOKMARK_GAP)
            },
        );
        // Items that don't fit are hidden (`fit_bookmarks`); the clip only covers the frame
        // before they are measured.
        let list = ui.add(
            bar,
            Widget::container().with_clip(),
            Style {
                // Always its natural width, so its items measure true; the bar is sized to the
                // ones that fit and hides the rest.
                flex_grow: 0.0,
                flex_shrink: 0.0,
                flex_basis: auto(),
                min_size: Size { width: length(0.0), height: auto() },
                size: Size { width: auto(), height: length(BOOKMARK_ITEM_HEIGHT) },
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Hidden,
                },
                ..row(BOOKMARK_GAP)
            },
        );
        let hint = ui.add_label(
            list,
            "Bookmarks you add appear here",
            TextStyle::Footnote,
            Style { padding: padding(10.0, 10.0, 0.0, 0.0), ..Default::default() },
        );
        if let Some(w) = ui.widget_mut(hint) {
            w.set_paint(Paint::Secondary);
        }
        let glyph = Widget::icon(Icon::ChevronDown, 14.0).with_paint(Paint::Secondary);
        let (more, _) = icon_button(ui, bar, ChromeAction::BookmarksOverflow, glyph, BOOKMARK_ITEM_HEIGHT);
        ui.set_visible(more, false);
        ui.set_visible(bar, false);
        Self {
            bar,
            list,
            more,
            hint,
            items: Vec::new(),
            widths: None,
            fit: 0,
            signature: Vec::new(),
            shown: false,
            area: (0.0, 0.0),
            applied: (-1.0, -1.0),
        }
    }
}

impl Chrome {
    /// Springs the tabs' shared width towards what fits now (all tabs equally wide: the
    /// preferred width, or less when they don't fit), and gives it to every tab segment.
    fn animate_tab_widths(&mut self, ui: &mut Ui, dt: f32) {
        let separate = self.tab_bar == TabBarStyle::Separate;
        let (container, inner_padding, reserved) = if separate {
            (self.tab_strip, TAB_STRIP_PADDING, 0.0)
        } else {
            // The address segment keeps at least its minimum.
            (self.tab_capsule, TAB_CAPSULE_PADDING, ACTIVE_SEGMENT_MIN + TAB_GAP)
        };
        // The separate strip shrinks to its tabs, so it measures its span, not itself.
        let outer = if separate { self.tab_widths.strip_area.1 } else { ui.frame(container).size.x };
        if outer <= 1.0 || !ui.is_visible(container) {
            return;
        }
        let widths = &mut self.tab_widths;
        let pinned = widths.pills.iter().filter(|(_, pinned)| *pinned).count();
        let flexible = widths.pills.len() - pinned;
        let available = outer - 2.0 * inner_padding - reserved - pinned as f32 * (widths.pinned + TAB_GAP);
        let target = equal_tab_width(available, flexible, TAB_GAP, widths.preferred, widths.min);
        widths.width.target = target;
        if !widths.ready || widths.reduce_motion {
            widths.width.snap(target);
            widths.ready = true;
        } else {
            widths.width.step(dt);
        }
        let width = widths.width.value.max(0.0);
        if separate {
            // As wide as its tabs (and their gaps), centred in its span.
            let count = widths.pills.len();
            let gaps = count.saturating_sub(1) as f32 * TAB_GAP;
            let tabs = flexible as f32 * width + pinned as f32 * widths.pinned + gaps;
            let (left, span) = widths.strip_area;
            let strip_width = (tabs + 2.0 * inner_padding).min(span);
            let placed = (left + (span - strip_width) / 2.0, strip_width);
            if (placed.0 - widths.strip_applied.0).abs() > 0.05
                || (placed.1 - widths.strip_applied.1).abs() > 0.05
            {
                widths.strip_applied = placed;
                let mut style = ui.style(self.tab_strip);
                style.inset.left = length(placed.0);
                style.size.width = length(placed.1);
                ui.set_style(self.tab_strip, style);
            }
        }
        if (width - widths.applied).abs() < 0.05 {
            return;
        }
        widths.applied = width;
        for &(pill, pinned) in &widths.pills {
            if pinned {
                continue;
            }
            let mut style = ui.style(pill);
            style.size.width = length(width);
            style.min_size.width = length(TAB_HARD_MIN.min(width));
            ui.set_style(pill, style);
        }
    }

    /// The separate tab bar's hairlines: between two tabs, except next to the open tab and
    /// the one under the pointer (their highlight separates them already).
    fn update_separators(&mut self, ui: &mut Ui, hovered: Option<NodeId>) {
        let widths = &mut self.tab_widths;
        if widths.separator_hover == Some(hovered) {
            return;
        }
        widths.separator_hover = Some(hovered);
        // The hovered tab (or its close button).
        let hovered_pill = hovered.and_then(|n| {
            widths.separators.iter().position(|&(pill, _, _)| pill == n || ui.parent(n) == Some(pill))
        });
        let separators = &widths.separators;
        let lit =
            |i: usize| separators.get(i).is_some_and(|&(_, _, active)| active) || hovered_pill == Some(i);
        for (i, &(_, separator, _)) in separators.iter().enumerate() {
            if let Some(separator) = separator {
                ui.set_visible(separator, i > 0 && !lit(i) && !lit(i - 1));
            }
        }
    }

    /// The tab item at `node` or one of its parents (a click on a tab's close or reload button
    /// is a click on the tab), if any.
    pub fn tab_at(&self, ui: &Ui, node: NodeId) -> Option<TabItem> {
        let mut at = Some(node);
        while let Some(n) = at {
            if let Some(item) = self.tab_item(n) {
                return Some(item);
            }
            at = ui.parent(n);
        }
        None
    }

    /// The plane a tab item lies in (its context menu flows out of it there).
    pub fn tab_plane(&self, item: &TabItem) -> NodeId {
        match item.surface {
            Surface::Sidebar => self.sidebar,
            Surface::Bar if self.tab_bar == TabBarStyle::Separate => self.tab_strip,
            Surface::Bar => self.bar,
        }
    }

    /// The separate tab bar's row (a plane of its own).
    pub fn tab_strip_node(&self) -> NodeId {
        self.tab_strip
    }

    // ---- bookmarks bar ----

    fn layout_bookmarks_bar(&mut self, ui: &mut Ui, config: &Config, left: f32, window_width: f32) {
        let l = &config.layout;
        let m = l.margin;
        let strip = if l.tab_bar == TabBarStyle::Separate { TAB_STRIP_HEIGHT + TAB_STRIP_GAP } else { 0.0 };
        let bar = self.bookmarks.bar;
        let mut style = ui.style(bar);
        // Left and width follow the items (see `fit_bookmarks`).
        style.inset.top = length(m + l.bar_height + strip + TAB_STRIP_GAP);
        style.inset.bottom = auto();
        style.inset.right = auto();
        ui.set_style(bar, style);
        let area = (left, (window_width - left - m).max(0.0));
        if area != self.bookmarks.area {
            self.bookmarks.area = area;
            self.bookmarks.applied = (-1.0, -1.0);
            self.bookmarks.fit = usize::MAX;
        }
        ui.set_visible(bar, self.bookmarks.shown && !self.page_fullscreen);
    }

    /// The height the bookmarks bar takes from the page (with its gap), 0 while hidden.
    fn bookmarks_room(&self) -> f32 {
        if self.bookmarks.shown && !self.page_fullscreen { BOOKMARKS_BAR_HEIGHT + TAB_STRIP_GAP } else { 0.0 }
    }

    /// Shows or hides the bookmarks bar. Returns whether that changed (the page then needs a
    /// new size: it never lies under the bar).
    pub fn set_bookmarks_bar_shown(&mut self, ui: &mut Ui, shown: bool) -> bool {
        if self.bookmarks.shown == shown {
            return false;
        }
        self.bookmarks.shown = shown;
        ui.set_visible(self.bookmarks.bar, shown && !self.page_fullscreen);
        true
    }

    /// The bookmarks bar's plane (its menus lie in it).
    pub fn bookmarks_bar_node(&self) -> NodeId {
        self.bookmarks.bar
    }

    /// Fills the bookmarks bar with `items` (url, title); rebuilt only when they (or their
    /// favicons) changed.
    pub fn set_bookmarks(&mut self, ui: &mut Ui, items: &[(String, String)], favicons: &Favicons) {
        let signature: Vec<(String, String, bool)> = items
            .iter()
            .map(|(url, title)| (url.clone(), title.clone(), favicons.get(url).is_some()))
            .collect();
        if signature == self.bookmarks.signature {
            return;
        }
        self.bookmarks.signature = signature;
        for node in std::mem::take(&mut self.bookmarks.items) {
            ui.remove(node);
        }
        for (i, (url, title)) in items.iter().enumerate() {
            let item = ui.add(
                self.bookmarks.list,
                Widget::container().with_action(ChromeAction::OpenBookmark(i).id()).with_radius(CAPSULE),
                Style {
                    flex_shrink: 0.0,
                    max_size: Size { width: length(BOOKMARK_MAX_WIDTH), height: auto() },
                    size: Size { width: auto(), height: length(BOOKMARK_ITEM_HEIGHT) },
                    padding: padding(8.0, 10.0, 0.0, 0.0),
                    ..row(6.0)
                },
            );
            favicon_node(ui, item, url, BOOKMARK_FAVICON, favicons);
            let text = if title.trim().is_empty() { display_host(url) } else { title.clone() };
            ui.add_label(item, &text, TextStyle::Footnote, shrinking_label());
            self.bookmarks.items.push(item);
        }
        ui.set_visible(self.bookmarks.hint, items.is_empty());
        ui.set_visible(self.bookmarks.more, false);
        self.bookmarks.widths = None;
        self.bookmarks.fit = items.len();
    }

    /// Hides the bookmarks that don't fit (they are listed in the chevron's menu). Measures
    /// the items once after they were laid out.
    fn fit_bookmarks(&mut self, ui: &mut Ui) {
        let b = &mut self.bookmarks;
        if !b.shown || !ui.is_visible(b.bar) {
            return;
        }
        // The bar spans the whole width.
        let (left, span) = b.area;
        b.place(ui, left, span, span);
        if b.items.is_empty() {
            return;
        }
        if b.widths.is_none() {
            let widths: Vec<f32> = b.items.iter().map(|&n| ui.frame(n).size.x).collect();
            if widths.iter().any(|&w| w <= 0.0) {
                // Not laid out yet.
                return;
            }
            b.widths = Some(widths);
            b.fit = usize::MAX;
        }
        let Some(widths) = b.widths.clone() else { return };
        let available = span - 2.0 * BOOKMARKS_BAR_PADDING;
        let fit = bookmarks_that_fit(&widths, BOOKMARK_GAP, available, BOOKMARK_ITEM_HEIGHT);
        if fit == b.fit {
            return;
        }
        b.fit = fit;
        for (i, &node) in b.items.iter().enumerate() {
            ui.set_visible(node, i < fit);
        }
        ui.set_visible(b.more, fit < b.items.len());
    }

    /// The bookmarks (indices) that don't fit into the bar.
    pub fn bookmarks_overflow(&self) -> std::ops::Range<usize> {
        self.bookmarks.fit.min(self.bookmarks.items.len())..self.bookmarks.items.len()
    }

    /// The bookmarks bar's chevron (logical px), where the overflow menu flows out of.
    pub fn bookmarks_more_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.bookmarks.more)
    }

    /// Bookmark `i` of the bar (logical px).
    pub fn bookmark_frame(&self, ui: &Ui, i: usize) -> Option<Frame> {
        self.bookmarks.items.get(i).map(|&n| ui.frame(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_share_the_width_equally() {
        // Few tabs: each at the preferred width (left-aligned, not stretched).
        assert_eq!(equal_tab_width(1000.0, 2, 4.0, 220.0, 72.0), 220.0);
        // Many: they shrink equally to fill the row …
        let w = equal_tab_width(1000.0, 8, 4.0, 220.0, 72.0);
        assert!((w * 8.0 + 7.0 * 4.0 - 1000.0).abs() < 0.01, "{w}");
        // … down to the minimum.
        assert_eq!(equal_tab_width(1000.0, 40, 4.0, 220.0, 72.0), 72.0);
        assert_eq!(equal_tab_width(1000.0, 0, 4.0, 220.0, 72.0), 220.0);
    }

    #[test]
    fn bookmarks_overflow_behind_the_chevron() {
        let widths = [100.0, 100.0, 100.0];
        assert_eq!(bookmarks_that_fit(&widths, 2.0, 304.0, 26.0), 3);
        // One doesn't fit: room for the chevron is kept.
        assert_eq!(bookmarks_that_fit(&widths, 2.0, 300.0, 26.0), 2);
        assert_eq!(bookmarks_that_fit(&widths, 2.0, 150.0, 26.0), 1);
        assert_eq!(bookmarks_that_fit(&widths, 2.0, 50.0, 26.0), 0);
        assert_eq!(bookmarks_that_fit(&[], 2.0, 50.0, 26.0), 0);
    }

    #[test]
    fn bookmark_actions_round_trip() {
        for action in
            [ChromeAction::OpenBookmark(0), ChromeAction::OpenBookmark(42), ChromeAction::BookmarksOverflow]
        {
            assert_eq!(ChromeAction::from_id(action.id()), Some(action));
        }
    }
}
