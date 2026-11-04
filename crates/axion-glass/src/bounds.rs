//! Screen regions a glass layer can touch.
//!
//! The composite shader returns the plain backdrop wherever the combined SDF is further than
//! [`FAR`] from every shape, and the frosted backdrop is only sampled inside the shapes (plus
//! the refraction offset). So a layer only has to run its blur and composite passes over the
//! bounding boxes of its shapes, grown by the margins below. All rectangles are conservative:
//! growing one only costs time, never changes a pixel.

use glam::Vec2;

use crate::params::GlassParams;
use crate::renderer::BLUR_DOWNSAMPLE;
use crate::shape::GlassShape;

/// Distance from the SDF beyond which `glass.wgsl` returns the backdrop untouched
/// (`SHADOW_REACH + SHADOW_OFFSET.y`).
const FAR: f32 = 28.0 + 8.0;
/// Anti-aliasing and rounding slack around every box, in pixels.
const PAD: f32 = 2.0;
/// Most regions one layer is split into; more are merged (each costs three small passes).
const MAX_REGIONS: usize = 8;
/// Two regions merge when the union is at most this much larger than their sum.
const MERGE_GROWTH: f32 = 1.25;

/// Integer pixel rectangle, `x0..x1` by `y0..y1` (exclusive end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl IRect {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    fn area(&self) -> f32 {
        self.width() as f32 * self.height() as f32
    }

    fn intersects(&self, o: &Self) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    fn union(&self, o: &Self) -> Self {
        Self { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    pub fn full(size: (u32, u32)) -> Self {
        Self { x0: 0, y0: 0, x1: size.0, y1: size.1 }
    }

    /// Grows by `by` pixels on every side, clamped to `size`.
    pub fn grown(&self, by: u32, size: (u32, u32)) -> Self {
        Self {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: (self.x1 + by).min(size.0),
            y1: (self.y1 + by).min(size.1),
        }
    }
}

/// Float rectangle in pixels (may lie partly or wholly off screen).
#[derive(Clone, Copy, Debug)]
struct FRect {
    min: Vec2,
    max: Vec2,
}

impl FRect {
    fn grown(self, by: f32) -> Self {
        Self { min: self.min - by, max: self.max + by }
    }

    /// Clamps to the screen; `None` when nothing is left.
    fn to_screen(self, size: (u32, u32)) -> Option<IRect> {
        if !(self.min.is_finite() && self.max.is_finite()) {
            return Some(IRect::full(size));
        }
        let x0 = self.min.x.floor().clamp(0.0, size.0 as f32) as u32;
        let y0 = self.min.y.floor().clamp(0.0, size.1 as f32) as u32;
        let x1 = self.max.x.ceil().clamp(0.0, size.0 as f32) as u32;
        let y1 = self.max.y.ceil().clamp(0.0, size.1 as f32) as u32;
        (x1 > x0 && y1 > y0).then_some(IRect { x0, y0, x1, y1 })
    }
}

/// One group of nearby shapes with the screen rectangles its passes cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    /// Where the composite pass writes (anything outside equals the backdrop).
    pub composite: IRect,
    /// Where the blurred backdrop is sampled, in full-resolution pixels.
    pub blur: IRect,
}

/// Bounding box of everything `shape_sdf` (in `glass.wgsl`) maps within `reach` of the shape.
///
/// The shader unstretches the point (`d' = R(a)·diag(1/s)·R(-a)·(p - c)`) and scales the
/// distance by `min(s)`, so `sdf <= reach` bounds `d'` to the half-size box grown by
/// `reach / min(s)`; mapping that box back with `T = R(a)·diag(s)·R(-a)` gives the extent.
fn shape_box(shape: &GlassShape, reach: f32) -> FRect {
    let s = shape.stretch.max(Vec2::splat(0.05));
    let grow = reach / s.min_element();
    let h = shape.half_size.abs() + Vec2::splat(grow);
    let (sin, cos) = shape.stretch_angle.sin_cos();
    let off = cos * sin * (s.x - s.y);
    let t = [[cos * cos * s.x + sin * sin * s.y, off], [off, sin * sin * s.x + cos * cos * s.y]];
    let ext = Vec2::new(t[0][0].abs() * h.x + t[0][1].abs() * h.y, t[1][0].abs() * h.x + t[1][1].abs() * h.y);
    FRect { min: shape.center - ext, max: shape.center + ext }
}

