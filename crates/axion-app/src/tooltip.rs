//! Tooltips: a small glass capsule under a button that says what it does (and its keyboard
//! shortcut) once the pointer has rested on it for `ui.tooltip_delay_ms`.
//!
//! Buttons opt in through [`axion_ui::Widget::tooltip`]; this module only watches the hovered
//! button ([`Dwell`]), draws the capsule and springs it in and out (`axion-physics`, no easing
//! curves). It goes away at once when the pointer leaves, on a click, scroll or key press,
//! and never shows while a menu, popover or drag is active (the caller passes `blocked`).

use axion_compositor::Rect as Frame;
use axion_config::{Config, Keymap};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

const HEIGHT: f32 = 30.0;
const PADDING: f32 = 13.0;
const GAP: f32 = 8.0;
/// Space between the button and the capsule.
const GAP_BELOW_ANCHOR: f32 = 8.0;
/// Kept free at the window's edges.
const WINDOW_MARGIN: f32 = 8.0;
/// The capsule grows from this scale to full size as it appears.
const MIN_SCALE: f32 = 0.82;
/// Below this opacity (and heading away) the capsule is taken out of the layout.
const HIDDEN_BELOW: f32 = 0.01;

/// Decides when a tooltip for the hovered item should be visible: after the pointer rested on
/// the same item for the delay, until the item changes or the tooltip is cancelled.
#[derive(Debug)]
pub struct Dwell<K> {
    item: Option<K>,
    /// Seconds the pointer has rested on `item`.
    rested: f32,
    /// Cancelled (click, scroll, key) for `item`: stays hidden until another item is hovered.
    cancelled: bool,
}

impl<K: PartialEq + Copy> Dwell<K> {
    pub fn new() -> Self {
        Self { item: None, rested: 0.0, cancelled: false }
    }

    /// Hides the tooltip of the current item until the pointer moves to another one.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Advances by `dt` seconds with `hovered` under the pointer; returns the item whose
    /// tooltip is visible, if any. `blocked` (a menu or drag is active) restarts the wait.
    pub fn update(&mut self, hovered: Option<K>, blocked: bool, dt: f32, delay: f32) -> Option<K> {
        if blocked || hovered != self.item {
            self.item = if blocked { None } else { hovered };
            self.rested = 0.0;
            self.cancelled = false;
            return None;
        }
        let item = self.item?;
        self.rested += dt;
        (!self.cancelled && self.rested >= delay).then_some(item)
    }
}

pub struct Tooltip {
    plate: NodeId,
    label: NodeId,
    shortcut: NodeId,
    /// 0: hidden, 1: shown; overshoots a little while it springs in.
    presence: Animated<f32>,
    dwell: Dwell<NodeId>,
    /// The button the capsule belongs to, while shown.
    anchor: Option<(NodeId, Frame)>,
    /// Last position set, to relayout only when it moved.
    placed: Option<Vec2>,
}

impl Tooltip {
    /// `parent` is the root, so the capsule lies above everything built before it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let plate = ui.add(
            parent,
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                size: Size { width: auto(), height: length(HEIGHT) },
                align_items: AlignItems::CENTER,
                flex_direction: FlexDirection::Row,
                gap: Size { width: length(GAP), height: length(0.0) },
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        let label = ui.add_label(plate, "", TextStyle::Footnote, Style::default());
        let shortcut = ui.add_label(plate, "", TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(shortcut) {
            w.set_paint(Paint::Secondary);
        }
        ui.set_visible(plate, false);
        ui.set_opacity(plate, 0.0);
        Self {
            plate,
            label,
            shortcut,
            presence: Animated::new(0.0, Spring::SNAPPY),
            dwell: Dwell::new(),
            anchor: None,
            placed: None,
        }
    }

    /// A click, scroll or key press: the tooltip goes away at once.
    pub fn cancel(&mut self, ui: &mut Ui) {
        self.dwell.cancel();
        self.hide_now(ui);
    }

    fn hide_now(&mut self, ui: &mut Ui) {
        self.anchor = None;
        self.presence.snap(0.0);
        ui.set_opacity(self.plate, 0.0);
        ui.set_visual_frame(self.plate, None);
        ui.set_visible(self.plate, false);
        self.placed = None;
    }

    /// Follows the pointer and springs the capsule; call every frame. `window` is the
    /// window's logical size, `blocked` whether a menu, popover or drag is active.
    pub fn animate(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        keymap: &Keymap,
        window: Vec2,
        blocked: bool,
        dt: f32,
    ) {
        let hovered = ui.hovered_tooltip().map(|(node, ..)| node);
        let delay = config.ui.tooltip_delay_ms as f32 / 1000.0;
        match self.dwell.update(hovered, blocked, dt, delay) {
            Some(node) => {
                if self.anchor.map(|(n, _)| n) != Some(node) {
                    self.show(ui, keymap, node);
                }
                if let Some((_, frame, _)) = ui.hovered_tooltip() {
                    self.anchor = Some((node, frame));
                }
                self.presence.target = 1.0;
            }
            None if self.anchor.is_some() => self.hide_now(ui),
            None => {}
        }
        if self.anchor.is_none() {
            return;
        }
        // Flat mode: the same quick fade and slight scale, without the spring's overshoot.
        self.presence.spring = if config.glass.is_flat() {
            crate::drip::spring(config, config.motion.snappy)
        } else {
            Spring::SNAPPY
        };
        self.presence.step(dt);
        self.place(ui, window);
    }

