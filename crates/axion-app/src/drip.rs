//! The "drip" from the design ("Drip — button to popover"): a glass shape flows out of the
//! button that opened it to its final rectangle on the liquid spring, and back when it closes.
//! Its content fades in once the glass has nearly arrived.

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_physics::{Animated, Spring};
use glam::Vec2;

/// Content starts to appear when the glass is this close (fraction of the travel) to its target.
const CONTENT_FROM: f32 = 0.35;
const SETTLED_PX: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Closed,
    Opening,
    Open,
    Closing,
}

pub struct Drip {
    origin: Animated<Vec2>,
    size: Animated<Vec2>,
    /// Where it came from (and returns to).
    anchor: Frame,
    target: Frame,
    phase: Phase,
}

impl Drip {
    pub fn new() -> Self {
        Self {
            origin: Animated::new(Vec2::ZERO, Spring::SNAPPY),
            size: Animated::new(Vec2::ZERO, Spring::SNAPPY),
            anchor: Frame::default(),
            target: Frame::default(),
            phase: Phase::Closed,
        }
    }

    fn springs(&mut self, config: &Config) {
        let motion = &config.motion;
        let spring = if config.glass.reduce_motion {
            // Reduce Motion: no overshoot, quick.
            Spring::new(0.2 * motion.speed, 1.0)
        } else {
            Spring::new(motion.liquid.response * motion.speed, motion.liquid.damping)
        };
        self.origin.spring = spring;
        self.size.spring = spring;
    }

    /// Starts flowing from `anchor` to `target` (logical px).
    pub fn open(&mut self, config: &Config, anchor: Frame, target: Frame) {
        self.springs(config);
        if self.phase == Phase::Closed {
            self.origin.snap(anchor.origin);
            self.size.snap(anchor.size);
        }
        self.anchor = anchor;
        self.target = target;
        self.origin.target = target.origin;
        self.size.target = target.size;
        self.phase = Phase::Opening;
    }

    /// Moves the target (the list grew while typing, the window was resized) without
    /// restarting: the glass glides to its new size instead of jumping or dripping again.
    pub fn retarget(&mut self, target: Frame) {
        if self.phase == Phase::Opening || self.phase == Phase::Open {
            self.target = target;
            self.origin.target = target.origin;
            self.size.target = target.size;
        }
    }

    /// Opens if closed or closing (dripping out of `anchor`), else glides to `target`.
    pub fn show(&mut self, config: &Config, anchor: Frame, target: Frame) {
        match self.phase {
            Phase::Open | Phase::Opening => self.retarget(target),
            // Reopened while flowing back: turn around from where it is, no new drip.
            Phase::Closing => {
                self.springs(config);
                self.anchor = anchor;
                self.target = target;
                self.origin.target = target.origin;
                self.size.target = target.size;
                self.phase = Phase::Opening;
            }
            Phase::Closed => self.open(config, anchor, target),
        }
    }

    /// Flows back into the anchor.
    pub fn close(&mut self, config: &Config) {
        if self.phase == Phase::Closed {
            return;
        }
        self.springs(config);
        self.origin.target = self.anchor.origin;
        self.size.target = self.anchor.size;
        self.phase = Phase::Closing;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn step(&mut self, dt: f32) -> Phase {
        if self.phase == Phase::Closed {
            return self.phase;
        }
        self.origin.step(dt);
        self.size.step(dt);
        let settled = self.origin.is_settled(SETTLED_PX) && self.size.is_settled(SETTLED_PX);
        self.phase = match self.phase {
            Phase::Opening if settled => Phase::Open,
            Phase::Closing if settled => Phase::Closed,
            phase => phase,
        };
        self.phase
    }

    /// The glass's current rectangle.
    pub fn frame(&self) -> Frame {
        Frame { origin: self.origin.value, size: self.size.value.max(Vec2::ONE) }
    }

    /// Opacity of the content: 0 while the glass travels, 1 once it is (nearly) there.
    pub fn content_opacity(&self) -> f32 {
        let travel = (self.target.size - self.anchor.size).length().max(1.0);
        let remaining = (self.target.size - self.size.value).length() / travel;
        match self.phase {
            Phase::Closed => 0.0,
            Phase::Closing => (1.0 - (1.0 - remaining) * 3.0).clamp(0.0, 1.0).min(remaining),
            _ => ((CONTENT_FROM - remaining) / CONTENT_FROM).clamp(0.0, 1.0),
        }
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
        assert_eq!(drip.content_opacity(), 0.0);
        for _ in 0..240 {
            drip.step(1.0 / 120.0);
        }
        assert_eq!(drip.phase(), Phase::Open);
        assert!((drip.frame().size - target.size).length() < 1.0);
        assert!(drip.content_opacity() > 0.99);
        drip.close(&config);
        for _ in 0..240 {
            drip.step(1.0 / 120.0);
        }
        assert_eq!(drip.phase(), Phase::Closed);
    }
}