/// Splits `shapes` into regions. `merge_k` is the widest smooth-union radius in use.
///
/// The smooth minimum of any number of fields is never lower than their minimum minus `k`
/// (each step lowers it by `(k - gap)^2 / 4k` and the gap shrinks the further it is lowered),
/// so the combined field is within `FAR` only where some shape's own field is within
/// `FAR + k`.
pub(crate) fn regions(
    shapes: &[GlassShape],
    params: &GlassParams,
    merge_k: f32,
    size: (u32, u32),
) -> Vec<Region> {
    let mut regions: Vec<Region> = Vec::new();
    // Refraction moves the lookup by at most `|refraction| * (1 + dispersion)`.
    let shift = params.refraction.abs() * (1.0 + params.dispersion.abs());
    for shape in shapes {
        let comp = shape_box(shape, FAR + merge_k).grown(PAD);
        // The frosted backdrop is only used where the shape's own edge is (d < 1).
        let blur = shape_box(shape, 1.0 + merge_k).grown(PAD + shift + 1.0);
        let Some(composite) = comp.to_screen(size) else {
            // Off screen: the box already includes everything this shape can reach.
            continue;
        };
        let blur = blur.to_screen(size).unwrap_or(composite);
        regions.push(Region { composite, blur });
    }
    merge(&mut regions);
    regions
}

fn merge(regions: &mut Vec<Region>) {
    let union = |a: &Region, b: &Region| Region {
        composite: a.composite.union(&b.composite),
        blur: a.blur.union(&b.blur),
    };
    // Composite rectangles must not overlap: the in-place composite would otherwise draw the
    // overlap twice. Merge those first, then anything that is cheaper as one.
    loop {
        let mut pair = None;
        'search: for i in 0..regions.len() {
            for j in i + 1..regions.len() {
                let (a, b) = (&regions[i], &regions[j]);
                let u = a.composite.union(&b.composite).area();
                if a.composite.intersects(&b.composite)
                    || u <= (a.composite.area() + b.composite.area()) * MERGE_GROWTH
                {
                    pair = Some((i, j));
                    break 'search;
                }
            }
        }
        let Some((i, j)) = pair else { break };
        let b = regions.swap_remove(j);
        regions[i] = union(&regions[i], &b);
    }
    // Too many: merge the pair that grows the least, ignoring overlap (already merged above;
    // a union can create new overlaps, so run the overlap merge again afterwards).
    while regions.len() > MAX_REGIONS {
        let mut best = (0, 1, f32::MAX);
        for i in 0..regions.len() {
            for j in i + 1..regions.len() {
                let growth = regions[i].composite.union(&regions[j].composite).area()
                    - regions[i].composite.area()
                    - regions[j].composite.area();
                if growth < best.2 {
                    best = (i, j, growth);
                }
            }
        }
        let b = regions.swap_remove(best.1);
        regions[best.0] = union(&regions[best.0], &b);
        // Restore the disjointness invariant.
        let mut again = true;
        while again {
            again = false;
            'o: for i in 0..regions.len() {
                for j in i + 1..regions.len() {
                    if regions[i].composite.intersects(&regions[j].composite) {
                        let b = regions.swap_remove(j);
                        regions[i] = union(&regions[i], &b);
                        again = true;
                        break 'o;
                    }
                }
            }
        }
    }
}