    fn show(&mut self, ui: &mut Ui, keymap: &Keymap, node: NodeId) {
        let Some(tip) = ui.widget(node).and_then(|w| w.tooltip.clone()) else { return };
        let keys = tip.action.and_then(|a| keymap.chords_for(a).first().map(ToString::to_string));
        ui.set_text(self.label, &tip.label);
        ui.set_text(self.shortcut, keys.as_deref().unwrap_or(""));
        ui.set_visible(self.shortcut, keys.is_some());
        ui.set_visible(self.plate, true);
        self.presence.snap(0.0);
        self.placed = None;
    }

    /// Under the anchor, centred, inside the window (above it when there is no room below).
    fn place(&mut self, ui: &mut Ui, window: Vec2) {
        let Some((_, anchor)) = self.anchor else { return };
        let size = ui.frame(self.plate).size;
        let left = (anchor.origin.x + anchor.size.x / 2.0 - size.x / 2.0)
            .clamp(WINDOW_MARGIN, (window.x - size.x - WINDOW_MARGIN).max(WINDOW_MARGIN));
        let below = anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR;
        let top = if below + size.y > window.y - WINDOW_MARGIN {
            (anchor.origin.y - GAP_BELOW_ANCHOR - size.y).max(WINDOW_MARGIN)
        } else {
            below
        };
        let at = Vec2::new(left, top);
        if self.placed != Some(at) {
            self.placed = Some(at);
            let mut style = ui.style(self.plate);
            style.inset = Rect { left: length(left), right: auto(), top: length(top), bottom: auto() };
            ui.set_style(self.plate, style);
        }
        let t = self.presence.value;
        ui.set_opacity(self.plate, t.clamp(0.0, 1.0));
        // Grows out of its edge facing the button.
        let scale = MIN_SCALE + (1.0 - MIN_SCALE) * t.max(0.0);
        let (w, h) = (size.x * scale, size.y * scale);
        let x = left + (size.x - w) / 2.0;
        let y = if top < anchor.origin.y { top + size.y - h } else { top };
        ui.set_visual_frame(self.plate, Some(Frame::new(x, y, w, h)));
        if t < HIDDEN_BELOW && self.presence.target == 0.0 {
            self.hide_now(ui);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: f32 = 0.7;

    #[test]
    fn shows_only_after_the_delay() {
        let mut d = Dwell::new();
        assert_eq!(d.update(Some(1), false, 0.0, DELAY), None);
        assert_eq!(d.update(Some(1), false, 0.5, DELAY), None);
        assert_eq!(d.update(Some(1), false, 0.3, DELAY), Some(1));
        assert_eq!(d.update(Some(1), false, 0.1, DELAY), Some(1));
    }

    #[test]
    fn moving_to_another_item_restarts_the_wait() {
        let mut d = Dwell::new();
        d.update(Some(1), false, 0.0, DELAY);
        assert_eq!(d.update(Some(1), false, 1.0, DELAY), Some(1));
        assert_eq!(d.update(Some(2), false, 0.1, DELAY), None);
        assert_eq!(d.update(Some(2), false, 0.5, DELAY), None);
        assert_eq!(d.update(Some(2), false, 0.3, DELAY), Some(2));
    }

    #[test]
    fn leaving_hides_and_restarts() {
        let mut d = Dwell::new();
        d.update(Some(1), false, 0.0, DELAY);
        assert_eq!(d.update(Some(1), false, 1.0, DELAY), Some(1));
        assert_eq!(d.update(None::<u32>, false, 0.1, DELAY), None);
        d.update(Some(1), false, 0.0, DELAY);
        assert_eq!(d.update(Some(1), false, 0.3, DELAY), None);
    }

    #[test]
    fn cancel_hides_until_the_item_changes() {
        let mut d = Dwell::new();
        d.update(Some(1), false, 0.0, DELAY);
        assert_eq!(d.update(Some(1), false, 1.0, DELAY), Some(1));
        d.cancel();
        assert_eq!(d.update(Some(1), false, 5.0, DELAY), None);
        d.update(Some(2), false, 0.0, DELAY);
        assert_eq!(d.update(Some(2), false, 1.0, DELAY), Some(2));
    }

    #[test]
    fn blocked_never_shows_and_restarts_the_wait() {
        let mut d = Dwell::new();
        d.update(Some(1), false, 0.0, DELAY);
        assert_eq!(d.update(Some(1), true, 5.0, DELAY), None);
        assert_eq!(d.update(Some(1), false, 0.0, DELAY), None);
        assert_eq!(d.update(Some(1), false, 0.5, DELAY), None);
        assert_eq!(d.update(Some(1), false, 0.3, DELAY), Some(1));
    }
}
