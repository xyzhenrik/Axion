//! The knob of a switch or slider. At rest an opaque white pill; while held it turns into a
//! clear glass lens that follows the pointer, can be pulled a little past the ends (rubber
//! band), and on release springs to its place and frosts back into the white knob — as Liquid
//! Glass controls do on iPadOS 26.

use std::time::Instant;

use axion_physics::{Animated, Spring, rubber_band};

/// How far (in units of the travel) the knob can be pulled past an end.
const OVERPULL: f32 = 0.18;
/// Pointer movement (logical px) that turns a press into a drag.
const DRAG_SLOP: f32 = 3.0;
/// The lens frosts back once the knob is this close (in units of the travel) to its place.
const REFROST_DISTANCE: f32 = 0.06;
/// Velocity smoothing for the squash while dragging.
const VELOCITY_SMOOTHING: f32 = 0.35;

pub(crate) struct Knob {
    /// Position along the travel: 0 at the start, 1 at the end (beyond while over-pulled).
    pub pos: Animated<f32>,
    /// 0: opaque knob, 1: clear glass lens.
    pub lens: Animated<f32>,
    drag: Option<Drag>,
    /// Smoothed speed along the travel (units per second), for the squash.
    pub speed: f32,
}

struct Drag {
    start_x: f32,
    start_pos: f32,
    last: (f32, Instant),
    moved: bool,
}

impl Knob {
    pub fn new(pos: f32, spring: Spring) -> Self {
        Self { pos: Animated::new(pos, spring), lens: Animated::new(0.0, spring), drag: None, speed: 0.0 }
    }

    /// Pointer down at `x` (logical px): the knob swells into the lens.
    pub fn press(&mut self, x: f32) {
        self.lens.target = 1.0;
        self.drag =
            Some(Drag { start_x: x, start_pos: self.pos.value, last: (x, Instant::now()), moved: false });
    }

    /// Pointer at `x`; `travel` is the knob's full travel in logical px.
    pub fn drag_to(&mut self, x: f32, travel: f32) {
        let Some(drag) = &mut self.drag else { return };
        if (x - drag.start_x).abs() > DRAG_SLOP {
            drag.moved = true;
        }
        if !drag.moved {
            return;
        }
        let raw = drag.start_pos + (x - drag.start_x) / travel.max(1.0);
        let pos = if raw < 0.0 {
            rubber_band(raw, OVERPULL)
        } else if raw > 1.0 {
            1.0 + rubber_band(raw - 1.0, OVERPULL)
        } else {
            raw
        };
        let now = Instant::now();
        let dt = now.duration_since(drag.last.1).as_secs_f32();
        if dt > 0.0 {
            let v = (x - drag.last.0) / travel.max(1.0) / dt;
            self.speed += (v - self.speed) * VELOCITY_SMOOTHING;
        }
        drag.last = (x, now);
        self.pos.snap(pos);
    }

    /// Pointer up. Returns whether it was a drag (then the knob's side decides), not a tap.
    pub fn release(&mut self, target: f32) -> bool {
        let moved = self.drag.take().is_some_and(|d| d.moved);
        self.pos.target = target;
        moved
    }

    /// Goes to `target` without a press (the value was set from outside).
    pub fn set(&mut self, target: f32) {
        if self.drag.is_none() {
            self.pos.target = target;
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.pos.step(dt);
        if self.drag.is_none() {
            self.speed *= 1.0 - VELOCITY_SMOOTHING;
            if (self.pos.value - self.pos.target).abs() < REFROST_DISTANCE {
                self.lens.target = 0.0;
            }
        }
        self.lens.step(dt);
    }

    /// Nothing left to animate.
    pub fn is_settled(&self) -> bool {
        self.drag.is_none()
            && self.lens.target == 0.0
            && self.lens.is_settled(0.002)
            && self.pos.is_settled(0.001)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knob() -> Knob {
        Knob::new(0.0, Spring::new(0.3, 0.8))
    }

    #[test]
    fn tap_swells_then_frosts_at_the_other_end() {
        let mut k = knob();
        k.press(10.0);
        for _ in 0..10 {
            k.step(1.0 / 120.0);
        }
        assert!(k.lens.value > 0.3, "the lens appears while held");
        assert!(!k.release(1.0), "a tap, not a drag");
        for _ in 0..240 {
            k.step(1.0 / 120.0);
        }
        assert!((k.pos.value - 1.0).abs() < 0.01 && k.lens.value < 0.01 && k.is_settled());
    }

    #[test]
    fn drag_follows_and_rubber_bands() {
        let mut k = knob();
        k.press(0.0);
        k.drag_to(10.0, 20.0);
        assert!((k.pos.value - 0.5).abs() < 1e-4);
        k.drag_to(200.0, 20.0);
        assert!(k.pos.value > 1.0 && k.pos.value < 1.0 + OVERPULL, "{}", k.pos.value);
        k.drag_to(-200.0, 20.0);
        assert!(k.pos.value < 0.0 && k.pos.value > -OVERPULL);
        assert!(k.release(0.0), "a drag");
    }
}
