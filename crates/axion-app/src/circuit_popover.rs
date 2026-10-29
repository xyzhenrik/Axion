//! The circuit popover: drips out of the circuit button in the address field of a page that
//! goes through Tor and lists the path of the site's circuit (This browser, guard, middle
//! relay, exit relay, the site), like Tor Browser's circuit display, with two actions.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::popover::Popover;

const ACTION_BASE: u32 = 6970;
const NEW_CIRCUIT: ActionId = ActionId(ACTION_BASE + 1);
const NEW_IDENTITY: ActionId = ActionId(ACTION_BASE + 2);
const WIDTH: f32 = 372.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 16.0;
const GAP: f32 = 10.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const HEADER: f32 = 40.0;
const ROW: f32 = 44.0;
const RAIL: f32 = 14.0;
const LINE: f32 = 2.0;
const DOT: f32 = 10.0;
const BADGE_WIDTH: f32 = 34.0;
const BADGE_HEIGHT: f32 = 22.0;
const NOTE: f32 = 18.0;
const BUTTON: f32 = 36.0;

/// One stop on the path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircuitRow {
    pub kind: RowKind,
    /// Two-letter country code for the badge of a relay ("??" when unknown).
    pub badge: Option<String>,
    /// "This browser", "Germany (Guard)", "example.com".
    pub title: String,
    /// The relay's IP address (monospace); empty for the others.
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Browser,
    Relay,
    Site,
}

/// What the popover shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircuitView {
    /// "Circuit for example.com".
    pub title: String,
    /// A line under the title: bootstrap progress, or why there is no circuit yet.
    pub status: Option<String>,
    pub rows: Vec<CircuitRow>,
}

pub enum CircuitAction {
    NewCircuit,
    NewIdentity,
}

pub struct CircuitPopover {
    popover: Popover,
    plate: NodeId,
    /// What the content was built from: an unchanged view is not rebuilt on every update.
    shown: Option<CircuitView>,
}

