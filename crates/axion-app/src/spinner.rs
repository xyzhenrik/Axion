//! The loading indicator of the players (Playlist, fullscreen player, Picture in Picture):
//! a turning ring (Lucide `loader-circle`) on a glass disc. Pure glass, no fill; in flat mode
//! the disc becomes the usual flat card. It shows while a video loads or buffers
//! (`Player::loading`) and fades with a spring (snaps with Reduce Motion).

use axion_compositor::Icon;
use axion_config::Config;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, Paint, Ui, Widget};

/// The ring's size relative to its disc.
const RING_RATIO: f32 = 0.5;
const SETTLED: f32 = 0.01;

pub struct Spinner {
    root: NodeId,
    glyph: NodeId,
    angle: f32,
    opacity: Animated<f32>,
}

impl Spinner {
    /// A disc of `side` logical px under `parent` (a flex container: it is laid out by it).
    pub fn build(ui: &mut Ui, parent: NodeId, side: f32) -> Self {
        let root = ui.add(
            parent,
            Widget::container()
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_button_tint(),
            Style {
                size: Size { width: length(side), height: length(side) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let glyph = ui.add(
            root,
            Widget::icon(Icon::LoaderCircle, side * RING_RATIO).with_paint(Paint::Label),
            Style::default(),
        );
        ui.set_visible(root, false);
        Self { root, glyph, angle: 0.0, opacity: Animated::new(0.0, Spring::SNAPPY) }
    }

    /// Whether the disc is on screen (still fading out counts).
    pub fn is_shown(&self) -> bool {
        self.opacity.value > SETTLED || self.opacity.target > 0.5
    }

    /// Turns and fades the ring; `want` is whether something is loading. Returns whether the
    /// disc is shown.
    pub fn update(&mut self, ui: &mut Ui, config: &Config, want: bool, dt: f32) -> bool {
        let want = want && config.media.loading_indicator;
        self.opacity.target = if want { 1.0 } else { 0.0 };
        if config.glass.reduce_motion {
            self.opacity.snap(self.opacity.target);
        } else {
            self.opacity.step(dt);
        }
        let shown = self.is_shown();
        ui.set_visible(self.root, shown);
        if !shown {
            return false;
        }
        ui.set_opacity(self.root, self.opacity.value.clamp(0.0, 1.0));
        // Icons are rasterized for -180..180 degrees: keep the angle in that range.
        let turn = 360.0 / config.media.loading_spin_seconds.max(0.2);
        self.angle += turn * dt;
        if self.angle > 180.0 {
            self.angle -= 360.0;
        }
        if let Some(w) = ui.widget_mut(self.glyph) {
            w.rotation = self.angle;
        }
        true
    }
}