/// Rectangle in blur-target pixels covering the full-resolution rectangle `r`, plus one
/// texel on each side for bilinear filtering.
pub(crate) fn blur_rect(r: &IRect, bounds: (u32, u32)) -> Option<IRect> {
    let d = BLUR_DOWNSAMPLE;
    let x0 = (r.x0 / d).saturating_sub(1);
    let y0 = (r.y0 / d).saturating_sub(1);
    let x1 = (r.x1.div_ceil(d) + 1).min(bounds.0);
    let y1 = (r.y1.div_ceil(d) + 1).min(bounds.1);
    (x1 > x0 && y1 > y0).then_some(IRect { x0, y0, x1, y1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotate(v: Vec2, a: f32) -> Vec2 {
        let (s, c) = a.sin_cos();
        Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
    }

    /// Mirror of `shape_sdf` in `glass.wgsl`.
    fn sdf(s: &GlassShape, p: Vec2) -> f32 {
        let st = s.stretch.max(Vec2::splat(0.05));
        let mut d = rotate(p - s.center, -s.stretch_angle);
        d /= st;
        d = rotate(d, s.stretch_angle);
        let r = s.corner_radius.clamp(0.0, s.half_size.min_element());
        let q = d.abs() - s.half_size + Vec2::splat(r);
        (q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - r) * st.min_element()
    }

    #[test]
    fn box_contains_the_reach_of_stretched_rotated_shapes() {
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..200 {
            let mut shape = GlassShape::rounded_rect(
                Vec2::new(rnd() * 400.0, rnd() * 300.0),
                Vec2::new(5.0 + rnd() * 120.0, 5.0 + rnd() * 80.0),
                rnd() * 40.0,
            );
            shape.stretch = Vec2::new(0.6 + rnd() * 1.2, 0.6 + rnd() * 1.2);
            shape.stretch_angle = rnd() * std::f32::consts::TAU;
            let reach = 10.0 + rnd() * 60.0;
            let b = shape_box(&shape, reach);
            for _ in 0..400 {
                let p = Vec2::new(rnd() * 700.0 - 150.0, rnd() * 600.0 - 150.0);
                if sdf(&shape, p) <= reach {
                    assert!(
                        p.x >= b.min.x - 1e-3
                            && p.x <= b.max.x + 1e-3
                            && p.y >= b.min.y - 1e-3
                            && p.y <= b.max.y + 1e-3,
                        "{p:?} within reach of {shape:?} but outside {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn small_buttons_stay_small_and_far_ones_stay_apart() {
        let params = GlassParams::default();
        let size = (2560, 1440);
        let a = GlassShape::circle(Vec2::new(100.0, 100.0), 20.0);
        let b = GlassShape::circle(Vec2::new(2000.0, 1200.0), 20.0);
        let r = regions(&[a, b], &params, params.merge_smoothness, size);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|r| r.composite.width() < 300 && r.composite.height() < 300));
    }

    #[test]
    fn overlapping_boxes_merge_and_composites_stay_disjoint() {
        let params = GlassParams::default();
        let shapes: Vec<_> = (0..40)
            .map(|i| GlassShape::circle(Vec2::new(60.0 * i as f32, 50.0 + 3.0 * i as f32), 25.0))
            .collect();
        let r = regions(&shapes, &params, params.merge_smoothness, (2560, 1440));
        assert!(r.len() <= MAX_REGIONS);
        for (i, a) in r.iter().enumerate() {
            for b in &r[i + 1..] {
                assert!(!a.composite.intersects(&b.composite));
            }
        }
    }

    #[test]
    fn off_screen_shapes_are_dropped_and_edge_shapes_clamped() {
        let params = GlassParams::default();
        let size = (800, 600);
        let off = GlassShape::circle(Vec2::new(-500.0, -500.0), 20.0);
        assert!(regions(&[off], &params, 20.0, size).is_empty());
        let edge = GlassShape::circle(Vec2::new(0.0, 0.0), 20.0);
        let r = regions(&[edge], &params, 20.0, size);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].composite.x0, r[0].composite.y0), (0, 0));
    }
}
