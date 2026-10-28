//! A Safari-like arrangement of glass elements driven by springs.
//!
//! Anchored elements (bar, buttons, sidebar, media controls) can be pulled with
//! rubber-banding and spring back home. Free droplets can be thrown around and merge with
//! anything they touch.

use std::time::Instant;

use axion_glass::GlassShape;
use axion_physics::{Animated, Spring, VelocityTracker, decay, rubber_band_vec};
use glam::Vec2;

/// How far an anchored element can be pulled from home, in logical pixels.
const PULL_LIMIT: f32 = 90.0;
/// Speed (px/s) at which the stretch reaches its maximum.
const STRETCH_SPEED: f32 = 2600.0;
const MAX_STRETCH: f32 = 0.32;
/// Scale of an element while it is held.
const LIFT_SCALE: f32 = 1.06;
/// Fraction of velocity kept per millisecond for thrown droplets.
const FLING_DECAY: f32 = 0.9975;
const WALL_BOUNCE: f32 = 0.55;
const MIN_FLING_SPEED: f32 = 8.0;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Anchored,
    Free,
}

#[derive(Clone, Copy, PartialEq)]
enum Motion {
    Spring,
    Fling,
}

struct Element {
    kind: Kind,
    motion: Motion,
    home: Vec2,
    half_size: Vec2,
    radius: f32,
    pos: Animated<Vec2>,
    stretch: Animated<f32>,
    stretch_angle: f32,
    lift: Animated<f32>,
}

impl Element {
    fn new(kind: Kind, half_size: Vec2, radius: f32) -> Self {
        Self {
            kind,
            motion: Motion::Spring,
            home: Vec2::ZERO,
            half_size,
            radius,
            pos: Animated::new(Vec2::ZERO, Spring::BOUNCY),
            stretch: Animated::new(0.0, Spring::SNAPPY),
            stretch_angle: 0.0,
            lift: Animated::new(1.0, Spring::BOUNCY),
        }
    }

    fn shape(&self, scale: f32) -> GlassShape {
        let lift = self.lift.value;
        let s = 1.0 + self.stretch.value.max(0.0);
        GlassShape {
            center: self.pos.value,
            half_size: self.half_size * scale * lift,
            corner_radius: self.radius * scale * lift,
            stretch: Vec2::new(s, 1.0 / s),
            stretch_angle: self.stretch_angle,
            tint_opacity: None,
            merge: None,
            glow: 0.0,
            join_only: false,
            shadow: 1.0,
        }
    }
}

struct Drag {
    index: usize,
    grab_offset: Vec2,
    tracker: VelocityTracker,
}

pub struct Scene {
    elements: Vec<Element>,
    drag: Option<Drag>,
    pointer: Vec2,
    size: Vec2,
    /// Physical pixels per logical pixel.
    scale: f32,
    laid_out: bool,
}

// Indices into `elements`, in draw order.
const SIDEBAR: usize = 0;
const BAR: usize = 1;
const BACK: usize = 2;
const OVERVIEW: usize = 3;
const MEDIA: usize = 4;
const DROP_BIG: usize = 5;
const DROP_SMALL: usize = 6;

impl Scene {
    pub fn new() -> Self {
        let elements = vec![
            Element::new(Kind::Anchored, Vec2::new(140.0, 300.0), 28.0),
            Element::new(Kind::Anchored, Vec2::new(300.0, 24.0), 24.0),
            Element::new(Kind::Anchored, Vec2::splat(24.0), 24.0),
            Element::new(Kind::Anchored, Vec2::splat(24.0), 24.0),
            Element::new(Kind::Anchored, Vec2::new(210.0, 28.0), 28.0),
            Element::new(Kind::Free, Vec2::splat(46.0), 46.0),
            Element::new(Kind::Free, Vec2::splat(30.0), 30.0),
        ];
        Self { elements, drag: None, pointer: Vec2::ZERO, size: Vec2::ZERO, scale: 1.0, laid_out: false }
    }

