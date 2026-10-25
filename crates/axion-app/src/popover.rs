//! A glass popover that flows out of the bar or button it belongs to. Its glass drips out of
//! the anchor on the liquid spring (see [`Drip`]) and stays joined to the anchor's glass by a
//! narrow neck: popover, neck and anchor are glass of the same level in the same plane at the
//! same nesting depth, so they are drawn in one glass pass and the smooth-min merges them into
//! one liquid shape. The content sits directly on the glass (no plates); axion-ui maps it onto
//! the animated glass and fades it with the glass's growth, so both come and go together.
//!
//! The page menu, account menu (and its submenus), Shields and downloads popovers, address
//! suggestions and the password prompt are built on it.

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, Ui, Widget};
use glam::Vec2;

use crate::drip::{Drip, Phase};

/// The bar's glass level: popovers are drawn in its pass, so they merge with it …
pub const LEVEL: GlassLevel = GlassLevel::Thin;
/// … but are tinted as densely as thick glass, so text directly on them stays legible on
/// any page (light, dark or busy).
pub const DENSITY: GlassLevel = GlassLevel::Thick;
/// The neck needs at least this much length (logical px) before it is drawn.
const MIN_NECK_LENGTH: f32 = 1.0;

pub struct Popover {
    parent: NodeId,
    root: NodeId,
    neck: NodeId,
    drip: Drip,
    radius: f32,
    neck_width: f32,
    /// A button without glass of its own that becomes glass while the popover is open, so the
    /// popover can flow out of it (the account button), with its background from before.
    anchor_glass: Option<(NodeId, Background)>,
    /// Glass of the same pass that the open popover lies over (the Now Playing card under
    /// the account menu): it would merge into the popover and its content would show through,
    /// so it fades out as the popover grows.
    covered: Option<NodeId>,
}

impl Popover {
    /// The popover's glass as an absolute child of `parent` (the plane the anchor's glass lies
    /// in: the toolbar, the sidebar). `style` lays out its content (it is made absolute).
    pub fn build(ui: &mut Ui, parent: NodeId, radius: f32, style: Style) -> Self {
        let neck = ui.add(
            parent,
            Widget::container().with_background(Background::Glass(LEVEL)).with_radius(CAPSULE),
            Style { position: Position::Absolute, ..Default::default() },
        );
        let root = ui.add(
            parent,
            Widget::container()
                .with_background(Background::Glass(LEVEL))
                .with_glass_density(DENSITY)
                .with_radius(radius),
            Style { position: Position::Absolute, ..style },
        );
        ui.set_visible(neck, false);
        ui.set_visible(root, false);
        Self {
            parent,
            root,
            neck,
            drip: Drip::new(),
            radius,
            neck_width: 0.0,
            anchor_glass: None,
            covered: None,
        }
    }

    /// The glass panel: the popover's content goes in here.
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn phase(&self) -> Phase {
        self.drip.phase()
    }

    /// Open or opening.
    pub fn is_open(&self) -> bool {
        matches!(self.drip.phase(), Phase::Opening | Phase::Open)
    }

    /// While open, `node` (a button without glass) turns into glass, so the popover flows out
    /// of it. Set before opening.
    pub fn set_anchor_glass(&mut self, ui: &mut Ui, node: NodeId) {
        if self.anchor_glass.is_none_or(|(n, _)| n != node) {
            self.restore_anchor(ui);
            let background = ui.widget(node).map_or(Background::None, |w| w.background);
            self.anchor_glass = Some((node, background));
        }
    }

    /// See [`Popover::covered`]. Set before opening.
    pub fn set_covered(&mut self, ui: &mut Ui, node: Option<NodeId>) {
        if let Some(old) = self.covered.take() {
            ui.set_opacity(old, 1.0);
        }
        self.covered = node;
    }

