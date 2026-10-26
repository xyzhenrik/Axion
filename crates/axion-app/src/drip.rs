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
//! never detaches. The body first shoots back to a small drop at the anchor's rim on an
//! underdamped spring, so it plunges in and recoils out once (`motion.drain`); once that
//! settles, the drop flows on into the anchor's centre (`motion.drain_absorb`), the point
//! deepest inside the glass it drains into (a drop near that glass's rim would still bulge it
//! through the smooth union), and only then is it removed.
//!
//! Flat mode (`glass.style = "flat"`) has no drops: the shape is simply at its target frame and
//! appears with a quick fade and a slight scale on a critically damped spring (`fade()`), and
//! leaves the same way. Callers read the same `frame()`, `growth()` and `phase()`; only
//! `fade()` is new (the popover and the sheet apply it as their opacity).

use axion_compositor::Rect as Frame;
use axion_config::{Config, SpringSpec};
use axion_physics::{Animated, Spring};
use glam::Vec2;

const SETTLED_PX: f32 = 0.4;
/// Flat mode: a card appears at this share of its size and grows to full size as it fades in.
const FLAT_SCALE_FROM: f32 = 0.96;
/// Flat mode: the card counts as gone / shown at this fade.
const FLAT_FADE_SETTLED: f32 = 0.004;
/// The recoil counts as over (and the drop flows on in) once every edge is this close.
const RECOIL_SETTLED_PX: f32 = 1.5;

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
    /// Closing, after the recoil: the drop flows into the anchor's centre.
    absorbing: bool,
    absorb_spring: Spring,
    /// Closing: the edge facing away from the anchor, and whether it is recoiling out yet.
    far: usize,
    recoiling: bool,
    /// Flat mode (see the module docs): no drip, a fade instead.
    flat: bool,
    fade: Animated<f32>,
}

