//! Physical motion primitives for Axion's UI.
//!
//! Every animation in the browser is driven by these types instead of fixed-duration easing
//! curves, so motion can be interrupted and redirected at any time (grab a sliding panel
//! mid-flight and it simply follows the pointer from its current velocity).

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::ops::{Add, Mul, Sub};
use std::time::{Duration, Instant};

use glam::Vec2;

/// A value that springs can animate.
pub trait Animatable: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<f32, Output = Self> {
    const ZERO: Self;
    fn magnitude(self) -> f32;
}

impl Animatable for f32 {
    const ZERO: Self = 0.0;
    fn magnitude(self) -> f32 {
        self.abs()
    }
}

impl Animatable for Vec2 {
    const ZERO: Self = Vec2::ZERO;
    fn magnitude(self) -> f32 {
        self.length()
    }
}

/// Spring parameters, expressed like SwiftUI springs.
///
/// `response` is the period (seconds) of the undamped oscillation — roughly how long the
/// motion takes. `damping_ratio` of 1.0 is critically damped (no overshoot); below 1.0 bounces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub response: f32,
    pub damping_ratio: f32,
}

impl Spring {
    /// Fast, barely damped follow for direct manipulation (dragging).
    pub const INTERACTIVE: Spring = Spring::new(0.15, 0.86);
    /// Quick settle with a hint of overshoot, for buttons and small elements.
    pub const SNAPPY: Spring = Spring::new(0.3, 0.8);
    /// Visible bounce, for elements springing back after being pulled.
    pub const BOUNCY: Spring = Spring::new(0.5, 0.6);
    /// Calm, no overshoot, for large panels and layout changes.
    pub const SMOOTH: Spring = Spring::new(0.5, 1.0);

    pub const fn new(response: f32, damping_ratio: f32) -> Self {
        Self { response, damping_ratio }
    }

    fn angular_frequency(&self) -> f32 {
        TAU / self.response.max(1e-3)
    }

    fn stiffness(&self) -> f32 {
        let w = self.angular_frequency();
        w * w
    }

    fn damping(&self) -> f32 {
        2.0 * self.damping_ratio * self.angular_frequency()
    }
}

/// A value moving towards a target under a spring.
#[derive(Clone, Copy, Debug)]
pub struct Animated<T> {
    pub value: T,
    pub velocity: T,
    pub target: T,
    pub spring: Spring,
}

impl<T: Animatable> Animated<T> {
    /// Largest integration step; smaller steps keep stiff springs stable.
    const MAX_STEP: f32 = 1.0 / 240.0;
    /// Frames longer than this (window hidden, debugger) are clamped to avoid explosions.
    const MAX_FRAME: f32 = 0.1;

    pub fn new(value: T, spring: Spring) -> Self {
        Self { value, velocity: T::ZERO, target: value, spring }
    }

    /// Jump to `value` with no motion.
    pub fn snap(&mut self, value: T) {
        self.value = value;
        self.target = value;
        self.velocity = T::ZERO;
    }

    pub fn step(&mut self, dt: f32) {
        let k = self.spring.stiffness();
        let c = self.spring.damping();
        let mut remaining = dt.clamp(0.0, Self::MAX_FRAME);
        while remaining > 0.0 {
            let h = remaining.min(Self::MAX_STEP);
            let accel = (self.target - self.value) * k - self.velocity * c;
            // Semi-implicit Euler: update velocity first, then position.
            self.velocity = self.velocity + accel * h;
            self.value = self.value + self.velocity * h;
            remaining -= h;
        }
    }

    pub fn is_settled(&self, epsilon: f32) -> bool {
        (self.target - self.value).magnitude() < epsilon && self.velocity.magnitude() < epsilon
    }
}

/// Estimates pointer velocity from recent samples, for flings on release.
#[derive(Clone, Debug)]
pub struct VelocityTracker {
    samples: VecDeque<(Instant, Vec2)>,
    window: Duration,
}

impl Default for VelocityTracker {
    fn default() -> Self {
        Self::new(Duration::from_millis(80))
    }
}

impl VelocityTracker {
    pub fn new(window: Duration) -> Self {
        Self { samples: VecDeque::new(), window }
    }

    pub fn push(&mut self, time: Instant, position: Vec2) {
        self.samples.push_back((time, position));
        while let Some(&(t, _)) = self.samples.front() {
            if time.duration_since(t) > self.window && self.samples.len() > 2 {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Velocity in units per second; zero if there aren't enough samples.
    pub fn velocity(&self) -> Vec2 {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, p0)), Some(&(t1, p1))) => {
                let dt = t1.duration_since(t0).as_secs_f32();
                if dt > 1e-4 { (p1 - p0) / dt } else { Vec2::ZERO }
            }
            _ => Vec2::ZERO,
        }
    }
}

/// iOS-style rubber-banding: the further you pull past the boundary, the less it moves.
/// The result approaches `limit` asymptotically but never exceeds it.
pub fn rubber_band(offset: f32, limit: f32) -> f32 {
    const COEFFICIENT: f32 = 0.55;
    if limit <= 0.0 {
        return 0.0;
    }
    let x = offset.abs();
    let banded = (1.0 - 1.0 / (x * COEFFICIENT / limit + 1.0)) * limit;
    banded.copysign(offset)
}

/// Rubber-banding applied to a 2D offset along its own direction.
pub fn rubber_band_vec(offset: Vec2, limit: f32) -> Vec2 {
    let len = offset.length();
    if len < 1e-6 { Vec2::ZERO } else { offset / len * rubber_band(len, limit) }
}

/// Exponential decay of a fling velocity. `rate` is the fraction kept per millisecond,
/// like `UIScrollView.decelerationRate` (0.998 normal, 0.99 fast).
pub fn decay(velocity: Vec2, dt: f32, rate: f32) -> Vec2 {
    velocity * rate.powf(dt * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_to_target() {
        let mut a = Animated::new(0.0_f32, Spring::BOUNCY);
        a.target = 100.0;
        for _ in 0..300 {
            a.step(1.0 / 60.0);
        }
        assert!(a.is_settled(0.01), "value {} velocity {}", a.value, a.velocity);
    }

    #[test]
    fn critically_damped_spring_does_not_overshoot() {
        let mut a = Animated::new(0.0_f32, Spring::SMOOTH);
        a.target = 1.0;
        for _ in 0..240 {
            a.step(1.0 / 120.0);
            assert!(a.value <= 1.0 + 1e-4, "overshot to {}", a.value);
        }
    }

    #[test]
    fn huge_frame_does_not_explode() {
        let mut a = Animated::new(Vec2::ZERO, Spring::INTERACTIVE);
        a.target = Vec2::splat(50.0);
        a.step(10.0);
        assert!(a.value.is_finite());
        assert!(a.value.length() < 200.0);
    }

    #[test]
    fn rubber_band_is_bounded_and_monotonic() {
        let limit = 100.0;
        let mut last = 0.0;
        for i in 1..1000 {
            let v = rubber_band(i as f32, limit);
            assert!(v > last && v < limit);
            last = v;
        }
        assert_eq!(rubber_band(-50.0, limit), -rubber_band(50.0, limit));
    }

    #[test]
    fn velocity_tracker_measures_constant_motion() {
        let mut t = VelocityTracker::default();
        let start = Instant::now();
        for i in 0..10 {
            let time = start + Duration::from_millis(i * 10);
            t.push(time, Vec2::new(i as f32 * 10.0, 0.0));
        }
        let v = t.velocity();
        assert!((v.x - 1000.0).abs() < 1.0, "{v}");
    }
}
