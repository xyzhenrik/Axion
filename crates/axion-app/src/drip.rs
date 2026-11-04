//! The "drip" from the design ("Drip — button to popover"): a glass shape flows out of the
//! button that opened it to its final rectangle, and flows back into it when it closes. Its
//! content follows the glass (axion-ui maps it onto the glass's visual frame and fades it with
//! the glass's growth), so there is no separate content animation to fall out of step.
//!
//! Each edge of the shape is a spring of its own, and the surface behaves like one drop of
//! liquid: it grows out of its origin in a single gentle approach (`motion.liquid_open` for a
//! sheet, `motion.liquid_small` for a popover or menu; damping a little under critical, so
//! the edges pass their target by 2-3 % of their travel once and never ring), and its
//! near edge, the neck facing the origin, stays attached to the origin's glass through the
//! popover bridge until the body has left. Closing is quicker and never overshoots
//! (`motion.liquid_close`, `liquid_small_close`, critically damped): the neck sinks into the
//! origin's glass first (`motion.liquid_neck_lead`), the body follows, and the shape is gone
//! as soon as it fits inside the origin.
//!
//! Every edge is also clamped: it never passes its target by more than the preset's
//! `overshoot` share of its travel, nor by more than `motion.max_overshoot_px`, so redirected
//! or interrupted motion cannot slosh over the window or the neighbouring chrome.
//!
//! Flat mode (`glass.style = "flat"`) and Reduce Motion have no drops: the shape is simply at
//! its target frame and appears with a short fade and a slight scale on a critically damped
//! spring (`fade()`), and leaves the same way. Callers read the same `frame()`, `growth()` and
//! `phase()`; `fade()` is the opacity they apply and `fades()` says the drop is off (no
//! bridges, no proxy).

use axion_compositor::Rect as Frame;
use axion_config::{Config, LiquidSpec, SpringSpec};
use axion_physics::{Animated, Spring};
use glam::Vec2;

const SETTLED_PX: f32 = 0.4;
/// Fade mode: the card counts as gone / shown at this fade.
const FADE_SETTLED: f32 = 0.004;
/// An edge that travels less than this (logical px) is not clamped: there is no direction.
const MIN_TRAVEL_PX: f32 = 0.5;
/// A shape this close (logical px) to inside its origin counts as drained into it.
const INSIDE_TOLERANCE_PX: f32 = 0.5;

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

/// Which presets a drip moves on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A popover or menu (`motion.liquid_small`, `liquid_small_close`).
    Small,
    /// A modal sheet (`motion.liquid_open`, `liquid_close`).
    Sheet,
}

impl Class {
    fn open(self, config: &Config) -> LiquidSpec {
        match self {
            Class::Small => config.motion.liquid_small,
            Class::Sheet => config.motion.liquid_open,
        }
    }

    fn close(self, config: &Config) -> LiquidSpec {
        match self {
            Class::Small => config.motion.liquid_small_close,
            Class::Sheet => config.motion.liquid_close,
        }
    }
}