impl CircuitPopover {
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            RADIUS,
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(GAP) },
                padding: uniform(PADDING + INSET),
                ..Default::default()
            },
        );
        let plate = popover.root();
        Self { popover, plate, shown: None }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    pub fn open(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, window_width: f32, view: &CircuitView) {
        self.fill(ui, view);
        let rows = view.rows.len() as f32;
        let status = if view.status.is_some() { NOTE + GAP } else { 0.0 };
        let height = 2.0 * (PADDING + INSET)
            + HEADER
            + status
            + rows * ROW
            + (NOTE + BUTTON + NOTE + BUTTON) // the two actions with their notes
            + 5.0 * GAP;
        let left = (anchor.origin.x - PADDING - 8.0).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        self.popover.open(ui, config, anchor, target);
    }

    /// Refreshes an open popover; the content is rebuilt only when the view changed.
    pub fn update(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, window_width: f32, view: &CircuitView) {
        if self.is_open() && self.shown.as_ref() != Some(view) {
            self.open(ui, config, anchor, window_width, view);
        }
    }

    fn fill(&mut self, ui: &mut Ui, view: &CircuitView) {
        self.shown = Some(view.clone());
        ui.clear_children(self.plate);
        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 12.0));
        ui.add(header, Widget::icon(Icon::Route, 26.0).with_paint(Paint::Accent), Style::default());
        let t = ui.add_label(header, &view.title, TextStyle::BodyEmphasis, shrink());
        ui.set_label_lines(t, 1);
        if let Some(status) = &view.status {
            let row = ui.add(self.plate, Widget::container(), fixed_row(NOTE, 0.0));
            secondary(ui, row, status, TextStyle::Footnote);
        }

        let last = view.rows.len().saturating_sub(1);
        for (i, row) in view.rows.iter().enumerate() {
            self.add_row(ui, row, i == 0, i == last);
        }

        self.add_action(ui, NEW_CIRCUIT, Icon::Reload, tr("New Tor circuit for this site"));
        let n = ui.add(self.plate, Widget::container(), fixed_row(NOTE, 0.0));
        secondary(ui, n, tr("Your guard relay may not change"), TextStyle::Footnote);
        self.add_action(ui, NEW_IDENTITY, Icon::Shuffle, tr("New Tor identity"));
        let n = ui.add(self.plate, Widget::container(), fixed_row(NOTE, 0.0));
        secondary(ui, n, tr("Closes every circuit and reloads the pages that use Tor"), TextStyle::Footnote);
    }

    fn add_action(&self, ui: &mut Ui, id: ActionId, icon: Icon, label: &str) {
        let button = ui.add(
            self.plate,
            Widget::container()
                .with_action(id)
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin)),
            Style {
                size: Size { width: auto(), height: length(BUTTON) },
                flex_shrink: 0.0,
                padding: Rect { left: length(14.0), right: length(12.0), top: length(0.0), bottom: length(0.0) },
                ..row_style(8.0)
            },
        );
        ui.add(button, Widget::icon(icon, 16.0).with_paint(Paint::Secondary), Style::default());
        let l = ui.add_label(button, label, TextStyle::Body, shrink());
        ui.set_label_lines(l, 1);
    }

    /// A row of the path: the rail (line, dot, line), then the badge or icon, the title and the
    /// address.
    fn add_row(&self, ui: &mut Ui, row: &CircuitRow, first: bool, last: bool) {
        let line = |ui: &mut Ui, parent: NodeId, visible: bool| {
            ui.add(
                parent,
                Widget::container().with_background(if visible {
                    Background::Fill(Paint::Secondary, 0.55)
                } else {
                    Background::None
                }),
                Style {
                    size: Size { width: length(LINE), height: auto() },
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    ..Default::default()
                },
            );
        };
        let node = ui.add(self.plate, Widget::container(), fixed_row(ROW, 10.0));
        let rail = ui.add(
            node,
            Widget::container(),
            Style {
                size: Size { width: length(RAIL), height: length(ROW) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        line(ui, rail, !first);
        let dot_paint = match row.kind {
            RowKind::Relay => Paint::Accent,
            _ => Paint::Secondary,
        };
        ui.add(
            rail,
            Widget::container().with_radius(CAPSULE).with_background(Background::Fill(dot_paint, 1.0)),
            Style { size: Size { width: length(DOT), height: length(DOT) }, flex_shrink: 0.0, ..Default::default() },
        );
        line(ui, rail, !last);

        match (&row.kind, &row.badge) {
            (RowKind::Relay, Some(code)) => {
                let badge = ui.add(
                    node,
                    Widget::container().with_radius(CAPSULE).with_background(Background::Glass(GlassLevel::Thin)),
                    Style {
                        size: Size { width: length(BADGE_WIDTH), height: length(BADGE_HEIGHT) },
                        flex_shrink: 0.0,
                        justify_content: JustifyContent::CENTER,
                        ..row_style(0.0)
                    },
                );
                ui.add_label(badge, code, TextStyle::Caption, Style::default());
            }
            (RowKind::Browser, _) => {
                ui.add(node, Widget::icon(Icon::Screen, 18.0).with_paint(Paint::Secondary), icon_slot());
            }
            _ => {
                ui.add(node, Widget::icon(Icon::Globe, 18.0).with_paint(Paint::Secondary), icon_slot());
            }
        }
        let title = ui.add_label(node, &row.title, TextStyle::Body, shrink_grow());
        ui.set_label_lines(title, 1);
        if !row.detail.is_empty() {
            let ip = ui.add_label(node, &row.detail, TextStyle::Mono, Style { flex_shrink: 0.0, ..Default::default() });
            if let Some(w) = ui.widget_mut(ip) {
                w.set_paint(Paint::Secondary);
            }
        }
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.shown = None;
        self.popover.close(ui, config);
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    pub fn action(&self, id: ActionId) -> Option<CircuitAction> {
        match id {
            NEW_CIRCUIT => Some(CircuitAction::NewCircuit),
            NEW_IDENTITY => Some(CircuitAction::NewIdentity),
            _ => None,
        }
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

fn row_style(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn fixed_row(height: f32, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row_style(gap) }
}

fn icon_slot() -> Style {
    Style { size: Size { width: length(BADGE_WIDTH), height: length(BADGE_HEIGHT) }, flex_shrink: 0.0, ..Default::default() }
}

fn shrink() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

fn shrink_grow() -> Style {
    Style { flex_grow: 1.0, ..shrink() }
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, shrink());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}
