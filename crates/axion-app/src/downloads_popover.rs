//! The downloads popover (Safari's downloads button): drips out of the toolbar's download
//! button and lists the latest downloads with their progress, a cancel button while they
//! run and open / show in folder when done; "Show All" opens the sidebar's Downloads list.
//! Built like the Shields popover, so its glass merges with the toolbar's.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::chrome::PanelItem;
use crate::drip::{Drip, Phase};
use crate::menu::place;

const ACTION_BASE: u32 = 6200;
const SHOW_ALL: ActionId = ActionId(ACTION_BASE);
const ROW_BASE: u32 = ACTION_BASE + 10;
const BUTTON_BASE: u32 = ACTION_BASE + 50;
/// Buttons a row can have (pause/resume and cancel, or show in folder).
const ROW_BUTTONS: usize = 4;
/// How many downloads the popover lists.
pub const ROWS: usize = 6;
const WIDTH: f32 = 340.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 12.0;
const GAP: f32 = 4.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const PLATE_OPACITY: f32 = 0.55;
/// Fixed row heights, so the drip knows its target size before layout.
const HEADER: f32 = 30.0;
const ROW: f32 = 54.0;
const EMPTY: f32 = 40.0;
const FOOTER: f32 = 36.0;
const PROGRESS_HEIGHT: f32 = 4.0;

/// A click in the popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadsAction {
    /// Row `i`: opens a finished file.
    Open(usize),
    /// Row `i`'s `k`th button.
    Button(usize, usize),
    ShowAll,
}

pub struct DownloadsPopover {
    parent: NodeId,
    root: NodeId,
    plate: NodeId,
    drip: Drip,
}

