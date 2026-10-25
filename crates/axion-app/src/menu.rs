//! A glass popover menu anchored to a button (the page menu ≡). Clicking outside or pressing
//! Escape closes it.

use axion_compositor::{Icon, Rect as Frame};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};

const ITEM_BASE: u32 = 6000;
const WIDTH: f32 = 280.0;
const ROW_HEIGHT: f32 = 44.0;
const GAP_BELOW_ANCHOR: f32 = 10.0;
const RADIUS: f32 = 20.0;

/// What a page menu entry does.
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
}

pub struct MenuItem {
    pub icon: Icon,
    pub label: String,
    /// Secondary text at the end of the row (e.g. the current zoom).
    pub detail: Option<String>,
    pub action: MenuAction,
}

impl MenuItem {
    pub fn new(icon: Icon, label: &str, action: MenuAction) -> Self {
        Self { icon, label: label.to_owned(), detail: None, action }
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }
}

pub struct Menu {
    root: NodeId,
    actions: Vec<MenuAction>,
}

impl Menu {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: length(WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(6.0),
                    right: length(6.0),
                    top: length(6.0),
                    bottom: length(6.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        Self { root, actions: Vec::new() }
    }

    pub fn is_open(&self) -> bool {
        !self.actions.is_empty()
    }

    /// Opens below `anchor` (logical px), right-aligned to it and kept inside the window.
    pub fn open(&mut self, ui: &mut Ui, anchor: Frame, window_width: f32, items: Vec<MenuItem>) {
        ui.clear_children(self.root);
        self.actions = items.iter().map(|i| i.action).collect();
        for (i, item) in items.iter().enumerate() {
            let row = ui.add(
                self.root,
                Widget::container().with_action(ActionId(ITEM_BASE + i as u32)).with_radius(CAPSULE),
                Style {
                    size: Size { width: auto(), height: length(ROW_HEIGHT) },
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
            ui.add(row, Widget::icon(item.icon, 19.0), Style::default());
            ui.add_label(
                row,
                &item.label,
                TextStyle::Body,
                Style {
                    flex_grow: 1.0,
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
        }
        let right = anchor.origin.x + anchor.size.x;
        let left = (right - WIDTH).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let mut style = ui.style(self.root);
        style.inset = Rect {
            left: length(left),
            right: auto(),
            top: length(anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR),
            bottom: auto(),
        };
        ui.set_style(self.root, style);
        ui.set_visible(self.root, true);
    }

    pub fn close(&mut self, ui: &mut Ui) {
        self.actions.clear();
        ui.set_visible(self.root, false);
    }

    /// The menu action for a clicked row.
    pub fn action(&self, id: ActionId) -> Option<MenuAction> {
        let index = id.0.checked_sub(ITEM_BASE)? as usize;
        self.actions.get(index).copied()
    }

    /// Whether a point (logical px) is inside the open menu.
    pub fn contains(&self, ui: &Ui, p: glam::Vec2) -> bool {
        self.is_open() && ui.frame(self.root).contains(p)
    }
}