    /// Responsive layout: homes move with the window and elements spring to them.
    pub fn layout(&mut self, size: Vec2, scale: f32) {
        self.size = size;
        self.scale = scale;
        let logical = size / scale;
        let margin = 16.0;
        let sidebar_w = 280.0;
        let sidebar_visible = logical.x > 900.0;

        let sidebar = &mut self.elements[SIDEBAR];
        sidebar.half_size = Vec2::new(sidebar_w / 2.0, ((logical.y - 2.0 * margin) / 2.0).max(40.0));
        sidebar.home = if sidebar_visible {
            Vec2::new(margin + sidebar_w / 2.0, logical.y / 2.0)
        } else {
            // Slid out to the left, off-screen.
            Vec2::new(-sidebar_w, logical.y / 2.0)
        };

        let content_left = if sidebar_visible { margin + sidebar_w } else { 0.0 };
        let content_center = (content_left + logical.x) / 2.0;
        let button = 24.0;
        let gap = 14.0;
        let available = logical.x - content_left - 2.0 * (margin + 2.0 * button + gap);
        let bar_half = (available / 2.0).clamp(80.0, 300.0);
        let top = margin + 24.0;
        self.elements[BAR].half_size.x = bar_half;
        self.elements[BAR].home = Vec2::new(content_center, top);
        self.elements[BACK].home = Vec2::new(content_center - bar_half - gap - button, top);
        self.elements[OVERVIEW].home = Vec2::new(content_center + bar_half + gap + button, top);
        self.elements[MEDIA].home = Vec2::new(content_center, logical.y - margin - 28.0 - 8.0);

        if !self.laid_out {
            self.elements[DROP_BIG].home = Vec2::new(logical.x - 180.0, logical.y - 260.0);
            self.elements[DROP_SMALL].home = Vec2::new(logical.x - 300.0, logical.y - 180.0);
        }

        for e in &mut self.elements {
            let home = e.home * scale;
            match (e.kind, self.laid_out) {
                (_, false) => e.pos.snap(home),
                (Kind::Anchored, true) => e.pos.target = home,
                (Kind::Free, true) => {
                    let clamped = e.pos.value.clamp(Vec2::ZERO, size);
                    e.pos.target = clamped;
                }
            }
        }
        self.laid_out = true;
    }

    pub fn pointer_moved(&mut self, p: Vec2) {
        self.pointer = p;
        if let Some(drag) = &mut self.drag {
            drag.tracker.push(Instant::now(), p);
        }
    }

    pub fn pointer_down(&mut self) {
        let p = self.pointer;
        let hit = (0..self.elements.len()).rev().find(|&i| self.elements[i].shape(self.scale).contains(p));
        if let Some(index) = hit {
            let e = &mut self.elements[index];
            e.motion = Motion::Spring;
            e.pos.spring = Spring::INTERACTIVE;
            e.lift.target = LIFT_SCALE;
            let mut tracker = VelocityTracker::default();
            tracker.push(Instant::now(), p);
            self.drag = Some(Drag { index, grab_offset: p - e.pos.value, tracker });
        }
    }

    pub fn pointer_up(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        let e = &mut self.elements[drag.index];
        let velocity = drag.tracker.velocity();
        e.lift.target = 1.0;
        e.pos.velocity = velocity;
        match e.kind {
            Kind::Anchored => {
                e.pos.spring = Spring::BOUNCY;
                e.pos.target = e.home * self.scale;
            }
            Kind::Free => e.motion = Motion::Fling,
        }
    }

    pub fn update(&mut self, dt: f32) {
        let scale = self.scale;
        if let Some(drag) = &self.drag {
            let e = &mut self.elements[drag.index];
            let wanted = self.pointer - drag.grab_offset;
            e.pos.target = match e.kind {
                Kind::Anchored => {
                    let home = e.home * scale;
                    home + rubber_band_vec(wanted - home, PULL_LIMIT * scale)
                }
                Kind::Free => wanted,
            };
        }

        for e in &mut self.elements {
            match e.motion {
                Motion::Spring => e.pos.step(dt),
                Motion::Fling => {
                    e.pos.value += e.pos.velocity * dt;
                    e.pos.velocity = decay(e.pos.velocity, dt, FLING_DECAY);
                    let r = e.half_size * scale;
                    for axis in 0..2 {
                        let lo = r[axis];
                        let hi = self.size[axis] - r[axis];
                        if e.pos.value[axis] < lo || e.pos.value[axis] > hi {
                            e.pos.value[axis] = e.pos.value[axis].clamp(lo, hi.max(lo));
                            e.pos.velocity[axis] *= -WALL_BOUNCE;
                        }
                    }
                    if e.pos.velocity.length() < MIN_FLING_SPEED * scale {
                        e.motion = Motion::Spring;
                        e.pos.spring = Spring::SNAPPY;
                        e.pos.target = e.pos.value;
                    }
                }
            }

            // Liquid stretch along the direction of motion, area preserving.
            let v = e.pos.velocity / scale;
            let speed = v.length();
            if speed > 30.0 {
                e.stretch_angle = v.y.atan2(v.x);
            }
            e.stretch.target = (speed / STRETCH_SPEED).min(1.0) * MAX_STRETCH;
            e.stretch.step(dt);
            e.lift.step(dt);
        }
    }

    /// Centre of the big droplet, for the scripted demo drag.
    pub fn droplet_center(&self) -> Vec2 {
        self.elements[DROP_BIG].pos.value
    }

    /// Centre of the tab bar, for the scripted demo drag.
    pub fn bar_center(&self) -> Vec2 {
        self.elements[BAR].home * self.scale
    }

    pub fn shapes(&self) -> Vec<GlassShape> {
        self.elements.iter().map(|e| e.shape(self.scale)).collect()
    }
}
