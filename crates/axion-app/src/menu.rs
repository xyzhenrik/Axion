//! A glass menu that flows out of its button (see [`Popover`]): the page menu (≡) out of the
//! address field in the toolbar, and the account menu rising from the account button at the
//! bottom of the sidebar. Like iPadOS 26 menus it is one glass panel with the rows directly on
//! it: icon and label, a value or checkmark at the end, grey section captions and hairline
//! separators. A row can open a submenu: a second glass panel beside the menu, joined to it,
//! while the row stays highlighted. Clicking outside or pressing Escape closes it.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::drip::Phase;
use crate::popover::Popover;

/// The page menu's rows.
pub const PAGE_MENU_BASE: u32 = 6000;
/// The account menu's rows (it holds fewer than `MAX_ITEMS`).
pub const ACCOUNT_MENU_BASE: u32 = 6050;
/// Clickable things a menu can have, its submenus' included (the next menu's ids start after).
pub const MAX_ITEMS: usize = 50;
const WIDTH: f32 = 280.0;
/// Characters a row's label and its detail (shortcut, value) may take together at `WIDTH`.
const MAX_LABEL_AND_DETAIL: usize = 24;
const SUBMENU_WIDTH: f32 = 200.0;
const ROW_HEIGHT: f32 = 44.0;
const CAPTION_HEIGHT: f32 = 30.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const PADDING: f32 = 6.0;
const ROW_INSET: f32 = 14.0;
const ICON: f32 = 19.0;
const GAP_TO_ANCHOR: f32 = 14.0;
/// A menu below its button reaches this far past the button's right edge.
const BEYOND_ANCHOR: f32 = 12.0;
/// Between a menu and its submenu (the bridge joins them across it).
const SUBMENU_GAP: f32 = 12.0;
/// Kept free at the window's edges.
const WINDOW_MARGIN: f32 = 8.0;
const RADIUS: f32 = 22.0;
/// The plate's distance from the glass's edge, and the blur of its own edge (logical px).
const PLATE_INSET: f32 = 14.0;
const PLATE_BLUR: f32 = 26.0;
const AVATAR: f32 = 26.0;
/// The row whose submenu is open stays highlighted (label colour at this opacity).
const OPEN_ROW_HIGHLIGHT: f32 = 0.08;

/// What a menu entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    AddBookmark,
    RemoveBookmark,
    CopyLink,
    /// The zoom row: opens its submenu (never run itself; names its value for `set_text`).
    Zoom,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    HardReload,
    FindOnPage,
    Settings,
    /// Switch to the account at this index of the list.
    SwitchAccount(usize),
    AddAccount,
    AccountSettings,
    /// Lock private tabs and Passwords now.
    LockAccount,
    /// A tab's context menu (see `context_menu`).
    Tab(crate::context_menu::TabCommand),
    /// A bookmark's context menu, or the bookmarks bar's overflow menu.
    Bookmark(crate::context_menu::BookmarkCommand),
}

/// Where a menu opens relative to its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Below, right-aligned (the page menu).
    Below,
    /// Above, as wide as the button (the account menu).
    Above,
    /// Below, left-aligned with the button (a tab's or a bookmark's context menu).
    BelowStart,
    /// Over the button, left-aligned: the menu grows out of it and covers it (a row in the
    /// sidebar, whose glass is another material than the menu's and so can't be bridged to
    /// it). It grows upward from the button's bottom edge when it doesn't fit below.
    Over,
}

/// A compact button in a row of buttons (the zoom submenu's − 100% +).
pub struct MenuControl {
    pub icon: Option<Icon>,
    pub text: Option<String>,
    pub action: MenuAction,
}

impl MenuControl {
    pub fn icon(icon: Icon, action: MenuAction) -> Self {
        Self { icon: Some(icon), text: None, action }
    }

    pub fn text(text: String, action: MenuAction) -> Self {
        Self { icon: None, text: Some(text), action }
    }
}

pub struct MenuItem {
    pub icon: Option<Icon>,
    /// An account's avatar (initials and colour) instead of an icon.
    pub avatar: Option<(String, Color)>,
    pub label: String,
    /// Secondary text at the end of the row (e.g. the current zoom).
    pub detail: Option<String>,
    /// A checkmark at the end of the row (the current account).
    pub checked: bool,
    /// A hairline above the row.
    pub separated: bool,
    /// A grey section caption above the row.
    pub caption: Option<String>,
    /// Rows of a submenu that opens beside the menu (the row gets a chevron).
    pub submenu: Vec<MenuItem>,
    /// Instead of icon and label: a row of compact buttons. They keep the menu open.
    pub controls: Vec<MenuControl>,
    pub action: MenuAction,
}

