//! A glass popover that flows out of the bar or button it belongs to. Its glass drips out of
//! the anchor on the liquid spring (see [`Drip`]) and stays joined to the glass the anchor
//! lies in by a broad liquid bridge: popover, bridge and that glass are glass of the same level
//! in the same plane at the same nesting depth, so they are drawn in one glass pass and the
//! smooth-min merges them into one liquid shape. The bridge spans the whole front the two
//! shapes share (less their rounded corners) and has a wider smooth union of its own
//! (`glass.popover_bridge_fillet`), so the junction is one smooth band with concave fillets
//! only at its outer ends: no stalk, no pinch. The content sits directly on the glass (no
//! plates); axion-ui maps it onto the animated glass and fades it with the glass's growth, so
//! both come and go together.
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
/// A bridge narrower than this (logical px) isn't drawn.
const MIN_BRIDGE_WIDTH: f32 = 1.0;
/// A bridge reaches at most to the middle of a shape.
const MIDDLE: f32 = 0.5;

pub struct Popover {
    parent: NodeId,
    root: NodeId,
    bridge: NodeId,
    drip: Drip,
    radius: f32,
    /// The glass the popover flows out of (frame and corner radius, logical px): the glass
    /// under the anchor's centre, or the anchor itself.
    join: Option<(Frame, f32)>,
    /// How deep the bridge reaches into the shapes it joins (logical px; see [`bridge_frame`]).
    bridge_depth: f32,
    /// At most this share (0..=0.5) of the joined glass's thickness is reached into.
    bridge_into_join: f32,
    /// Content under the popover that would show through it, and fades out as it grows.
    covered: Vec<NodeId>,
    /// The bridge thins out as the popover settles and is gone once it is open: it only
    /// joins it to its anchor while it flows out and back (the find bar).
    detach: bool,
    /// Where it opens to (logical px).
    target: Frame,
}

impl Popover {
    /// Floats free of its anchor once open (joined only while it flows out and back in).
    pub fn detached(mut self) -> Self {
        self.detach = true;
        self
    }

    /// The popover's glass as an absolute child of `parent` (the plane the anchor's glass lies
    /// in: the toolbar, the sidebar). `style` lays out its content (it is made absolute).
    pub fn build(ui: &mut Ui, parent: NodeId, radius: f32, style: Style) -> Self {
        let root = ui.add(
            parent,
            Widget::container()
                .with_background(Background::Glass(LEVEL))
                .with_glass_density(DENSITY)
                .with_radius(radius),
            Style { position: Position::Absolute, ..style },
        );
        // After the popover, so its wider smooth union applies where it meets both the
        // popover and the anchor's glass (it flows into the glass drawn before it). It only
        // joins them: the pointer passes through it and the tint blends across it from theirs.
        let bridge = ui.add(
            parent,
            Widget::container().with_background(Background::Glass(LEVEL)).as_join(),
            Style { position: Position::Absolute, ..Default::default() },
        );
        ui.claim_space(root);
        ui.set_visible(bridge, false);
        ui.set_visible(root, false);
        Self {
            parent,
            root,
            bridge,
            drip: Drip::new(),
            radius,
            join: None,
            bridge_depth: 0.0,
            bridge_into_join: MIDDLE,
            covered: Vec::new(),
            detach: false,
            target: Frame::default(),
        }
    }

    /// Limits how far into the glass it flows out of the bridge reaches (a share of its
    /// thickness, 0.5 = to its middle, the default).
    pub fn set_bridge_into_join(&mut self, share: f32) {
        self.bridge_into_join = share;
    }

    /// The glass panel: the popover's content goes in here.
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn phase(&self) -> Phase {
        self.drip.phase()
    }

    /// The top edge of the glass as it is drawn now (logical px; `None` while closed).
    pub fn top(&self) -> Option<f32> {
        (self.drip.phase() != Phase::Closed).then(|| self.drip.frame().origin.y)
    }

    /// The glass's frame as it is drawn now (`None` while closed).
    pub fn frame(&self) -> Option<Frame> {
        (self.drip.phase() != Phase::Closed).then(|| self.drip.frame())
    }

    /// Open or opening.
    pub fn is_open(&self) -> bool {
        matches!(self.drip.phase(), Phase::Opening | Phase::Open)
    }