/// A spring from the config, at the configured speed (Reduce Motion: quick, no overshoot; flat
/// mode: never bouncing).
pub fn spring(config: &Config, spec: SpringSpec) -> Spring {
    let motion = &config.motion;
    if config.glass.reduce_motion {
        Spring::new(0.2 * motion.speed, 1.0)
    } else {
        Spring::new(spec.response * motion.speed, config.glass.damping(spec.damping))
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
            absorbing: false,
            absorb_spring: Spring::SNAPPY,
            far: BOTTOM,
            recoiling: false,
            flat: false,
            fade: Animated::new(0.0, Spring::CRISP),
        }
    }

    /// Flat mode: the card is at its target at once and fades in or out.
    fn start_fade(&mut self, config: &Config, to: f32) {
        self.fade.spring = spring(config, config.motion.snappy);
        self.fade.target = to;
    }

    /// 1 in liquid mode; in flat mode how far the card has faded in (0-1): the opacity of the
    /// shape and its content.
    pub fn fade(&self) -> f32 {
        if self.flat { self.fade.value.clamp(0.0, 1.0) } else { 1.0 }
    }

    fn set_targets(&mut self, frame: Frame) {
        for (edge, value) in self.edges.iter_mut().zip(edges_of(frame)) {
            edge.target = value;
        }
    }

    /// Opening: every edge on the liquid spring, so the shape grows as one.
    fn open_springs(&mut self, config: &Config) {
        let liquid = spring(config, config.motion.liquid);
        self.absorbing = false;
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
        self.flat = config.glass.is_flat();
        if self.phase == Phase::Closed {
            // Flat: it starts where it ends, and only fades.
            let from = if self.flat { target } else { start };
            for (edge, value) in self.edges.iter_mut().zip(edges_of(from)) {
                edge.snap(value);
            }
            self.fade.snap(0.0);
        }
        if self.flat {
            self.start_fade(config, 1.0);
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
                self.flat = config.glass.is_flat();
                if self.flat {
                    self.start_fade(config, 1.0);
                }
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
        self.flat = config.glass.is_flat();
        if self.flat {
            self.start_fade(config, 0.0);
            self.absorbing = true;
            self.phase = Phase::Closing;
            return;
        }
        let near = near_edge(self.anchor, self.frame());
        let (drain, neck) = (spring(config, config.motion.drain), spring(config, config.motion.drain_neck));
        for (i, edge) in self.edges.iter_mut().enumerate() {
            edge.spring = if i == near { neck } else { drain };
        }
        self.absorb_spring = spring(config, config.motion.drain_absorb);
        self.far = (near + 2) % 4;
        self.recoiling = false;
        self.absorbing = config.glass.reduce_motion;
        if self.absorbing {
            self.set_targets(Frame { origin: self.anchor.center(), size: Vec2::ZERO });
        } else {
            self.set_targets(self.rim_drop(near, config.motion.drain_drop));
        }
        self.phase = Phase::Closing;
    }

    /// +1 when the far edge moves away from the anchor with growing values, else −1.
    fn outward(&self) -> f32 {
        if self.far == RIGHT || self.far == BOTTOM { 1.0 } else { -1.0 }
    }

    /// The small drop the body recoils into: just inside the anchor's rim facing the glass,
    /// at most the anchor's size. At rest it is hidden in the anchor's glass; only the
    /// recoil's overshoot bulges out of the rim.
    fn rim_drop(&self, near: usize, size: f32) -> Frame {
        let (a, c) = (self.anchor, self.anchor.center());
        let rim = match near {
            // The glass's left edge faces the anchor: the anchor's right rim.
            LEFT => Vec2::new(a.origin.x + a.size.x, c.y),
            TOP => Vec2::new(c.x, a.origin.y + a.size.y),
            RIGHT => Vec2::new(a.origin.x, c.y),
            _ => Vec2::new(c.x, a.origin.y),
        };
        let size = Vec2::splat(size).min(a.size);
        let inward = (c - rim).normalize_or_zero() * size / 2.0;
        Frame { origin: rim + inward - size / 2.0, size }
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
        if self.flat {
            self.fade.step(dt);
            let glided = self.edges.iter().all(|e| e.is_settled(SETTLED_PX));
            self.phase = match self.phase {
                Phase::Opening if glided && self.fade.is_settled(FLAT_FADE_SETTLED) => Phase::Open,
                Phase::Closing if self.fade.value <= FLAT_FADE_SETTLED => Phase::Closed,
                phase => phase,
            };
            return self.phase;
        }
        if self.phase == Phase::Closing && !self.absorbing {
            // Away from the anchor is +x/+y for the right/bottom edge, −x/−y for left/top.
            if self.edges[self.far].velocity * self.outward() > 0.0 {
                self.recoiling = true;
            }
        }
        let recoil_over = self.recoiling && self.edges[self.far].velocity * self.outward() <= 0.0;
        if self.phase == Phase::Closing
            && !self.absorbing
            && (recoil_over || self.edges.iter().all(|e| e.is_settled(RECOIL_SETTLED_PX)))
        {
            // The recoil peaked: the drop flows on into the anchor from there.
            self.absorbing = true;
            for edge in &mut self.edges {
                edge.spring = self.absorb_spring;
            }
            self.set_targets(Frame { origin: self.anchor.center(), size: Vec2::ZERO });
        }
        let settled = self.edges.iter().all(|e| e.is_settled(SETTLED_PX));
        self.phase = match self.phase {
            Phase::Opening if settled => Phase::Open,
            Phase::Closing if settled && self.absorbing => Phase::Closed,
            phase => phase,
        };
        self.phase
    }

    /// The glass's current rectangle (never inside out, should an edge overshoot another).
    pub fn frame(&self) -> Frame {
        let e = |i: usize| self.edges[i].value;
        let lo = Vec2::new(e(LEFT).min(e(RIGHT)), e(TOP).min(e(BOTTOM)));
        let hi = Vec2::new(e(LEFT).max(e(RIGHT)), e(TOP).max(e(BOTTOM)));
        let frame = Frame { origin: lo, size: (hi - lo).max(Vec2::ONE) };
        if !self.flat {
            return frame;
        }
        // Flat: a slight scale about the centre as it fades.
        let k = FLAT_SCALE_FROM + (1.0 - FLAT_SCALE_FROM) * self.fade.value.clamp(0.0, 1.0);
        Frame { origin: frame.center() - frame.size * k / 2.0, size: frame.size * k }
    }

    /// How far it has grown towards its target (0 at the anchor, 1 there; by the smaller axis).
    pub fn growth(&self) -> f32 {
        if self.flat {
            return if self.phase == Phase::Closed { 0.0 } else { self.fade() };
        }
        let travel = (self.target.size - self.anchor.size).abs().max(Vec2::ONE);
        let grown = ((self.frame().size - self.anchor.size).max(Vec2::ZERO) / travel).min_element();
        match self.phase {
            Phase::Closed => 0.0,
            _ => grown.clamp(0.0, 1.0),
        }
    }

    /// How far it has grown along the axis it travels furthest on (0 at the anchor, 1 at the
    /// target): unlike [`Drip::growth`] it is not held at 0 by an axis that doesn't grow (a
    /// menu as wide as, or narrower than, the row it flows out of).
    pub fn progress(&self) -> f32 {
        if self.phase == Phase::Closed {
            return 0.0;
        }
        if self.flat {
            return self.fade();
        }
        let travel = (self.target.size - self.anchor.size).abs();
        let axis = if travel.y >= travel.x { 1 } else { 0 };
        let grown = (self.frame().size[axis] - self.anchor.size[axis]) / travel[axis].max(1.0);
        grown.abs().clamp(0.0, 1.0)
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
    fn body_recoils_out_of_the_rim_before_it_is_absorbed() {
        let config = Config::default();
        let mut drip = Drip::new();
        let anchor = Frame::new(300.0, 10.0, 34.0, 34.0);
        let target = Frame::new(100.0, 56.0, 280.0, 300.0);
        drip.open(&config, anchor, target);
        for _ in 0..360 {
            drip.step(1.0 / 120.0);
        }
        drip.close(&config);
        let rim = anchor.origin.y + anchor.size.y;
        // The far (bottom) edge plunges above the rim's drop, then comes back out below it.
        let (mut deepest, mut rebound) = (f32::MAX, f32::MIN);
        while drip.step(1.0 / 120.0) != Phase::Closed {
            let bottom = drip.frame().origin.y + drip.frame().size.y;
            if bottom < deepest {
                deepest = bottom;
            } else if !drip.absorbing {
                rebound = rebound.max(bottom);
            }
        }
        let rest = rim;
        assert!(deepest < rest - 1.0, "no plunge: {deepest} vs {rest}");
        assert!(rebound > deepest + 1.0, "no recoil: {deepest} → {rebound}");
    }

    fn flat_config() -> Config {
        let mut config = Config::default();
        config.glass.style = axion_config::GlassStyle::Flat;
        config
    }

    #[test]
    fn flat_mode_appears_in_place_with_a_fade_and_no_overshoot() {
        let config = flat_config();
        let mut drip = Drip::new();
        let anchor = Frame::new(100.0, 10.0, 34.0, 34.0);
        let target = Frame::new(60.0, 70.0, 280.0, 300.0);
        drip.open(&config, anchor, target);
        // At the target (not the anchor) from the first frame, only a touch smaller, invisible.
        let first = drip.frame();
        assert!((first.center() - target.center()).length() < 0.5);
        assert!(first.size.x > target.size.x * 0.9 && first.size.x < target.size.x);
        assert_eq!(drip.fade(), 0.0);
        let mut last = 0.0;
        for _ in 0..240 {
            drip.step(1.0 / 120.0);
            assert!(drip.fade() >= last - 1e-5 && drip.fade() <= 1.0, "fade {}", drip.fade());
            last = drip.fade();
            // The shape never leaves its target's neighbourhood: no drip.
            let width = drip.frame().size.x;
            assert!(width <= target.size.x + 0.01 && width >= target.size.x * 0.95);
        }
        assert_eq!(drip.phase(), Phase::Open);
        assert!((drip.frame().size - target.size).length() < 0.1);
        assert!((drip.growth() - 1.0).abs() < 1e-3);
        drip.close(&config);
        assert_eq!(drip.phase(), Phase::Closing);
        for _ in 0..240 {
            drip.step(1.0 / 120.0);
        }
        assert_eq!(drip.phase(), Phase::Closed);
        assert_eq!(drip.growth(), 0.0);
    }

    #[test]
    fn liquid_mode_never_fades_the_shape() {
        let config = Config::default();
        let mut drip = Drip::new();
        drip.open(&config, Frame::new(0.0, 0.0, 30.0, 30.0), Frame::new(0.0, 40.0, 200.0, 200.0));
        assert_eq!(drip.fade(), 1.0);
    }

    #[test]
    fn flat_springs_are_never_underdamped() {
        let flat = flat_config();
        for spec in [flat.motion.snappy, flat.motion.liquid, flat.motion.drain, flat.motion.fling] {
            assert!(spring(&flat, spec).damping_ratio >= 1.0);
        }
        let liquid = Config::default();
        assert_eq!(spring(&liquid, liquid.motion.liquid).damping_ratio, liquid.motion.liquid.damping);
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
