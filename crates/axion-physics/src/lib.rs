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

/// Spring parameters, expressed like SwiftUI/UIKit springs (see `docs/physics.md`).
///
/// `response` is the period (seconds) of the undamped oscillation, i.e. the "perceptual
/// duration". `damping_ratio` of 1.0 is critically damped (no overshoot); below 1.0 bounces.
/// With unit mass: `stiffness = (2π / response)²`, `damping = 4π · damping_ratio / response`
/// (`spring(response:dampingFraction:)`, CASpringAnimation's mass/stiffness/damping).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub response: f32,
    pub damping_ratio: f32,
}

impl Spring {
    /// `Spring.interactiveSpring`: response 0.15, dampingFraction 0.86.
    pub const INTERACTIVE: Spring = Spring::new(0.15, 0.86);
    /// `.snappy`: `Spring(duration: 0.5, bounce: 0.15)`.
    pub const SNAPPY: Spring = Spring::from_duration_bounce(0.5, 0.15);
    /// `.bouncy`: `Spring(duration: 0.5, bounce: 0.3)`.
    pub const BOUNCY: Spring = Spring::from_duration_bounce(0.5, 0.3);
    /// `.smooth`: `Spring(duration: 0.5, bounce: 0)`, critically damped.
    pub const SMOOTH: Spring = Spring::from_duration_bounce(0.5, 0.0);

    /// Quick and critically damped: appear/disappear and small moves without any overshoot
    /// (the flat, glass-free mode). Not an Apple preset.
    pub const CRISP: Spring = Spring::new(0.2, 1.0);

    pub const fn new(response: f32, damping_ratio: f32) -> Self {
        Self { response, damping_ratio }
    }

    /// SwiftUI's `Spring(duration:bounce:)`: `response = duration`; `damping_ratio = 1 - bounce`
    /// for `bounce >= 0` and `1 / (1 + bounce)` for negative bounce (over-damped).
    pub const fn from_duration_bounce(duration: f32, bounce: f32) -> Self {
        let damping_ratio = if bounce >= 0.0 { 1.0 - bounce } else { 1.0 / (1.0 + bounce) };
        Self { response: duration, damping_ratio }
    }

    /// The same spring, but never under-damped: it settles without overshoot.
    pub fn critical(self) -> Self {
        Self { damping_ratio: self.damping_ratio.max(1.0), ..self }
    }

    /// Undamped angular frequency ω = 2π / response.
    pub fn angular_frequency(&self) -> f32 {
        TAU / self.response.max(1e-3)
    }

    /// Spring stiffness for unit mass: ω² = (2π / response)².
    pub fn stiffness(&self) -> f32 {
        let w = self.angular_frequency();
        w * w
    }

    /// Damping coefficient for unit mass: 2ζω = 4π ζ / response.
    pub fn damping(&self) -> f32 {
        2.0 * self.damping_ratio * self.angular_frequency()
    }