impl DownloadsPopover {
    /// `parent` is the toolbar, so the popover's glass merges with it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let root = ui.add(
            parent,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: length(WIDTH), height: auto() },
                padding: uniform(PADDING),
                ..Default::default()
            },
        );
        let plate = ui.add(
            root,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, PLATE_OPACITY))
                .with_radius(RADIUS - PADDING),
            Style {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(GAP) },
                padding: uniform(INSET),
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        Self { parent, root, plate, drip: Drip::new() }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.drip.phase(), Phase::Opening | Phase::Open)
    }

    /// Opens below `anchor` (the download button), right-aligned with it, or refreshes the
    /// content if already open.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        items: &[PanelItem],
    ) {
        self.fill(ui, items);
        let rows = if items.is_empty() { EMPTY } else { ROW * items.len() as f32 };
        let count = 2.0 + items.len().max(1) as f32;
        let height = 2.0 * (PADDING + INSET) + HEADER + rows + FOOTER + (count - 1.0) * GAP;
        let right = anchor.origin.x + anchor.size.x + PADDING + 8.0;
        let left = (right - WIDTH).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        place(ui, self.root, self.parent, target);
        ui.set_visible(self.root, true);
        if self.is_open() {
            self.drip.retarget(target);
        } else {
            self.drip.open(config, anchor, target);
        }
        self.animate(ui, 0.0);
    }

    fn fill(&mut self, ui: &mut Ui, items: &[PanelItem]) {
        ui.clear_children(self.plate);
        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 8.0));
        ui.add_label(header, "Downloads", TextStyle::BodyEmphasis, Style::default());
        if items.is_empty() {
            let empty = ui.add(self.plate, Widget::container(), fixed_row(EMPTY, 0.0));
            secondary(ui, empty, "Nothing downloaded yet", TextStyle::Footnote);
        }
        for (i, item) in items.iter().enumerate() {
            let row_node = ui.add(
                self.plate,
                Widget::container().with_action(ActionId(ROW_BASE + i as u32)).with_radius(14.0),
                Style { padding: padding(8.0, 4.0), ..fixed_row(ROW, 10.0) },
            );
            let icon = item.icon.unwrap_or(Icon::File);
            ui.add(row_node, Widget::icon(icon, 24.0).with_paint(Paint::Accent), Style::default());
            let text = ui.add(
                row_node,
                Widget::container(),
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    flex_direction: FlexDirection::Column,
                    gap: Size { width: length(0.0), height: length(3.0) },
                    ..Default::default()
                },
            );
            ui.add_label(text, &item.title, TextStyle::Body, shrinking());
            if let Some(progress) = item.progress {
                let track = ui.add(
                    text,
                    Widget::container()
                        .with_background(Background::Fill(Paint::Label, 0.12))
                        .with_radius(CAPSULE),
                    Style {
                        size: Size { width: percent(1.0), height: length(PROGRESS_HEIGHT) },
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
            secondary(ui, text, &item.detail, TextStyle::Footnote);
            for (k, &icon) in item.buttons.iter().take(ROW_BUTTONS).enumerate() {
                let id = ActionId(BUTTON_BASE + (i * ROW_BUTTONS + k) as u32);
                let button = ui.add(
                    row_node,
                    Widget::container()
                        .with_action(id)
                        .with_radius(CAPSULE)
                        .with_background(Background::Fill(Paint::Label, 0.07)),
                    Style {
                        size: Size { width: length(28.0), height: length(28.0) },
                        flex_shrink: 0.0,
                        justify_content: JustifyContent::CENTER,
                        ..row(0.0)
                    },
                );
                ui.add(button, Widget::icon(icon, 14.0).with_paint(Paint::Accent), Style::default());
            }
        }
        let footer = ui.add(
            self.plate,
            Widget::container().with_action(SHOW_ALL).with_radius(CAPSULE),
            Style { padding: padding(8.0, 0.0), ..fixed_row(FOOTER, 4.0) },
        );
        let label = ui.add_label(footer, "Show All Downloads", TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Accent);
        }
        let mut style = ui.style(label);
        style.flex_grow = 1.0;
        ui.set_style(label, style);
        ui.add(footer, Widget::icon(Icon::ChevronRight, 14.0).with_paint(Paint::Secondary), Style::default());
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.drip.close(config);
        self.animate(ui, 0.0);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if self.drip.step(dt) == Phase::Closed {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_visual_frame(self.root, Some(self.drip.frame()));
        ui.set_opacity(self.plate, self.drip.content_opacity());
    }

    pub fn action(&self, id: ActionId) -> Option<DownloadsAction> {
        action_of(id)
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.is_open() && ui.frame(self.root).contains(p)
    }
}

/// The popover's clicks, by action id.
fn action_of(id: ActionId) -> Option<DownloadsAction> {
    let n = id.0;
    if id == SHOW_ALL {
        Some(DownloadsAction::ShowAll)
    } else if (ROW_BASE..ROW_BASE + ROWS as u32).contains(&n) {
        Some(DownloadsAction::Open((n - ROW_BASE) as usize))
    } else if (BUTTON_BASE..BUTTON_BASE + (ROWS * ROW_BUTTONS) as u32).contains(&n) {
        let k = (n - BUTTON_BASE) as usize;
        Some(DownloadsAction::Button(k / ROW_BUTTONS, k % ROW_BUTTONS))
    } else {
        None
    }
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

fn padding(h: f32, v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(h), right: length(h), top: length(v), bottom: length(v) }
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn fixed_row(height: f32, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row(gap) }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, shrinking());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids() {
        assert_eq!(action_of(SHOW_ALL), Some(DownloadsAction::ShowAll));
        assert_eq!(action_of(ActionId(ROW_BASE + 3)), Some(DownloadsAction::Open(3)));
        let id = ActionId(BUTTON_BASE + (2 * ROW_BUTTONS + 1) as u32);
        assert_eq!(action_of(id), Some(DownloadsAction::Button(2, 1)));
        assert_eq!(action_of(ActionId(ACTION_BASE + 99)), None);
    }
}
