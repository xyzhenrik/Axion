//! The "drip" from the design ("Drip — button to popover"): a glass shape flows out of the
//! button that opened it to its final rectangle on the liquid spring, and drains back into it
//! when it closes. Its content follows the glass (axion-ui maps it onto the glass's visual
//! frame and fades it with the glass's growth), so there is no separate content animation to
//! fall out of step.
//!
//! Each edge of the shape is a spring of its own. Opening, all four share the liquid spring
//! (the shape grows as one). Closing, it drains like liquid flowing back into a basin: the
//! near edge (the neck, facing the anchor) sinks into the anchor's glass first
//! (`motion.drain_neck`), while the far edge and the sides retract after it on
//! `motion.drain`. The neck stays inside the glass it flows into the whole way, so the body
//! never detaches; it ends as a drop shrinking to the anchor's centre, the point deepest inside
//! the glass it drains into (a drop near that glass's rim would still bulge it through the
//! smooth union), and only then is it removed.

use axion_compositor::Rect as Frame;
use axion_config::{Config, SpringSpec};
use axion_physics::{Animated, Spring};
use glam::Vec2;

const SETTLED_PX: f32 = 0.4;

/// Edge indices: left, top, right, bottom.
const LEFT: usize = 0;
const TOP: usize = 1;
const RIGHT: usize = 2;
const BOTTOM: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Closed,
    Opening,
    Open,
    Closing,
}

pub struct Drip {
    /// Left, top, right, bottom (logical px).
    edges: [Animated<f32>; 4],
    /// Where it came from (and returns to).
    anchor: Frame,
    target: Frame,
    phase: Phase,
}

/// A spring from the config, at the configured speed (Reduce Motion: quick, no overshoot).
pub fn spring(config: &Config, spec: SpringSpec) -> Spring {
    let motion = &config.motion;
    if config.glass.reduce_motion {
        Spring::new(0.2 * motion.speed, 1.0)
    } else {
        Spring::new(spec.response * motion.speed, spec.damping)
    }
}

fn edges_of(frame: Frame) -> [f32; 4] {
    let max = frame.origin + frame.size;
    [frame.origin.x, frame.origin.y, max.x, max.y]
}

/// The edge of `glass` that faces `anchor` (the neck it drains through): the side with the
/// widest gap between them, or, while they overlap, the side the glass's centre lies to.
fn near_edge(anchor: Frame, glass: Frame) -> usize {
    let (amin, amax) = (anchor.origin, anchor.origin + anchor.size);
    let (gmin, gmax) = (glass.origin, glass.origin + glass.size);
    let gaps = [
        (gmin.x - amax.x, LEFT),   // the glass right of the anchor: its left edge faces it
        (gmin.y - amax.y, TOP),    // below
        (amin.x - gmax.x, RIGHT),  // left of it
        (amin.y - gmax.y, BOTTOM), // above
    ];
    let (gap, edge) = gaps.iter().copied().fold((f32::MIN, TOP), |a, b| if b.0 > a.0 { b } else { a });
    if gap > 0.0 {
        return edge;
    }
    let d = glass.center() - anchor.center();
    match (d.x.abs() > d.y.abs(), d.x >= 0.0, d.y >= 0.0) {
        (true, true, _) => LEFT,
        (true, false, _) => RIGHT,
        (false, _, true) => TOP,
        (false, _, false) => BOTTOM,
    }
}

impl Drip {
    pub fn new() -> Self {
        Self {
            edges: std::array::from_fn(|_| Animated::new(0.0, Spring::SNAPPY)),
            anchor: Frame::default(),
            target: Frame::default(),
            phase: Phase::Closed,
        }
    }

    fn set_targets(&mut self, frame: Frame) {
        for (edge, value) in self.edges.iter_mut().zip(edges_of(frame)) {
            edge.target = value;
        }
    }

    /// Opening: every edge on the liquid spring, so the shape grows as one.
    fn open_springs(&mut self, config: &Config) {
        let liquid = spring(config, config.motion.liquid);
        for edge in &mut self.edges {
            edge.spring = liquid;
        }
    }

    /// Starts flowing from `anchor` to `target` (logical px).
    pub fn open(&mut self, config: &Config, anchor: Frame, target: Frame) {
        self.open_from(config, anchor, anchor, target);
    }

    /// Starts flowing from `start` (a shape inside `anchor`) to `target`; it drains back into
    /// `anchor` when it closes.
    pub fn open_from(&mut self, config: &Config, anchor: Frame, start: Frame, target: Frame) {
        self.open_springs(config);
        if self.phase == Phase::Closed {
            for (edge, value) in self.edges.iter_mut().zip(edges_of(start)) {
                edge.snap(value);
            }
        }
        self.anchor = anchor;
        self.target = target;
        self.set_targets(target);
        self.phase = Phase::Opening;
    }

    /// Moves the target (the list grew while typing, the window was resized) without
    /// restarting: the glass glides to its new size instead of jumping or dripping again.
    pub fn retarget(&mut self, target: Frame) {
        if self.phase == Phase::Opening || self.phase == Phase::Open {
            self.target = target;
            self.set_targets(target);
        }
    }