    /// Nodes that fade out as the popover grows (the content of the row a context menu flows
    /// out of, which would otherwise poke out of the menu's neck). Set before opening.
    pub fn set_covered_nodes(&mut self, ui: &mut Ui, nodes: Vec<NodeId>) {
        for old in std::mem::take(&mut self.covered) {
            ui.set_opacity(old, 1.0);
        }
        self.covered = nodes;
    }

    /// Drips out of `anchor` to `target` (logical px); if already open it glides there, and if
    /// it is closing it turns around.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, target: Frame) {
        self.target = target;
        ui.set_claim_target(self.root, Some(target));
        crate::menu::place(ui, self.root, self.parent, target);
        ui.set_visible(self.root, true);
        self.find_join(ui, anchor);
        self.bridge_depth = config.glass.bezel_width.max(0.0);
        if let Some(w) = ui.widget_mut(self.bridge) {
            w.glass_merge = Some(config.glass.popover_bridge_fillet());
        }
        self.drip.show(config, anchor, target);
        self.animate(ui, 0.0);
    }

    /// Glides an open popover to `target` (the window or the layout changed).
    pub fn retarget(&mut self, ui: &mut Ui, target: Frame) {
        self.target = target;
        if !self.is_open() {
            return;
        }
        ui.set_claim_target(self.root, Some(target));
        crate::menu::place(ui, self.root, self.parent, target);
        self.drip.retarget(target);
    }

    /// Flows back into `anchor` instead (a submenu, when its whole menu closes).
    pub fn close_into(&mut self, ui: &mut Ui, config: &Config, anchor: Frame) {
        if self.drip.phase() != Phase::Closed {
            ui.set_claim_target(self.root, None);
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
        ui.set_claim_target(self.root, None);
        self.drip.close(config);
        self.animate(ui, 0.0);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) -> Phase {
        let phase = self.drip.step(dt);
        if phase == Phase::Closed {
            if ui.is_visible(self.root) {
                ui.set_visible(self.root, false);
                ui.set_visible(self.bridge, false);
                for &node in &self.covered {
                    ui.set_opacity(node, 1.0);
                }
            }
            return phase;
        }
        // The glass it flows out of may move or resize (the bar while the window changes).
        self.find_join(ui, self.drip.anchor());
        let frame = self.drip.frame();
        ui.set_visual_frame(self.root, Some(frame));
        // Flat mode fades the card in and out instead (1 otherwise: nothing changes).
        ui.set_opacity(self.root, self.drip.fade());
        // Flat mode has no bridges: the card is just a card.
        let mut bridge = self.join.filter(|_| !ui.theme().flat()).and_then(|(join, r)| {
            bridge_frame_reaching(join, r, frame, self.radius, self.bridge_depth, self.bridge_into_join)
        });
        if self.detach {
            // Narrower the further it has flowed away from its anchor, gone when open.
            let from = self.drip.anchor();
            let (start, end) = (from.origin.y + from.size.y, self.target.origin.y);
            let travelled = if end > start { (frame.origin.y - start) / (end - start) } else { 1.0 };
            let keep = (1.0 - travelled).clamp(0.0, 1.0);
            bridge = bridge
                .map(|b| {
                    let width = b.size.x * keep;
                    Frame::new(b.origin.x + (b.size.x - width) / 2.0, b.origin.y, width, b.size.y)
                })
                .filter(|b| b.size.x >= MIN_BRIDGE_WIDTH);
        }
        ui.set_visible(self.bridge, bridge.is_some());
        ui.set_visual_frame(self.bridge, bridge);
        for &node in &self.covered {
            ui.set_opacity(node, 1.0 - self.drip.progress());
        }
        phase
    }

    /// Whether a point (logical px) is inside the open popover.
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.is_open() && ui.frame(self.root).contains(p)
    }

    /// Finds the glass under `anchor`'s centre, else takes the anchor itself (as a capsule).
    fn find_join(&mut self, ui: &Ui, anchor: Frame) {
        let found = ui.glass_at(self.parent, anchor.center(), &[self.root, self.bridge]);
        self.join = Some(found.map_or((anchor, CAPSULE), |(frame, radius, _)| (frame, radius)));
    }
}