impl MenuItem {
    pub fn new(icon: Icon, label: &str, action: MenuAction) -> Self {
        Self {
            icon: Some(icon),
            avatar: None,
            label: label.to_owned(),
            detail: None,
            checked: false,
            separated: false,
            caption: None,
            submenu: Vec::new(),
            controls: Vec::new(),
            action,
        }
    }

    /// A row for an account: its avatar and name.
    pub fn account(initials: String, color: Color, name: &str, checked: bool, action: MenuAction) -> Self {
        Self { icon: None, avatar: Some((initials, color)), checked, ..Self::new(Icon::Person, name, action) }
    }

    /// A row of compact buttons.
    pub fn controls(controls: Vec<MenuControl>) -> Self {
        let action = controls.first().map_or(MenuAction::Settings, |c| c.action);
        Self { icon: None, controls, ..Self::new(Icon::More, "", action) }
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn with_submenu(mut self, items: Vec<MenuItem>) -> Self {
        self.submenu = items;
        self
    }

    pub fn with_caption(mut self, caption: &str) -> Self {
        self.caption = Some(caption.to_owned());
        self
    }

    pub fn separated(mut self) -> Self {
        self.separated = true;
        self
    }

    fn height(&self, first: bool) -> f32 {
        let mut h = ROW_HEIGHT;
        if self.separated && !first {
            h += SEPARATOR_HEIGHT;
        }
        if self.caption.is_some() {
            h += CAPTION_HEIGHT;
        }
        h
    }
}

/// The soft plate under a menu's rows (see [`Background::Soft`]): inset from the glass's edge
/// so its bezel still refracts, and fading out towards it and towards the neck the menu flows
/// out of, so there is no hard edge anywhere.
fn plate(ui: &mut Ui, panel: NodeId, config: &Config) {
    let inset = length(PLATE_INSET);
    ui.add(
        panel,
        Widget::container().with_background(Background::Soft(
            Paint::Ground,
            config.glass.menu_plate_opacity.clamp(0.0, 1.0),
            PLATE_BLUR,
        )),
        Style {
            position: Position::Absolute,
            inset: Rect { left: inset, right: inset, top: inset, bottom: inset },
            ..Default::default()
        },
    );
}

/// What a click in a menu means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuClick {
    pub action: MenuAction,
    /// The menu stays open (a control in a row of buttons).
    pub keep_open: bool,
}

#[derive(Clone, Copy)]
enum Slot {
    Run(MenuClick),
    /// Opens the submenu of the parent row with this index.
    Submenu(usize),
}

/// An open submenu.
struct Open {
    /// The parent row's index and node (highlighted while open).
    row: usize,
    node: NodeId,
}

pub struct Menu {
    popover: Popover,
    sub: Popover,
    base: u32,
    placement: Placement,
    /// Clickable things, by `ActionId - base`.
    slots: Vec<Slot>,
    /// Parent rows: their node and submenu items.
    rows: Vec<(NodeId, Vec<MenuItem>)>,
    /// Labels updated in place (`set_text`), by the action they show.
    labels: Vec<(MenuAction, NodeId)>,
    /// Slots and labels of the parent menu (the submenu's follow).
    parent_slots: usize,
    parent_labels: usize,
    open_sub: Option<Open>,
    window: Vec2,
}

impl Menu {
    /// `parent` is the plane the menu's glass merges with (the toolbar, the sidebar); row
    /// actions are `base + index`.
    pub fn build(ui: &mut Ui, parent: NodeId, base: u32, placement: Placement) -> Self {
        let column = Style {
            size: Size { width: length(WIDTH), height: auto() },
            padding: uniform(PADDING),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        };
        let popover = Popover::build(ui, parent, RADIUS, column.clone());
        let sub = Popover::build(ui, parent, RADIUS, column);
        Self {
            popover,
            sub,
            base,
            placement,
            slots: Vec::new(),
            rows: Vec::new(),
            labels: Vec::new(),
            parent_slots: 0,
            parent_labels: 0,
            open_sub: None,
            window: Vec2::ZERO,
        }
    }

