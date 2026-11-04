//! The small "Restart" pill that stays in the window after "Later" was chosen in the restart
//! notice (`general.restart_hint_pill`): a glass capsule under the toolbar's right end that
//! opens the notice again. A plane of its own above the page; it claims its space, so toasts and
//! popovers keep clear of it.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::tr;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

/// The pill's click id (free: see `action_ids`).
const ACTION: ActionId = ActionId(9260);
const HEIGHT: f32 = 30.0;
const GAP_BELOW_ANCHOR: f32 = 8.0;
/// Kept free at the window's right edge.
const WINDOW_MARGIN: f32 = 12.0;
/// The pill pops in from this size.
const START_SCALE: f32 = 0.8;

pub struct RestartPill {
    root: NodeId,
    shown: bool,
    appear: Animated<f32>,
    /// Where the pill was last placed (to avoid restyling every frame).
    placed: Option<(f32, f32)>,
}

impl RestartPill {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_action(ACTION)
                .with_radius(CAPSULE)
                .with_background(Background::TintedGlass(GlassLevel::Thin, Paint::Accent)),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(HEIGHT) },
                padding: Rect {
                    left: length(12.0),
                    right: length(14.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(6.0), height: length(0.0) },
                ..Default::default()
            },
        );
        ui.add(root, Widget::icon(Icon::Reload, 14.0).with_paint(Paint::OnAccent), Style::default());
        let label = ui.add_label(root, tr("Restart"), TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::OnAccent);
        }
        ui.claim_space(root);
        ui.set_visible(root, false);
        Self { root, shown: false, appear: Animated::new(0.0, Spring::SNAPPY), placed: None }
    }

    /// Whether `id` is the pill's click id.
    pub fn owns(&self, id: ActionId) -> bool {
        id == ACTION
    }

    /// Shows or hides the pill (it pops in and out on the snappy spring).
    pub fn set_shown(&mut self, ui: &mut Ui, config: &Config, shown: bool) {
        if shown == self.shown {
            return;
        }
        self.shown = shown;
        if shown {
            self.appear.spring = if config.glass.reduce_motion {
                Spring::new(0.15, 1.0)
            } else {
                Spring::new(
                    config.motion.snappy.response * config.motion.speed,
                    config.glass.damping(config.motion.snappy.damping),
                )
            };
            self.appear.snap(0.0);
            self.placed = None;
            ui.set_visible(self.root, true);
        }
        self.appear.target = if shown { 1.0 } else { 0.0 };
    }

    /// Advances the pop; `anchor` is the toolbar's right-most button, `window` the window size
    /// (logical px). Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, anchor: Frame, window: Vec2, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if !self.shown && k < 0.01 {
            ui.set_visible(self.root, false);
            ui.set_visual_frame(self.root, None);
            return;
        }
        // Right-aligned with the anchor, kept inside the window.
        let right = (window.x - (anchor.origin.x + anchor.size.x)).max(WINDOW_MARGIN);
        let top = anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR;
        if self.placed != Some((right, top)) {
            let mut style = ui.style(self.root);
            style.inset = Rect { left: auto(), right: length(right), top: length(top), bottom: auto() };
            ui.set_style(self.root, style);
            self.placed = Some((right, top));
        }
        ui.set_opacity(self.root, k.min(1.0));
        let frame = ui.frame(self.root);
        if frame.size.x > 0.0 {
            let scale = START_SCALE + (1.0 - START_SCALE) * k;
            let size = frame.size * scale;
            let origin = frame.origin + (frame.size - size) / 2.0;
            ui.set_visual_frame(self.root, Some(Frame { origin, size: size.max(Vec2::ONE) }));
        }
    }
}