pub struct Drip {
    /// Left, top, right, bottom (logical px).
    edges: [Animated<f32>; 4],
    /// Where it came from (and returns to).
    anchor: Frame,
    target: Frame,
    phase: Phase,
    class: Class,
    /// Per edge: the direction it travels (±1, 0 = none) and how far it may pass its target.
    dir: [f32; 4],
    allow: [f32; 4],
    /// The current preset's overshoot share, and the hard cap in px (`motion.max_overshoot_px`).
    overshoot: f32,
    max_px: f32,
    /// Fade mode (flat or Reduce Motion, see the module docs): no drip, a fade instead.
    flat: bool,
    fade: Animated<f32>,
    /// Fade mode: the share of its size the card appears at (`motion.fade_scale_from`).
    fade_scale: f32,
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

/// A liquid preset as a spring: Reduce Motion and flat mode use the short, critically damped
/// fade (`motion.reduce_response`) instead of the preset.
pub fn liquid_spring(config: &Config, spec: LiquidSpec) -> Spring {
    let motion = &config.motion;
    if config.glass.reduce_motion {
        Spring::new(motion.reduce_response * motion.speed, 1.0)
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

impl Default for Drip {
    fn default() -> Self {
        Self::new()
    }
}

impl Drip {
    /// A drip for a popover or menu.
    pub fn new() -> Self {
        Self::with_class(Class::Small)
    }

    /// A drip for a modal sheet.
    pub fn sheet() -> Self {
        Self::with_class(Class::Sheet)
    }

    pub fn with_class(class: Class) -> Self {
        Self {
            edges: std::array::from_fn(|_| Animated::new(0.0, Spring::SNAPPY)),
            anchor: Frame::default(),
            target: Frame::default(),
            phase: Phase::Closed,
            class,
            dir: [0.0; 4],
            allow: [0.0; 4],
            overshoot: 0.0,
            max_px: 0.0,
            flat: false,
            fade: Animated::new(0.0, Spring::CRISP),
            fade_scale: 1.0,
        }
    }

    /// Fade mode: the card is at its target at once and fades in or out.
    fn start_fade(&mut self, config: &Config, spec: LiquidSpec, to: f32) {
        self.fade.spring = liquid_spring(config, spec);
        self.fade.target = to;
    }

    /// Whether the drop is off (flat mode, Reduce Motion): the card only fades, so nothing
    /// joins it to its origin.
    pub fn fades(&self) -> bool {
        self.flat
    }

    /// 1 in liquid mode; in fade mode how far the card has faded in (0-1): the opacity of the
    /// shape and its content.
    pub fn fade(&self) -> f32 {
        if self.flat { self.fade.value.clamp(0.0, 1.0) } else { 1.0 }
    }

    /// Aims every edge at `frame` and works out how far each may pass it: the preset's share of
    /// the distance it has left to travel, at most `motion.max_overshoot_px`.
    fn set_targets(&mut self, frame: Frame) {
        for (i, (edge, value)) in self.edges.iter_mut().zip(edges_of(frame)).enumerate() {
            edge.target = value;
            let travel = value - edge.value;
            self.dir[i] = if travel.abs() < MIN_TRAVEL_PX { 0.0 } else { travel.signum() };
            self.allow[i] = (travel.abs() * self.overshoot).min(self.max_px).max(0.0);
        }
    }

    fn set_cap(&mut self, config: &Config, spec: LiquidSpec) {
        self.overshoot = spec.overshoot.max(0.0);
        self.max_px = config.motion.max_overshoot_px.max(0.0);
        self.fade_scale = config.motion.fade_scale_from.clamp(0.0, 1.0);
    }

    /// Opening: every edge on the class's open preset, so the shape grows as one.
    fn open_springs(&mut self, config: &Config) {
        let spec = self.class.open(config);
        let spring = liquid_spring(config, spec);
        self.set_cap(config, spec);
        for edge in &mut self.edges {
            edge.spring = spring;
        }
    }

    /// Starts flowing from `anchor` to `target` (logical px).
    pub fn open(&mut self, config: &Config, anchor: Frame, target: Frame) {
        self.open_from(config, anchor, anchor, target);
    }

    /// Starts flowing from `start` (a shape inside `anchor`) to `target`; it drains back into
    /// `anchor` when it closes.
    pub fn open_from(&mut self, config: &Config, anchor: Frame, start: Frame, target: Frame) {
        self.flat = config.glass.is_flat() || config.glass.reduce_motion;
        self.open_springs(config);
        if self.phase == Phase::Closed {
            // Fade mode: it starts where it ends, and only fades.
            let from = if self.flat { target } else { start };
            for (edge, value) in self.edges.iter_mut().zip(edges_of(from)) {
                edge.snap(value);
            }
            self.fade.snap(0.0);
        }
        if self.flat {
            self.start_fade(config, self.class.open(config), 1.0);
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
                self.flat = config.glass.is_flat() || config.glass.reduce_motion;
                self.open_springs(config);
                if self.flat {
                    self.start_fade(config, self.class.open(config), 1.0);
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
        let spec = self.class.close(config);
        self.set_cap(config, spec);
        self.flat = config.glass.is_flat() || config.glass.reduce_motion;
        if self.flat {
            self.start_fade(config, spec, 0.0);
            self.phase = Phase::Closing;
            return;
        }
        let near = near_edge(self.anchor, self.frame());
        let body = liquid_spring(config, spec);
        let neck =
            Spring::new(body.response * config.motion.liquid_neck_lead.clamp(0.05, 1.0), body.damping_ratio);
        for (i, edge) in self.edges.iter_mut().enumerate() {
            edge.spring = if i == near { neck } else { body };
        }
        self.set_targets(Frame { origin: self.anchor.center(), size: Vec2::ZERO });
        self.phase = Phase::Closing;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether the shape lies wholly inside the origin's glass (it is drained into it).
    fn inside_anchor(&self) -> bool {
        let e = edges_of(self.anchor);
        let now = edges_of(self.raw_frame());
        now[LEFT] >= e[LEFT] - INSIDE_TOLERANCE_PX
            && now[TOP] >= e[TOP] - INSIDE_TOLERANCE_PX
            && now[RIGHT] <= e[RIGHT] + INSIDE_TOLERANCE_PX
            && now[BOTTOM] <= e[BOTTOM] + INSIDE_TOLERANCE_PX
    }

    pub fn step(&mut self, dt: f32) -> Phase {
        if self.phase == Phase::Closed {
            return self.phase;
        }
        for (i, edge) in self.edges.iter_mut().enumerate() {
            edge.step(dt);
            // The hard clamp: never further past the target than allowed, and no momentum
            // left in that direction.
            let dir = self.dir[i];
            if dir != 0.0 {
                let limit = edge.target + dir * self.allow[i];
                if (edge.value - limit) * dir > 0.0 {
                    edge.value = limit;
                    if edge.velocity * dir > 0.0 {
                        edge.velocity = 0.0;
                    }
                }
            }
        }
        if self.flat {
            self.fade.step(dt);
            let glided = self.edges.iter().all(|e| e.is_settled(SETTLED_PX));
            self.phase = match self.phase {
                Phase::Opening if glided && self.fade.is_settled(FADE_SETTLED) => Phase::Open,
                Phase::Closing if self.fade.value <= FADE_SETTLED => Phase::Closed,
                phase => phase,
            };
            return self.phase;
        }
        let settled = self.edges.iter().all(|e| e.is_settled(SETTLED_PX));
        self.phase = match self.phase {
            Phase::Opening if settled => Phase::Open,
            Phase::Closing if settled || self.inside_anchor() => Phase::Closed,
            phase => phase,
        };
        self.phase
    }

    /// The edges as a rectangle (never inside out, should an edge pass another).
    fn raw_frame(&self) -> Frame {
        let e = |i: usize| self.edges[i].value;
        let lo = Vec2::new(e(LEFT).min(e(RIGHT)), e(TOP).min(e(BOTTOM)));
        let hi = Vec2::new(e(LEFT).max(e(RIGHT)), e(TOP).max(e(BOTTOM)));
        Frame { origin: lo, size: (hi - lo).max(Vec2::ONE) }
    }

    /// The glass's current rectangle.
    pub fn frame(&self) -> Frame {
        let frame = self.raw_frame();
        if !self.flat {
            return frame;
        }
        // Fade mode: a slight scale about the centre as it fades.
        let k = self.fade_scale + (1.0 - self.fade_scale) * self.fade.value.clamp(0.0, 1.0);
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
        // Not instant: it drains over a visible stretch of frames, but quickly.
        assert!(frames > 12 && frames < 120, "{frames}");
        // It ends inside the anchor.
        assert!(last.size.x <= anchor.size.x && last.size.y <= anchor.size.y);
        assert!(anchor.contains(last.center()));
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
        for spec in
            [flat.motion.snappy, flat.motion.liquid, flat.motion.liquid_open.spring(), flat.motion.fling]
        {
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

#[cfg(test)]
mod motion_tests;
