//! A glass popover menu (the page menu ≡) that drips out of its button. It lives in the
//! toolbar's plane with the toolbar's glass, so it visibly flows out of the bar. Clicking
//! outside or pressing Escape closes it.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::drip::{Drip, Phase};

const ITEM_BASE: u32 = 6000;
const WIDTH: f32 = 280.0;
const ROW_HEIGHT: f32 = 44.0;
const PADDING: f32 = 6.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 22.0;
/// The frosted plate under the rows keeps text legible over any page.
const PLATE_OPACITY: f32 = 0.55;

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
    parent: NodeId,
    root: NodeId,
    plate: NodeId,
    actions: Vec<MenuAction>,
    drip: Drip,
}

impl Menu {
    /// `parent` is the toolbar, so the menu's glass merges with it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
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
        Self { parent, root, plate, actions: Vec::new(), drip: Drip::new() }
    }

    /// Open (or opening): clicks on rows count, outside clicks close it.
    pub fn is_open(&self) -> bool {
        matches!(self.drip.phase(), Phase::Opening | Phase::Open)
    }

    /// Opens below `anchor` (logical px), right-aligned to it and kept inside the window.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        items: Vec<MenuItem>,
    ) {
        ui.clear_children(self.plate);
        self.actions = items.iter().map(|i| i.action).collect();
        for (i, item) in items.iter().enumerate() {
            let row = ui.add(
                self.plate,
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
        let height = items.len() as f32 * ROW_HEIGHT + 2.0 * PADDING;
        let right = anchor.origin.x + anchor.size.x;
        let left = (right - WIDTH).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        place(ui, self.root, self.parent, target);
        ui.set_visible(self.root, true);
        self.drip.open(config, anchor, target);
        self.animate(ui, 0.0);
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
        let index = id.0.checked_sub(ITEM_BASE)? as usize;
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