    /// Drips out of `anchor` to `target` (logical px); if already open it glides there, and if
    /// it is closing it turns around.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, target: Frame) {
        crate::menu::place(ui, self.root, self.parent, target);
        ui.set_visible(self.root, true);
        if let Some((node, _)) = self.anchor_glass
            && let Some(w) = ui.widget_mut(node)
        {
            w.background = Background::Glass(LEVEL);
        }
        self.neck_width = config.glass.popover_neck.max(0.0);
        if let Some(neck) = neck_frame(anchor, target, self.neck_width, self.radius) {
            crate::menu::place(ui, self.neck, self.parent, neck);
        }
        self.drip.show(config, anchor, target);
        self.animate(ui, 0.0);
    }

    /// Glides an open popover to `target` (the window or the layout changed).
    pub fn retarget(&mut self, ui: &mut Ui, target: Frame) {
        if !self.is_open() {
            return;
        }
        crate::menu::place(ui, self.root, self.parent, target);
        if let Some(neck) = neck_frame(self.drip.anchor(), target, self.neck_width, self.radius) {
            crate::menu::place(ui, self.neck, self.parent, neck);
        }
        self.drip.retarget(target);
    }

    /// Flows back into `anchor` instead (a submenu, when its whole menu closes).
    pub fn close_into(&mut self, ui: &mut Ui, config: &Config, anchor: Frame) {
        if self.drip.phase() != Phase::Closed {
            self.drip.close_into(config, anchor);
            self.animate(ui, 0.0);
        }
    }

    /// Where the popover flows out of.
    pub fn anchor(&self) -> Frame {
        self.drip.anchor()
    }

    /// Flows back into the anchor.
    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.drip.close(config);
        self.animate(ui, 0.0);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) -> Phase {
        let phase = self.drip.step(dt);
        if phase == Phase::Closed {
            if ui.is_visible(self.root) {
                ui.set_visible(self.root, false);
                ui.set_visible(self.neck, false);
                self.restore_anchor(ui);
                if let Some(node) = self.covered {
                    ui.set_opacity(node, 1.0);
                }
            }
            return phase;
        }
        let frame = self.drip.frame();
        ui.set_visual_frame(self.root, Some(frame));
        let neck = neck_frame(self.drip.anchor(), frame, self.neck_width, self.radius);
        ui.set_visible(self.neck, neck.is_some());
        ui.set_visual_frame(self.neck, neck);
        if let Some(node) = self.covered {
            ui.set_opacity(node, 1.0 - self.drip.growth());
        }
        phase
    }

    /// Whether a point (logical px) is inside the open popover.
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.is_open() && ui.frame(self.root).contains(p)
    }

    fn restore_anchor(&mut self, ui: &mut Ui) {
        if let Some((node, background)) = self.anchor_glass
            && let Some(w) = ui.widget_mut(node)
        {
            w.background = background;
        }
    }
}

/// The neck joining `glass` to its `anchor` (logical px): a capsule `width` wide from the
/// anchor's centre to `radius` inside the glass's nearest edge, kept off the glass's rounded
/// corners. `None` while they overlap (the drip is still inside the anchor) or without a neck.
pub fn neck_frame(anchor: Frame, glass: Frame, width: f32, radius: f32) -> Option<Frame> {
    if width <= 0.0 {
        return None;
    }
    let a = anchor.center();
    let (min, max) = (glass.origin, glass.origin + glass.size);
    let gaps = [
        min.y - (anchor.origin.y + anchor.size.y), // below
        anchor.origin.y - max.y,                   // above
        min.x - (anchor.origin.x + anchor.size.x), // right
        anchor.origin.x - max.x,                   // left
    ];
    let (side, gap) = gaps.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if gap <= 0.0 {
        return None;
    }
    let inset = radius.min(glass.size.min_element() / 2.0);
    // Along the edge, the neck stays on its straight part.
    let across = |lo: f32, hi: f32, at: f32| {
        let (lo, hi) = (lo + inset + width / 2.0, hi - inset - width / 2.0);
        if lo <= hi { at.clamp(lo, hi) } else { (lo + hi) / 2.0 }
    };
    let frame = match side {
        0 | 1 => {
            let x = across(min.x, max.x, a.x);
            let (y0, y1) = if side == 0 { (a.y, min.y + inset) } else { (max.y - inset, a.y) };
            Frame::new(x - width / 2.0, y0, width, y1 - y0)
        }
        _ => {
            let y = across(min.y, max.y, a.y);
            let (x0, x1) = if side == 2 { (a.x, min.x + inset) } else { (max.x - inset, a.x) };
            Frame::new(x0, y - width / 2.0, x1 - x0, width)
        }
    };
    (frame.size.min_element() >= MIN_NECK_LENGTH).then_some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neck_hangs_from_the_anchor() {
        let anchor = Frame::new(300.0, 10.0, 34.0, 34.0);
        let glass = Frame::new(60.0, 60.0, 280.0, 300.0);
        let neck = neck_frame(anchor, glass, 16.0, 22.0).expect("a neck below");
        assert_eq!(neck.size.x, 16.0);
        assert_eq!(neck.origin.y, anchor.center().y);
        assert_eq!(neck.origin.y + neck.size.y, glass.origin.y + 22.0);
        // Kept off the rounded corner.
        assert!(neck.origin.x + neck.size.x <= glass.origin.x + glass.size.x - 22.0);
    }

    #[test]
    fn neck_to_the_side_and_above() {
        let anchor = Frame::new(100.0, 100.0, 40.0, 40.0);
        let right = neck_frame(anchor, Frame::new(150.0, 80.0, 120.0, 100.0), 16.0, 20.0).expect("side");
        assert_eq!(right.size.y, 16.0);
        assert_eq!(right.origin.x, anchor.center().x);
        let above = neck_frame(anchor, Frame::new(80.0, 0.0, 200.0, 80.0), 16.0, 20.0).expect("above");
        assert_eq!(above.origin.y + above.size.y, anchor.center().y);
    }

    #[test]
    fn no_neck_while_overlapping() {
        let anchor = Frame::new(100.0, 100.0, 40.0, 40.0);
        assert!(neck_frame(anchor, anchor, 16.0, 20.0).is_none());
        assert!(neck_frame(anchor, Frame::new(100.0, 150.0, 40.0, 40.0), 0.0, 20.0).is_none());
    }
}