    /// Where the next `open` places the menu.
    pub fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
    }

    /// Open (or opening): clicks on rows count, outside clicks close it.
    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    /// Where the menu flows out of (its button, logical px).
    pub fn anchor(&self) -> Frame {
        self.popover.anchor()
    }

    /// See [`Popover::set_covered`].
    pub fn set_covered(&mut self, ui: &mut Ui, node: Option<NodeId>) {
        self.popover.set_covered(ui, node);
    }

    /// See [`Popover::set_covered_nodes`].
    pub fn set_covered_nodes(&mut self, ui: &mut Ui, nodes: Vec<NodeId>) {
        self.popover.set_covered_nodes(ui, nodes);
    }

    /// Opens next to `anchor` (logical px, see [`Placement`]), kept inside the window.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, window: Vec2, items: Vec<MenuItem>) {
        self.close_submenu(ui, config);
        let root = self.popover.root();
        ui.clear_children(root);
        self.slots.clear();
        self.rows.clear();
        self.labels.clear();
        self.window = window;
        plate(ui, root, config);
        let mut height = 2.0 * PADDING;
        for (i, mut item) in items.into_iter().enumerate() {
            if self.slots.len() >= MAX_ITEMS {
                break;
            }
            // A shortcut that would squeeze the label into an ellipsis is left out.
            if item
                .detail
                .as_ref()
                .is_some_and(|d| item.label.chars().count() + d.chars().count() > MAX_LABEL_AND_DETAIL)
            {
                item.detail = None;
            }
            height += item.height(i == 0);
            let node = self.add_item(ui, root, i == 0, &item, Slot::Submenu(self.rows.len()));
            self.rows.push((node, item.submenu));
        }
        self.parent_slots = self.slots.len();
        self.parent_labels = self.labels.len();
        let width = match self.placement {
            Placement::Below | Placement::BelowStart => WIDTH,
            // As wide as the row it covers, so none of the row pokes out at the sides.
            Placement::Over => anchor.size.x.max(WIDTH),
            Placement::Above => anchor.size.x.max(WIDTH * 0.75),
        };
        let max_left = (window.x - width - WINDOW_MARGIN).max(WINDOW_MARGIN);
        let target = match self.placement {
            Placement::Below => {
                // A little past the button, so the bridge reaches as far as the address field does.
                let right = anchor.origin.x + anchor.size.x + BEYOND_ANCHOR;
                let left = (right - width).clamp(WINDOW_MARGIN, max_left);
                Frame::new(left, anchor.origin.y + anchor.size.y + GAP_TO_ANCHOR, width, height)
            }
            Placement::BelowStart => {
                let left = anchor.origin.x.clamp(WINDOW_MARGIN, max_left);
                let below = anchor.origin.y + anchor.size.y + GAP_TO_ANCHOR;
                let above = anchor.origin.y - GAP_TO_ANCHOR - height;
                // Below the anchor; flips above it when it would run off the window's bottom
                // and fits there; else it is pushed up as far as it must.
                let top = if below + height + WINDOW_MARGIN <= window.y || above < WINDOW_MARGIN {
                    let max_top = (window.y - height - WINDOW_MARGIN).max(WINDOW_MARGIN);
                    below.min(max_top)
                } else {
                    above
                };
                Frame::new(left, top, width, height)
            }
            Placement::Over => {
                let left = anchor.origin.x.clamp(WINDOW_MARGIN, max_left);
                let bottom_up = anchor.origin.y + anchor.size.y - height;
                let top = if anchor.origin.y + height + WINDOW_MARGIN <= window.y || bottom_up < WINDOW_MARGIN
                {
                    anchor.origin.y.min((window.y - height - WINDOW_MARGIN).max(WINDOW_MARGIN))
                } else {
                    bottom_up
                };
                Frame::new(left, top, width, height)
            }
            Placement::Above => {
                let left = anchor.origin.x.clamp(WINDOW_MARGIN, max_left);
                let top = (anchor.origin.y - GAP_TO_ANCHOR - height).max(WINDOW_MARGIN);
                Frame::new(left, top, width, height)
            }
        };
        self.popover.open(ui, config, anchor, target);
    }

    /// Adds one entry (caption, separator, row) to `panel`; returns the row.
    fn add_item(
        &mut self,
        ui: &mut Ui,
        panel: NodeId,
        first: bool,
        item: &MenuItem,
        submenu: Slot,
    ) -> NodeId {
        if let Some(caption) = &item.caption {
            let line = ui.add(
                panel,
                Widget::container(),
                Style {
                    size: Size { width: auto(), height: length(CAPTION_HEIGHT) },
                    flex_shrink: 0.0,
                    align_items: AlignItems::END,
                    padding: Rect {
                        left: length(ROW_INSET),
                        right: length(ROW_INSET),
                        top: length(0.0),
                        bottom: length(2.0),
                    },
                    ..Default::default()
                },
            );
            let label = ui.add_label(line, caption, TextStyle::Footnote, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
        }
        if item.separated && !first {
            separator(ui, panel);
        }
        if !item.controls.is_empty() {
            return self.add_controls(ui, panel, &item.controls);
        }
        let slot = if item.submenu.is_empty() {
            Slot::Run(MenuClick { action: item.action, keep_open: false })
        } else {
            submenu
        };
        let row = ui.add(
            panel,
            Widget::container().with_action(self.slot(slot)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(ROW_HEIGHT) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(12.0), height: length(0.0) },
                padding: Rect {
                    left: length(ROW_INSET),
                    right: length(ROW_INSET),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        match (&item.avatar, item.icon) {
            (Some((initials, color)), _) => {
                // Shifted left a little, so its centre lines up with the icons'.
                let avatar = crate::settings::avatar(ui, row, initials, *color, AVATAR);
                let mut style = ui.style(avatar);
                style.margin =
                    Rect { left: length(-3.5), right: length(-3.5), top: length(0.0), bottom: length(0.0) };
                ui.set_style(avatar, style);
            }
            (None, Some(icon)) => {
                ui.add(row, Widget::icon(icon, ICON), Style::default());
            }
            (None, None) => {}
        }
        ui.add_label(
            row,
            &item.label,
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        if let Some(detail) = &item.detail {
            let label = ui.add_label(row, detail, TextStyle::Body, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            self.labels.push((item.action, label));
        }
        if item.checked {
            ui.add(row, Widget::icon(Icon::Check, 17.0).with_paint(Paint::Accent), Style::default());
        }
        if !item.submenu.is_empty() {
            ui.add(
                row,
                Widget::icon(Icon::ChevronRight, 14.0).with_paint(Paint::Secondary),
                Style::default(),
            );
        }
        row
    }

    /// A row of equally wide compact buttons.
    fn add_controls(&mut self, ui: &mut Ui, panel: NodeId, controls: &[MenuControl]) -> NodeId {
        let row = ui.add(
            panel,
            Widget::container(),
            Style {
                size: Size { width: auto(), height: length(ROW_HEIGHT) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(2.0), height: length(0.0) },
                ..Default::default()
            },
        );
        for control in controls {
            let id = self.slot(Slot::Run(MenuClick { action: control.action, keep_open: true }));
            let button = ui.add(
                row,
                Widget::container().with_action(id).with_radius(CAPSULE),
                Style {
                    flex_grow: 1.0,
                    flex_basis: length(0.0),
                    size: Size { width: auto(), height: length(ROW_HEIGHT) },
                    justify_content: JustifyContent::CENTER,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            );
            if let Some(icon) = control.icon {
                ui.add(button, Widget::icon(icon, ICON), Style::default());
            }
            if let Some(text) = &control.text {
                let label = ui.add_label(button, text, TextStyle::BodyEmphasis, Style::default());
                self.labels.push((control.action, label));
            }
        }
        row
    }

    fn slot(&mut self, slot: Slot) -> ActionId {
        self.slots.push(slot);
        ActionId(self.base + self.slots.len() as u32 - 1)
    }

    /// Opens the submenu of parent row `row` beside the menu (or keeps it open).
    fn open_submenu(&mut self, ui: &mut Ui, config: &Config, row: usize) {
        if self.open_sub.as_ref().is_some_and(|o| o.row == row) {
            return;
        }
        self.close_submenu(ui, config);
        let Some((node, items)) = self.rows.get_mut(row) else { return };
        let node = *node;
        let items = std::mem::take(items);
        let panel = self.sub.root();
        ui.clear_children(panel);
        plate(ui, panel, config);
        self.slots.truncate(self.parent_slots);
        self.labels.truncate(self.parent_labels);
        let mut height = 2.0 * PADDING;
        for (i, item) in items.iter().enumerate() {
            height += item.height(i == 0);
            self.add_item(
                ui,
                panel,
                i == 0,
                item,
                Slot::Run(MenuClick { action: item.action, keep_open: false }),
            );
        }
        if let Some((_, slot)) = self.rows.get_mut(row) {
            *slot = items;
        }
        let menu = ui.frame(self.popover.root());
        let row_frame = ui.frame(node);
        let fits_right =
            menu.origin.x + menu.size.x + SUBMENU_GAP + SUBMENU_WIDTH + WINDOW_MARGIN <= self.window.x;
        let (left, anchor_x) = if fits_right {
            (menu.origin.x + menu.size.x + SUBMENU_GAP, menu.origin.x + menu.size.x - ROW_HEIGHT)
        } else {
            (menu.origin.x - SUBMENU_GAP - SUBMENU_WIDTH, menu.origin.x)
        };
        let max_top = (self.window.y - height - WINDOW_MARGIN).max(WINDOW_MARGIN);
        let top = (row_frame.origin.y - PADDING).clamp(WINDOW_MARGIN, max_top);
        // It flows out of the menu's edge beside the row.
        let anchor = Frame::new(anchor_x, row_frame.origin.y, ROW_HEIGHT, ROW_HEIGHT);
        let target = Frame::new(left.max(WINDOW_MARGIN), top, SUBMENU_WIDTH, height);
        self.sub.open(ui, config, anchor, target);
        if let Some(w) = ui.widget_mut(node) {
            w.background = Background::Fill(Paint::Label, OPEN_ROW_HIGHLIGHT);
        }
        self.open_sub = Some(Open { row, node });
    }

    fn close_submenu(&mut self, ui: &mut Ui, config: &Config) {
        if let Some(open) = self.open_sub.take() {
            if let Some(w) = ui.widget_mut(open.node) {
                w.background = Background::None;
            }
            self.slots.truncate(self.parent_slots);
            self.labels.truncate(self.parent_labels);
            self.sub.close(ui, config);
        }
    }

    /// Opens the submenu of the row under the pointer, or closes it when the pointer moved to
    /// another row; call every frame.
    pub fn track_hover(&mut self, ui: &mut Ui, config: &Config) {
        if !self.popover.is_open() || self.popover.phase() == Phase::Opening {
            return;
        }
        let Some(hovered) = ui.hovered() else { return };
        let Some(row) = self.rows.iter().position(|(n, _)| *n == hovered) else { return };
        if self.rows[row].1.is_empty() {
            self.close_submenu(ui, config);
        } else {
            self.open_submenu(ui, config, row);
        }
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        // The submenu flows back with the menu, into its button.
        if self.open_sub.is_some() {
            self.sub.close_into(ui, config, self.popover.anchor());
        }
        self.close_submenu(ui, config);
        self.slots.clear();
        self.popover.close(ui, config);
    }

    /// Advances the drips; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
        self.sub.animate(ui, dt);
    }

    /// Whether `id` is one of this menu's rows or controls.
    pub fn owns(&self, id: ActionId) -> bool {
        id.0.checked_sub(self.base).is_some_and(|i| (i as usize) < self.slots.len())
    }

    /// Handles a click on one of the menu's rows: opens a submenu (`None`) or says what to do.
    pub fn click(&mut self, ui: &mut Ui, config: &Config, id: ActionId) -> Option<MenuClick> {
        let index = id.0.checked_sub(self.base)? as usize;
        match *self.slots.get(index)? {
            Slot::Run(click) => Some(click),
            Slot::Submenu(row) => {
                self.open_submenu(ui, config, row);
                None
            }
        }
    }

    /// Updates the text showing `action`'s value (a row's detail, a control's label).
    pub fn set_text(&self, ui: &mut Ui, action: MenuAction, text: &str) {
        for (a, node) in &self.labels {
            if *a == action {
                ui.set_text(*node, text);
            }
        }
    }

    /// Whether a point (logical px) is inside the open menu or its submenu.
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p) || self.sub.contains(ui, p)
    }

    /// Capture aid: opens the first submenu as if its row was hovered.
    pub fn open_first_submenu(&mut self, ui: &mut Ui, config: &Config) {
        if let Some(row) = self.rows.iter().position(|(_, items)| !items.is_empty()) {
            self.open_submenu(ui, config, row);
        }
    }
}

/// A hairline between groups of rows.
fn separator(ui: &mut Ui, panel: NodeId) {
    let line = ui.add(
        panel,
        Widget::container(),
        Style {
            size: Size { width: auto(), height: length(SEPARATOR_HEIGHT) },
            padding: Rect {
                left: length(ROW_INSET),
                right: length(ROW_INSET),
                top: length(SEPARATOR_HEIGHT / 2.0),
                bottom: length(0.0),
            },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    ui.add(
        line,
        Widget::container().with_background(Background::Fill(Paint::Hairline, 1.0)),
        Style { flex_grow: 1.0, size: Size { width: auto(), height: length(1.0) }, ..Default::default() },
    );
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

/// Positions an absolute child of `parent` at `target` (absolute logical px).
pub fn place(ui: &mut Ui, node: NodeId, parent: NodeId, target: Frame) {
    let origin = target.origin - ui.frame(parent).origin;
    let mut style = ui.style(node);
    style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
    style.size = Size { width: length(target.size.x), height: length(target.size.y) };
    ui.set_style(node, style);
}
