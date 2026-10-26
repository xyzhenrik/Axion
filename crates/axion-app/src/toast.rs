//! Toasts: a small glass notification ("Link copied") that drips out of the control that
//! caused it, stays for `toast.duration_seconds` and flows back into it. Built on
//! [`Popover`], so it uses the same liquid spring as every other glass popover (no easing
//! curves of its own) and works with the sidebar open and over the Home page alike.
//!
//! Anything can raise one with [`Toast::show`]; a new toast replaces the one on screen.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{Paint, TextStyle, Ui, Widget};

use crate::popover::Popover;

const HEIGHT: f32 = 44.0;
const PADDING: f32 = 18.0;
const ICON: f32 = 18.0;
const GAP: f32 = 8.0;
/// Rough advance of one character of the toast's text (logical px).
const CHAR_WIDTH: f32 = 8.0;
const MIN_WIDTH: f32 = 120.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
/// Kept free at the window's edges.
const WINDOW_MARGIN: f32 = 8.0;

pub struct Toast {
    popover: Popover,
    icon: NodeId,
    label: NodeId,
    /// Seconds until it flows back (while shown).
    remaining: Option<f32>,
}

impl Toast {
    /// `parent` is the toolbar, so the toast's glass flows out of the bar's.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            HEIGHT / 2.0,
            Style {
                size: Size { width: auto(), height: length(HEIGHT) },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(GAP), height: length(0.0) },
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        )
        .detached();
        let plate = popover.root();
        let icon = ui.add(
            plate,
            Widget::icon(Icon::Check, ICON).with_paint(Paint::Accent),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        let label = ui.add_label(plate, "", TextStyle::Callout, Style::default());
        Self { popover, icon, label, remaining: None }
    }

    /// Drips out of `anchor` (the control that caused it) with `text` and `icon`, right-aligned
    /// with it and kept inside the window; stays for the configured time.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        icon: Icon,
        text: &str,
    ) {
        ui.set_text(self.label, text);
        ui.set_icon(self.icon, icon);
        let width = toast_width(text);
        let right = anchor.origin.x + anchor.size.x;
        let max_left = (window_width - width - WINDOW_MARGIN).max(WINDOW_MARGIN);
        let left = (right - width).clamp(WINDOW_MARGIN, max_left);
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, width, HEIGHT);
        self.popover.open(ui, config, anchor, target);
        self.remaining = Some(config.toast.duration_seconds.max(0.0));
    }

    /// Flows back into its anchor now.
    pub fn dismiss(&mut self, ui: &mut Ui, config: &Config) {
        self.remaining = None;
        self.popover.close(ui, config);
    }

    /// Runs the timer and advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32) {
        if let Some(left) = self.remaining {
            let left = left - dt;
            if left <= 0.0 {
                self.dismiss(ui, config);
            } else {
                self.remaining = Some(left);
            }
        }
        self.popover.animate(ui, dt);
    }
}

/// The toast's width for `text` (laid out before the drip knows its target).
fn toast_width(text: &str) -> f32 {
    (2.0 * PADDING + ICON + GAP + text.chars().count() as f32 * CHAR_WIDTH).max(MIN_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_grows_with_text_but_has_a_floor() {
        assert_eq!(toast_width("Hi"), MIN_WIDTH);
        assert!(toast_width("A much longer notification") > toast_width("Link copied"));
    }
}