    /// Exact solution of the damped oscillator after `t` seconds, as the coefficients
    /// `(x_x, x_v, v_x, v_v)` with `x(t) = x_x·x0 + x_v·v0`, `v(t) = v_x·x0 + v_v·v0`, where
    /// `x0` is the offset from the target and `v0` the starting velocity.
    pub fn propagator(&self, t: f32) -> (f32, f32, f32, f32) {
        let w = self.angular_frequency();
        let z = self.damping_ratio.max(0.0);
        let zw = z * w;
        let e = (-zw * t).exp();
        if (z - 1.0).abs() < 1e-4 {
            // Critically damped.
            return (e * (1.0 + w * t), e * t, -e * w * w * t, e * (1.0 - w * t));
        }
        if z < 1.0 {
            let wd = w * (1.0 - z * z).sqrt();
            let (sn, cs) = (wd * t).sin_cos();
            (e * (cs + zw / wd * sn), e * sn / wd, -e * w * w / wd * sn, e * (cs - zw / wd * sn))
        } else {
            let s = w * (z * z - 1.0).sqrt();
            let (sh, ch) = ((s * t).sinh(), (s * t).cosh());
            (e * (ch + zw / s * sh), e * sh / s, -e * w * w / s * sh, e * (ch - zw / s * sh))
        }
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

    /// Advances by `dt` with the exact solution of the spring (no integration error, stable
    /// for any step). Velocity carries over when the target or spring changes.
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, Self::MAX_FRAME);
        if dt <= 0.0 {
            return;
        }
        let (xx, xv, vx, vv) = self.spring.propagator(dt);
        let x0 = self.value - self.target;
        let v0 = self.velocity;
        self.value = self.target + x0 * xx + v0 * xv;
        self.velocity = x0 * vx + v0 * vv;
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

/// UIScrollView's rubber-band constant `c`.
pub const RUBBER_BAND_C: f32 = 0.55;

/// UIScrollView's rubber band: `f(x, d, c) = (1 - 1 / (x·c/d + 1)) · d`, with `x` the overscroll
/// distance, `d` the view's dimension along the axis (`range`) and `c` 0.55. Approaches `range`
/// asymptotically, slope `c` at the boundary.
pub fn rubber_band_with(offset: f32, range: f32, c: f32) -> f32 {
    if range <= 0.0 || c <= 0.0 {
        return 0.0;
    }
    let x = offset.abs();
    ((1.0 - 1.0 / (x * c / range + 1.0)) * range).copysign(offset)
}

/// [`rubber_band_with`] at Apple's constant 0.55.
pub fn rubber_band(offset: f32, range: f32) -> f32 {
    rubber_band_with(offset, range, RUBBER_BAND_C)
}

/// How much glass stretches when pulled `distance` away from its rest position: the Apple
/// rubber band over the element's `range` (its size), scaled so the asymptote is `max`.
/// `stretch = max · f(distance, range, c) / range`; 0 at rest, `max` only at infinity.
pub fn drag_stretch(distance: f32, max: f32, range: f32, c: f32) -> f32 {
    if max <= 0.0 || range <= 0.0 {
        return 0.0;
    }
    max * rubber_band_with(distance.max(0.0), range, c) / range
}

/// `UIScrollView.DecelerationRate.normal`.
pub const DECELERATION_NORMAL: f32 = 0.998;
/// `UIScrollView.DecelerationRate.fast`.
pub const DECELERATION_FAST: f32 = 0.99;

/// Where a fling released with `velocity` (units/s) comes to rest, relative to its start:
/// `v/1000 · rate / (1 - rate)` (WWDC18 "Designing Fluid Interfaces" `project`).
pub fn project(velocity: f32, rate: f32) -> f32 {
    if rate >= 1.0 { 0.0 } else { velocity / 1000.0 * rate / (1.0 - rate) }
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
    fn drag_stretch_is_monotonic_saturating_and_capped() {
        let (max, range, c) = (0.08, 40.0, RUBBER_BAND_C);
        assert_eq!(drag_stretch(0.0, max, range, c), 0.0);
        let mut last = 0.0;
        let mut last_gain = f32::MAX;
        for i in 1..=200 {
            let v = drag_stretch(i as f32 * 5.0, max, range, c);
            assert!(v >= last && v <= max, "{v}");
            let gain = v - last;
            assert!(gain <= last_gain + 1e-6, "resistance grows: gain {gain}");
            last_gain = gain;
            last = v;
        }
        assert!((drag_stretch(1e7, max, range, c) - max).abs() < 1e-4, "asymptote is the maximum");
        // At x = d the Apple curve is 0.3548 d, so 35.48 % of the maximum.
        assert!((drag_stretch(range, max, range, c) / max - 0.354_838_7).abs() < 1e-5);
        assert_eq!(drag_stretch(50.0, 0.0, range, c), 0.0);
        assert_eq!(drag_stretch(-3.0, max, range, c), 0.0);
    }

    #[test]
    fn rubber_band_matches_apples_values() {
        // f(d, d, 0.55) = (1 - 1/1.55) d = 0.35484 d
        assert!((rubber_band(100.0, 100.0) - 35.483_87).abs() < 1e-3);
        // f(2d) = (1 - 1/2.1) d
        assert!((rubber_band(200.0, 100.0) - 52.380_95).abs() < 1e-3);
        // Same as the closed form x·d·c / (d + c·x).
        let (x, d, c) = (37.0_f32, 120.0_f32, 0.55_f32);
        assert!((rubber_band(x, d) - x * d * c / (d + c * x)).abs() < 1e-3);
        // Slope c at the boundary.
        assert!((rubber_band(0.01, 100.0) / 0.01 - 0.55).abs() < 1e-3);
        assert!((rubber_band_with(100.0, 100.0, 1.0) - 50.0).abs() < 1e-4);
    }

    #[test]
    fn spring_parametrisation_is_apples() {
        let s = Spring::new(0.5, 0.8);
        let w = TAU / 0.5;
        assert!((s.stiffness() - w * w).abs() < 1e-3, "(2π/response)²");
        assert!((s.damping() - 4.0 * std::f32::consts::PI * 0.8 / 0.5).abs() < 1e-3, "4πζ/response");
        assert_eq!(Spring::SMOOTH, Spring::new(0.5, 1.0));
        assert!((Spring::SNAPPY.damping_ratio - 0.85).abs() < 1e-6);
        assert!((Spring::BOUNCY.damping_ratio - 0.7).abs() < 1e-6);
        assert_eq!(Spring::INTERACTIVE, Spring::new(0.15, 0.86));
        assert!((Spring::from_duration_bounce(0.5, -0.5).damping_ratio - 2.0).abs() < 1e-6);
        // Critically damped: c = 2·sqrt(k).
        let c = Spring::SMOOTH;
        assert!((c.damping() - 2.0 * c.stiffness().sqrt()).abs() < 1e-3);
    }

    /// Analytic x(t) of x'' = -k x - c x' from offset x0 and velocity v0, textbook formulas.
    fn analytic(s: Spring, x0: f64, v0: f64, t: f64) -> f64 {
        let w = std::f64::consts::TAU / s.response as f64;
        let z = s.damping_ratio as f64;
        let e = (-z * w * t).exp();
        if (z - 1.0).abs() < 1e-9 {
            e * (x0 + (v0 + w * x0) * t)
        } else if z < 1.0 {
            let wd = w * (1.0 - z * z).sqrt();
            e * (x0 * (wd * t).cos() + (v0 + z * w * x0) / wd * (wd * t).sin())
        } else {
            let r = w * (z * z - 1.0).sqrt();
            e * (x0 * (r * t).cosh() + (v0 + z * w * x0) / r * (r * t).sinh())
        }
    }

    #[test]
    fn integrator_matches_the_analytic_solution() {
        let springs =
            [Spring::SMOOTH, Spring::SNAPPY, Spring::BOUNCY, Spring::INTERACTIVE, Spring::new(0.4, 1.6)];
        for spring in springs {
            for (x0, v0) in [(1.0_f32, 0.0_f32), (1.0, 3.0), (-2.0, 5.0)] {
                let mut a = Animated::new(x0, spring);
                a.target = 0.0;
                a.velocity = v0;
                let mut t = 0.0_f64;
                for _ in 0..90 {
                    a.step(1.0 / 60.0);
                    t += 1.0 / 60.0;
                    let want = analytic(spring, x0 as f64, v0 as f64, t);
                    assert!(
                        (a.value as f64 - want).abs() < 2e-3,
                        "{spring:?} t={t:.3}: {} vs {want}",
                        a.value
                    );
                }
            }
        }
    }

    #[test]
    fn smooth_spring_settles_in_about_a_response() {
        // Critically damped, response 0.5: 1 - (1 + ωt)e^{-ωt} at t = 0.5.
        let mut a = Animated::new(0.0_f32, Spring::SMOOTH);
        a.target = 1.0;
        for _ in 0..30 {
            a.step(1.0 / 60.0);
        }
        let w = TAU / 0.5;
        let want = 1.0 - (1.0 + w * 0.5) * (-w * 0.5).exp();
        assert!((a.value - want).abs() < 1e-3 && a.value > 0.8, "{}", a.value);
    }

    #[test]
    fn projection_matches_uikit_deceleration() {
        // 1000 pt/s at the normal rate travels 0.998/0.002 = 499 pt.
        assert!((project(1000.0, DECELERATION_NORMAL) - 499.0).abs() < 0.5);
        assert!((project(1000.0, DECELERATION_FAST) - 99.0).abs() < 0.01);
    }

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
    fn critical_preset_never_bounces_whatever_the_spring_was() {
        assert_eq!(Spring::BOUNCY.critical().damping_ratio, 1.0);
        assert_eq!(Spring::new(0.4, 1.3).critical().damping_ratio, 1.3, "over-damped stays");
        assert_eq!(Spring::BOUNCY.critical().response, Spring::BOUNCY.response);
        for spring in [Spring::CRISP, Spring::SNAPPY.critical(), Spring::INTERACTIVE.critical()] {
            let mut a = Animated::new(0.0_f32, spring);
            a.target = 1.0;
            for _ in 0..240 {
                a.step(1.0 / 120.0);
                assert!(a.value <= 1.0 + 1e-4, "overshot to {}", a.value);
            }
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