/// How far in from its ends a rounded shape `len` long with corner `radius` keeps its full
/// depth: the corner radius on a long edge, nothing on a circle's, smoothly in between.
fn straight_inset(len: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    radius * ((len - 2.0 * radius) / (2.0 * radius)).clamp(0.0, 1.0)
}

/// The bridge joining `glass` to the glass it flows out of, `join` (logical px): across the
/// front the two share on their facing sides (each less its rounded corners, or a round
/// anchor's full width), reaching `depth` into each (at most to its middle, at least to where
/// it spans the whole front), so in the gap the joined glass is as thick as inside them and
/// shows no rim. `None` while they overlap (the drip is still inside) or share no front.
pub fn bridge_frame(
    join: Frame,
    join_radius: f32,
    glass: Frame,
    glass_radius: f32,
    depth: f32,
) -> Option<Frame> {
    bridge_frame_reaching(join, join_radius, glass, glass_radius, depth, MIDDLE)
}

/// [`bridge_frame`] reaching at most `into_join` (0..=1) of the joined glass's thickness into
/// it. A shallow bridge keeps clear of the content of a thin joined shape (a tab's label) whose
/// own glass is too faint to hide the bridge's edge.
pub fn bridge_frame_reaching(
    join: Frame,
    join_radius: f32,
    glass: Frame,
    glass_radius: f32,
    depth: f32,
    into_join: f32,
) -> Option<Frame> {
    let (jmin, jmax) = (join.origin, join.origin + join.size);
    let (gmin, gmax) = (glass.origin, glass.origin + glass.size);
    let gaps = [
        gmin.y - jmax.y, // below
        jmin.y - gmax.y, // above
        gmin.x - jmax.x, // right
        jmin.x - gmax.x, // left
    ];
    let (side, gap) = gaps.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if gap <= 0.0 {
        return None;
    }
    let jr = join_radius.min(join.size.min_element() / 2.0).max(0.0);
    let gr = glass_radius.min(glass.size.min_element() / 2.0).max(0.0);
    // Along the front (x for a popover above or below), then across it.
    let vertical = side < 2;
    let pick = |v: Vec2| if vertical { (v.x, v.y) } else { (v.y, v.x) };
    let ((jlo, jnear_min), (jhi, jnear_max)) = (pick(jmin), pick(jmax));
    let ((glo, gnear_min), (ghi, gnear_max)) = (pick(gmin), pick(gmax));
    let (jlen, glen) = (jhi - jlo, ghi - glo);
    let (ij, ig) = (straight_inset(jlen, jr), straight_inset(glen, gr));
    let lo = (jlo + ij).max(glo + ig);
    let hi = (jhi - ij).min(ghi - ig);
    if hi - lo < MIN_BRIDGE_WIDTH {
        return None;
    }
    // Into each shape at least until it is as wide as the bridge (down to the widest line of a
    // round end), never past its middle.
    let reach = |thickness: f32, r: f32, inset: f32, share: f32| {
        depth.max(r - inset).min(thickness * share.clamp(0.0, MIDDLE))
    };
    let (jt, gt) = (jnear_max - jnear_min, gnear_max - gnear_min);
    let (dj, dg) = (reach(jt, jr, ij, into_join), reach(gt, gr, ig, MIDDLE));
    let (a, b) = if side % 2 == 0 {
        // The glass lies after the join (below, right).
        (jnear_max - dj, gnear_min + dg)
    } else {
        (gnear_max - dg, jnear_min + dj)
    };
    Some(if vertical { Frame::new(lo, a, hi - lo, b - a) } else { Frame::new(a, lo, b - a, hi - lo) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bezel width: how deep bridges reach.
    const DEPTH: f32 = 22.0;

    #[test]
    fn bridge_spans_the_popover_under_a_wide_bar() {
        // The address field (a capsule) and the page menu below it, reaching past its end.
        let bar = Frame::new(100.0, 10.0, 500.0, 36.0);
        let menu = Frame::new(338.0, 60.0, 280.0, 300.0);
        let b = bridge_frame(bar, CAPSULE, menu, 22.0, DEPTH).expect("a bridge below");
        assert_eq!(b.origin.x, menu.origin.x + 22.0);
        // Stops where the bar's round end begins.
        assert_eq!(b.origin.x + b.size.x, bar.origin.x + bar.size.x - 18.0);
        // From the middle of the bar to a bezel's width into the menu.
        assert_eq!(b.origin.y, bar.center().y);
        assert_eq!(b.origin.y + b.size.y, menu.origin.y + DEPTH);
    }

    #[test]
    fn shallow_bridge_keeps_clear_of_a_thin_tabs_label() {
        // A 30 px tab with its menu below: the usual bridge starts at the tab's middle (where
        // its label is), a shallow one lower down, but still inside the tab.
        let tab = Frame::new(100.0, 74.0, 260.0, 30.0);
        let menu = Frame::new(100.0, 118.0, 280.0, 200.0);
        let deep = bridge_frame(tab, CAPSULE, menu, 22.0, DEPTH).expect("deep");
        let shallow = bridge_frame_reaching(tab, CAPSULE, menu, 22.0, DEPTH, 0.3).expect("shallow");
        assert_eq!(deep.origin.y, tab.center().y);
        assert_eq!(shallow.origin.y, tab.origin.y + tab.size.y - 0.3 * tab.size.y);
        assert!(shallow.origin.y > deep.origin.y && shallow.origin.y < tab.origin.y + tab.size.y);
        // Same width, and it ends at the same depth in the menu.
        assert_eq!((shallow.origin.x, shallow.size.x), (deep.origin.x, deep.size.x));
        assert_eq!(shallow.origin.y + shallow.size.y, deep.origin.y + deep.size.y);
    }

    #[test]
    fn bridge_spans_a_round_button() {
        // A circle button, the menu below: the bridge is as wide as the button and starts at
        // its widest line.
        let button = Frame::new(300.0, 10.0, 34.0, 34.0);
        let menu = Frame::new(60.0, 60.0, 300.0, 300.0);
        let b = bridge_frame(button, CAPSULE, menu, 22.0, DEPTH).expect("a bridge below");
        assert_eq!((b.origin.x, b.size.x), (300.0, 34.0));
        assert_eq!(b.origin.y, button.center().y);
    }

    #[test]
    fn bridge_to_a_side_submenu_spans_the_shared_height() {
        let menu = Frame::new(100.0, 100.0, 280.0, 400.0);
        let sub = Frame::new(392.0, 200.0, 200.0, 100.0);
        let b = bridge_frame(menu, 22.0, sub, 22.0, DEPTH).expect("a bridge to the right");
        assert_eq!((b.origin.y, b.size.y), (222.0, 56.0));
        assert_eq!(b.origin.x, 380.0 - DEPTH);
        assert_eq!(b.origin.x + b.size.x, 392.0 + DEPTH);
        // And above, as for the account menu over its button.
        let button = Frame::new(20.0, 600.0, 240.0, 44.0);
        let above = Frame::new(20.0, 400.0, 240.0, 186.0);
        let b = bridge_frame(button, CAPSULE, above, 22.0, DEPTH).expect("a bridge above");
        assert_eq!((b.origin.x, b.size.x), (42.0, 196.0));
        assert_eq!(b.origin.y + b.size.y, button.center().y);
    }

    #[test]
    fn no_bridge_while_overlapping_or_apart() {
        let anchor = Frame::new(100.0, 100.0, 40.0, 40.0);
        assert!(bridge_frame(anchor, CAPSULE, anchor, 20.0, DEPTH).is_none());
        // Below, but shifted off to the side: no shared front.
        assert!(bridge_frame(anchor, CAPSULE, Frame::new(300.0, 160.0, 100.0, 100.0), 20.0, DEPTH).is_none());
    }

    #[test]
    fn bridge_grows_with_the_popover() {
        let bar = Frame::new(100.0, 10.0, 500.0, 36.0);
        let narrow =
            bridge_frame(bar, CAPSULE, Frame::new(300.0, 50.0, 60.0, 40.0), 22.0, DEPTH).expect("narrow");
        let wide =
            bridge_frame(bar, CAPSULE, Frame::new(200.0, 60.0, 280.0, 300.0), 22.0, DEPTH).expect("wide");
        assert!(narrow.size.x < wide.size.x);
    }
}
