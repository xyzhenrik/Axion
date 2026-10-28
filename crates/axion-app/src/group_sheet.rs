//! The sheet that names a tab group and picks its colour (a new group, or Rename Group).
//! A small glass card over the dimmed window, like the Add Account sheet.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::tr;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

use crate::tabs::GroupColor;

const BASE: u32 = 70_000;
const CANCEL: ActionId = ActionId(BASE);
const SAVE: ActionId = ActionId(BASE + 1);
const COLOR_BASE: u32 = BASE + 10;
const WIDTH: f32 = 340.0;
const RADIUS: f32 = 22.0;
const SWATCH: f32 = 30.0;
const START_SCALE: f32 = 0.88;
const BACKDROP_DIM: f32 = 0.28;

/// A click in the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupSheetAction {
    Save,
    Cancel,
    Color(GroupColor),
}

/// What the sheet is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupSheetPurpose {
    /// The tab (by view) goes into a new group.
    New(axion_engine::ViewId),
    /// Renames (and recolours) this group.
    Edit(u32),
}

pub struct GroupSheet {
    root: NodeId,
    alert: NodeId,
    title: NodeId,
    field: NodeId,
    /// The swatches' check marks, in palette order.
    checks: Vec<NodeId>,
    color: GroupColor,
    purpose: Option<GroupSheetPurpose>,
    open: bool,
    appear: Animated<f32>,
}

impl GroupSheet {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(CANCEL),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let alert = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(10.0) },
                padding: Rect {
                    left: length(20.0),
                    right: length(20.0),
                    top: length(20.0),
                    bottom: length(18.0),
                },
                ..Default::default()
            },
        );
        let title = ui.add_label(alert, tr("New Tab Group"), TextStyle::BodyEmphasis, Style::default());
        let field_box = ui.add(
            alert,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(12.0),
            Style {
                size: Size { width: percent(1.0), height: length(40.0) },
                padding: Rect {
                    left: length(12.0),
                    right: length(12.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let field = ui.add_text_field(
            field_box,
            tr("Group Name"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let swatches = ui.add(
            alert,
            Widget::container(),
            Style {
                gap: Size { width: length(8.0), height: length(0.0) },
                justify_content: JustifyContent::SPACE_BETWEEN,
                margin: Rect { left: length(0.0), right: length(0.0), top: length(4.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        let mut checks = Vec::new();
        for (i, color) in GroupColor::ALL.iter().enumerate() {
            let swatch = ui.add(
                swatches,
                Widget::container()
                    .with_action(ActionId(COLOR_BASE + i as u32))
                    .with_radius(CAPSULE)
                    .with_background(Background::TintedGlass(GlassLevel::Thin, Paint::Fixed(color.color()))),
                Style {
                    size: Size { width: length(SWATCH), height: length(SWATCH) },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            );
            let check = ui.add(
                swatch,
                Widget::icon(Icon::Check, 15.0)
                    .with_icon_weight(IconWeight::Heavy)
                    .with_paint(Paint::OnColor(color.color())),
                Style::default(),
            );
            checks.push(check);
        }
        let buttons = ui.add(
            alert,
            Widget::container(),
            Style {
                margin: Rect { left: length(0.0), right: length(0.0), top: length(6.0), bottom: length(0.0) },
                gap: Size { width: length(10.0), height: length(0.0) },
                ..Default::default()
            },
        );
        button(ui, buttons, CANCEL, tr("Cancel"), false);
        button(ui, buttons, SAVE, tr("Save"), true);
        ui.set_visible(root, false);
        Self {
            root,
            alert,
            title,
            field,
            checks,
            color: GroupColor::Blue,
            purpose: None,
            open: false,
            appear: Animated::new(0.0, Spring::SNAPPY),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn purpose(&self) -> Option<GroupSheetPurpose> {
        self.purpose
    }

    pub fn action(&self, id: ActionId) -> Option<GroupSheetAction> {
        match id {
            CANCEL => Some(GroupSheetAction::Cancel),
            SAVE => Some(GroupSheetAction::Save),
            ActionId(n) if (COLOR_BASE..COLOR_BASE + GroupColor::ALL.len() as u32).contains(&n) => {
                Some(GroupSheetAction::Color(GroupColor::ALL[(n - COLOR_BASE) as usize]))
            }
            _ => None,
        }
    }

    /// Whether `p` (logical px) is on the card itself (the backdrop's cancel also fires for
    /// clicks on the card's background).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.alert).contains(p)
    }

    /// Opens with the name field focused; returns the focus events.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        purpose: GroupSheetPurpose,
        name: &str,
        color: GroupColor,
    ) -> Vec<UiEvent> {
        self.open = true;
        self.purpose = Some(purpose);
        self.appear.spring = if config.glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(
                config.motion.snappy.response * config.motion.speed,
                config.glass.damping(config.motion.snappy.damping),
            )
        };
        self.appear.snap(0.0);
        self.appear.target = 1.0;
        let heading = match purpose {
            GroupSheetPurpose::New(_) => tr("New Tab Group"),
            GroupSheetPurpose::Edit(_) => tr("Rename Group"),
        };
        ui.set_text(self.title, heading);
        ui.set_field_text(self.field, name);
        self.set_color(ui, color);
        ui.set_visible(self.root, true);
        ui.focus(self.field)
    }

    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
        self.purpose = None;
        self.appear.target = 0.0;
        let _ = ui.blur();
    }

    pub fn color(&self) -> GroupColor {
        self.color
    }

    pub fn set_color(&mut self, ui: &mut Ui, color: GroupColor) {
        self.color = color;
        for (c, check) in GroupColor::ALL.iter().zip(&self.checks) {
            ui.set_visible(*check, *c == color);
        }
    }

    /// The name typed, trimmed (may be empty: the group is then shown as untitled).
    pub fn name(&self, ui: &Ui) -> String {
        ui.field_text(self.field).unwrap_or_default().trim().to_owned()
    }

    /// Advances the appear / disappear animation. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if !self.open && k < 0.01 {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_opacity(self.root, k.min(1.0));
        let frame = ui.frame(self.alert);
        if frame.size.x > 0.0 {
            let scale = START_SCALE + (1.0 - START_SCALE) * k;
            let size = frame.size * scale;
            let origin = frame.origin + (frame.size - size) / 2.0;
            ui.set_visual_frame(self.alert, Some(Frame { origin, size: size.max(Vec2::ONE) }));
        }
    }
}

fn button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) {
    let background = if primary {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
    let button = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            size: Size { width: auto(), height: length(40.0) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(button, text, TextStyle::BodyEmphasis, Style::default());
    if primary && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnAccent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_ids_round_trip() {
        let sheet_ids: Vec<_> = (0..GroupColor::ALL.len() as u32).map(|i| ActionId(COLOR_BASE + i)).collect();
        for (id, color) in sheet_ids.iter().zip(GroupColor::ALL) {
            let n = id.0;
            assert!((COLOR_BASE..COLOR_BASE + 8).contains(&n));
            assert_eq!(GroupColor::ALL[(n - COLOR_BASE) as usize], color);
        }
    }
}
