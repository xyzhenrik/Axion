//! A glass popover menu that drips out of its button: the page menu (≡) in the toolbar's
//! plane, so it visibly flows out of the bar, and the account menu that rises from the
//! account button at the bottom of the sidebar. Clicking outside or pressing Escape closes it.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::drip::{Drip, Phase};

/// The page menu's rows.
pub const PAGE_MENU_BASE: u32 = 6000;
/// The account menu's rows (it holds fewer than `MAX_ITEMS`).
pub const ACCOUNT_MENU_BASE: u32 = 6050;
/// Rows a menu can have (the next menu's ids start after them).
pub const MAX_ITEMS: usize = 50;
const WIDTH: f32 = 280.0;
const ROW_HEIGHT: f32 = 44.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const PADDING: f32 = 6.0;
const GAP_TO_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 22.0;
const AVATAR: f32 = 26.0;
/// The frosted plate under the rows keeps text legible over any page.
const PLATE_OPACITY: f32 = 0.55;

/// What a menu entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    AddBookmark,
    RemoveBookmark,
    CopyLink,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    HardReload,
    Settings,
    /// Switch to the account at this index of the list.
    SwitchAccount(usize),
    AddAccount,
    AccountSettings,
    /// Lock private tabs and Passwords now.
    LockAccount,
}

/// Where a menu opens relative to its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Below, right-aligned (the page menu).
    Below,
    /// Above, as wide as the button (the account menu).
    Above,
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
            action,
        }
    }

    /// A row for an account: its avatar and name.
    pub fn account(initials: String, color: Color, name: &str, checked: bool, action: MenuAction) -> Self {
        Self { icon: None, avatar: Some((initials, color)), checked, ..Self::new(Icon::Person, name, action) }
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn separated(mut self) -> Self {
        self.separated = true;
        self
    }
}

pub struct Menu {
    parent: NodeId,
    root: NodeId,
    plate: NodeId,
    actions: Vec<MenuAction>,
    drip: Drip,
    base: u32,
    placement: Placement,
}

impl Menu {
    /// `parent` is the plane the menu's glass merges with (the toolbar, the sidebar); row
    /// actions are `base + index`.
    pub fn build(ui: &mut Ui, parent: NodeId, base: u32, placement: Placement) -> Self {
        let root = ui.add(
            parent,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: length(WIDTH), height: auto() },
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(PADDING),
                    bottom: length(PADDING),
                },
                ..Default::default()
            },
        );
        let plate = ui.add(
            root,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, PLATE_OPACITY))
                .with_radius(RADIUS - PADDING),
            Style { flex_grow: 1.0, flex_direction: FlexDirection::Column, ..Default::default() },
        );
        ui.set_visible(root, false);
        Self { parent, root, plate, actions: Vec::new(), drip: Drip::new(), base, placement }
    }

    /// Open (or opening): clicks on rows count, outside clicks close it.
    pub fn is_open(&self) -> bool {
        matches!(self.drip.phase(), Phase::Opening | Phase::Open)
    }

    /// Opens next to `anchor` (logical px, see [`Placement`]), kept inside the window.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, window: Vec2, items: Vec<MenuItem>) {
        ui.clear_children(self.plate);
        let items: Vec<MenuItem> = items.into_iter().take(MAX_ITEMS).collect();
        self.actions = items.iter().map(|i| i.action).collect();
        let mut height = 2.0 * PADDING;
        for (i, item) in items.iter().enumerate() {
            if item.separated && i > 0 {
                let line = ui.add(
                    self.plate,
                    Widget::container(),
                    Style {
                        size: Size { width: auto(), height: length(SEPARATOR_HEIGHT) },
                        padding: Rect {
                            left: length(14.0),
                            right: length(14.0),
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
                    Style {
                        flex_grow: 1.0,
                        size: Size { width: auto(), height: length(1.0) },
                        ..Default::default()
                    },
                );
                height += SEPARATOR_HEIGHT;
            }
            self.add_row(ui, i, item);
            height += ROW_HEIGHT;
        }
        let width = match self.placement {
            Placement::Below => WIDTH,
            Placement::Above => anchor.size.x.max(WIDTH * 0.75),
        };
        let max_left = (window.x - width - 8.0).max(8.0);
        let target = match self.placement {
            Placement::Below => {
                let left = (anchor.origin.x + anchor.size.x - width).clamp(8.0, max_left);
                Frame::new(left, anchor.origin.y + anchor.size.y + GAP_TO_ANCHOR, width, height)
            }
            Placement::Above => {
                let left = anchor.origin.x.clamp(8.0, max_left);
                let top = (anchor.origin.y - GAP_TO_ANCHOR - height).max(8.0);
                Frame::new(left, top, width, height)
            }
        };
        place(ui, self.root, self.parent, target);
        ui.set_visible(self.root, true);
        self.drip.open(config, anchor, target);
        self.animate(ui, 0.0);
    }

    fn add_row(&self, ui: &mut Ui, i: usize, item: &MenuItem) {
        let row = ui.add(
            self.plate,
            Widget::container().with_action(ActionId(self.base + i as u32)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(ROW_HEIGHT) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(12.0), height: length(0.0) },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
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
                ui.add(row, Widget::icon(icon, 19.0), Style::default());
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
            let label = ui.add_label(row, detail, TextStyle::Footnote, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
        }
        if item.checked {
            ui.add(row, Widget::icon(Icon::Check, 17.0).with_paint(Paint::Accent), Style::default());
        }
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.actions.clear();
        self.drip.close(config);
        self.animate(ui, 0.0);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        let phase = self.drip.step(dt);
        if phase == Phase::Closed {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_visual_frame(self.root, Some(self.drip.frame()));
        ui.set_opacity(self.plate, self.drip.content_opacity());
    }

    /// The menu action for a clicked row.
    pub fn action(&self, id: ActionId) -> Option<MenuAction> {
        let index = id.0.checked_sub(self.base)? as usize;
        self.actions.get(index).copied()
    }

    /// Whether a point (logical px) is inside the open menu.
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.is_open() && ui.frame(self.root).contains(p)
    }
}

/// Positions an absolute child of `parent` at `target` (absolute logical px).
pub fn place(ui: &mut Ui, node: NodeId, parent: NodeId, target: Frame) {
    let origin = target.origin - ui.frame(parent).origin;
    let mut style = ui.style(node);
    style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
    style.size = Size { width: length(target.size.x), height: length(target.size.y) };
    ui.set_style(node, style);
}