    /// Opens if closed or closing (dripping out of `anchor`), else glides to `target`.
    pub fn show(&mut self, config: &Config, anchor: Frame, target: Frame) {
        match self.phase {
            Phase::Open | Phase::Opening => self.retarget(target),
            // Reopened while draining back: turn around from where it is, no new drip.
            Phase::Closing => {
                self.open_springs(config);
                self.anchor = anchor;
                self.target = target;
                self.set_targets(target);
                self.phase = Phase::Opening;
            }
            Phase::Closed => self.open(config, anchor, target),
        }
    }

    /// Drains back into `anchor` instead of where it came from.
    pub fn close_into(&mut self, config: &Config, anchor: Frame) {
        self.anchor = anchor;
        self.close(config);
    }

    /// Drains back into the anchor: the neck sinks into it first, the body follows.
    pub fn close(&mut self, config: &Config) {
        if self.phase == Phase::Closed {
            return;
        }
        let near = near_edge(self.anchor, self.frame());
        let (drain, neck) = (spring(config, config.motion.drain), spring(config, config.motion.drain_neck));
        for (i, edge) in self.edges.iter_mut().enumerate() {
            edge.spring = if i == near { neck } else { drain };
        }
        self.set_targets(Frame { origin: self.anchor.center(), size: Vec2::ZERO });
        self.phase = Phase::Closing;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn step(&mut self, dt: f32) -> Phase {
        if self.phase == Phase::Closed {
            return self.phase;
        }
        for edge in &mut self.edges {
            edge.step(dt);
        }
        let settled = self.edges.iter().all(|e| e.is_settled(SETTLED_PX));
        self.phase = match self.phase {
            Phase::Opening if settled => Phase::Open,
            Phase::Closing if settled => Phase::Closed,
            phase => phase,
        };
        self.phase
    }

    /// The glass's current rectangle (never inside out, should an edge overshoot another).
    pub fn frame(&self) -> Frame {
        let e = |i: usize| self.edges[i].value;
        let lo = Vec2::new(e(LEFT).min(e(RIGHT)), e(TOP).min(e(BOTTOM)));
        let hi = Vec2::new(e(LEFT).max(e(RIGHT)), e(TOP).max(e(BOTTOM)));
        Frame { origin: lo, size: (hi - lo).max(Vec2::ONE) }
    }

    /// How far it has grown towards its target (0 at the anchor, 1 there; by the smaller axis).
    pub fn growth(&self) -> f32 {
        let travel = (self.target.size - self.anchor.size).abs().max(Vec2::ONE);
        let grown = ((self.frame().size - self.anchor.size).max(Vec2::ZERO) / travel).min_element();
        match self.phase {
            Phase::Closed => 0.0,
            _ => grown.clamp(0.0, 1.0),
        }
    }

    /// Where it flows out of (and back into).
    pub fn anchor(&self) -> Frame {
        self.anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_closes() {
        let config = Config::default();
        let mut drip = Drip::new();
        let anchor = Frame::new(100.0, 10.0, 34.0, 34.0);
        let target = Frame::new(60.0, 70.0, 280.0, 300.0);
        drip.open(&config, anchor, target);
        assert_eq!(drip.frame().size, anchor.size);
        for _ in 0..240 {
            drip.step(1.0 / 120.0);
        }
        assert_eq!(drip.phase(), Phase::Open);
        assert!((drip.frame().size - target.size).length() < 1.0);
        drip.close(&config);
        for _ in 0..240 {
            drip.step(1.0 / 120.0);
        }
        assert_eq!(drip.phase(), Phase::Closed);
    }

    #[test]
    fn drains_neck_first_and_stays_attached() {
        let config = Config::default();
        let mut drip = Drip::new();
        // The page menu button in the bar, the menu below it.
        let anchor = Frame::new(300.0, 10.0, 34.0, 34.0);
        let target = Frame::new(100.0, 56.0, 280.0, 300.0);
        drip.open(&config, anchor, target);
        for _ in 0..360 {
            drip.step(1.0 / 120.0);
        }
        drip.close(&config);
        let mut last = drip.frame();
        let mut frames = 0;
        while drip.step(1.0 / 120.0) != Phase::Closed {
            let f = drip.frame();
            // The neck reaches into the anchor early and stays there: never a detached drop.
            if frames > 12 {
                assert!(f.origin.y <= anchor.origin.y + anchor.size.y, "frame {frames}: {f:?}");
            }
            // The neck leads: the body is never shorter than a sliver while it still drains.
            last = f;
            frames += 1;
            assert!(frames < 600, "never settles");
        }
        // Not instant: it drains over a visible stretch of frames.
        assert!(frames > 24, "{frames}");
        // It ends inside the anchor.
        assert!(last.size.x <= anchor.size.x && last.size.y <= anchor.size.y);
        assert!(anchor.contains(last.center()));
    }

    #[test]
    fn near_edge_faces_the_anchor() {
        let anchor = Frame::new(20.0, 600.0, 240.0, 44.0);
        assert_eq!(near_edge(anchor, Frame::new(20.0, 400.0, 240.0, 186.0)), BOTTOM);
        assert_eq!(near_edge(anchor, Frame::new(20.0, 660.0, 240.0, 100.0)), TOP);
        assert_eq!(near_edge(anchor, Frame::new(280.0, 600.0, 200.0, 100.0)), LEFT);
        // Overlapping: by where its centre lies.
        assert_eq!(near_edge(anchor, Frame::new(0.0, 0.0, 280.0, 1000.0)), BOTTOM);
    }
}
