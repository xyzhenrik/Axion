use bytemuck::{Pod, Zeroable};
use glam::Vec2;

/// One glass element: a rounded rectangle (capsules and circles are special cases), in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShape {
    pub center: Vec2,
    pub half_size: Vec2,
    pub corner_radius: f32,
    /// Squash and stretch along `stretch_angle`: x scales along the angle, y across it.
    /// `(1, 1)` is undeformed; keep `x * y ≈ 1` to preserve area like a liquid.
    pub stretch: Vec2,
    /// Direction of the stretch in radians (screen space).
    pub stretch_angle: f32,
    /// This shape's tint strength instead of the pass's (`GlassParams::tint` alpha). Where it
    /// flows into other shapes the strengths blend, so denser glass (a popover) can merge
    /// with thinner glass (the bar) in one pass.
    pub tint_opacity: Option<f32>,
    /// This shape's own smooth-union radius in pixels, when larger than the pass's
    /// (`GlassParams::merge_smoothness`): it flows into all other shapes of its pass with this
    /// wider fillet (the broad bridge joining a popover to its bar). Such shapes are merged
    /// last, all with the widest of their radii.
    pub merge: Option<f32>,
    /// Brightening inside this shape (0–1): a pressed button's lens is clearer (less tint) and
    /// lighter than the glass it swells out of. Blended in over the shape's soft edge.
    pub glow: f32,
    /// The shape only joins others (a bridge between a popover and its bar): it takes no part
    /// in the tint, which blends across it from the shapes it joins.
    pub join_only: bool,
    /// How much of the pass's drop shadow this shape casts (0-1; 1 = all of it). A glass
    /// *button* casts none: a shadow around a small control reads as a dark plate behind it.
    pub shadow: f32,
}

impl GlassShape {
    pub fn rounded_rect(center: Vec2, half_size: Vec2, corner_radius: f32) -> Self {
        Self {
            center,
            half_size,
            corner_radius,
            stretch: Vec2::ONE,
            stretch_angle: 0.0,
            tint_opacity: None,
            merge: None,
            glow: 0.0,
            join_only: false,
            shadow: 1.0,
        }
    }

    pub fn capsule(center: Vec2, half_size: Vec2) -> Self {
        Self::rounded_rect(center, half_size, half_size.min_element())
    }

    pub fn circle(center: Vec2, radius: f32) -> Self {
        Self::rounded_rect(center, Vec2::splat(radius), radius)
    }

    /// Signed distance from `p` to the undeformed shape (negative inside). Used for hit-testing.
    pub fn distance(&self, p: Vec2) -> f32 {
        sd_round_box(p - self.center, self.half_size, self.corner_radius)
    }

    pub fn contains(&self, p: Vec2) -> bool {
        self.distance(p) <= 0.0
    }

    pub(crate) fn to_gpu(self) -> ShapeGpu {
        ShapeGpu {
            center: self.center.to_array(),
            half_size: self.half_size.to_array(),
            stretch: self.stretch.max(Vec2::splat(0.05)).to_array(),
            radius: self.corner_radius,
            angle: self.stretch_angle,
            // -1: the pass's own tint strength; -2: none of its own (see `join_only`).
            tint_opacity: if self.join_only {
                JOIN_ONLY_TINT
            } else {
                self.tint_opacity.map_or(-1.0, |a| a.clamp(0.0, 1.0))
            },
            glow: self.glow.clamp(0.0, 1.0),
            shadow: self.shadow.clamp(0.0, 1.0),
            _pad: 0.0,
        }
    }
}

fn sd_round_box(p: Vec2, half: Vec2, radius: f32) -> f32 {
    let r = radius.min(half.min_element()).max(0.0);
    let q = p.abs() - half + Vec2::splat(r);
    q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - r
}

/// `ShapeGpu::tint_opacity` of a shape without a tint of its own (`JOIN_ONLY` in `glass.wgsl`).
const JOIN_ONLY_TINT: f32 = -2.0;

/// GPU layout of a shape; must match `Shape` in `glass.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ShapeGpu {
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    pub stretch: [f32; 2],
    pub radius: f32,
    pub angle: f32,
    pub tint_opacity: f32,
    pub glow: f32,
    pub shadow: f32,
    pub _pad: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_distance() {
        let c = GlassShape::circle(Vec2::new(10.0, 10.0), 5.0);
        assert!((c.distance(Vec2::new(10.0, 10.0)) + 5.0).abs() < 1e-4);
        assert!((c.distance(Vec2::new(20.0, 10.0)) - 5.0).abs() < 1e-4);
        assert!(c.contains(Vec2::new(12.0, 12.0)));
    }

    #[test]
    fn gpu_layout_is_48_bytes() {
        assert_eq!(std::mem::size_of::<ShapeGpu>(), 48);
        assert_eq!(std::mem::size_of::<crate::params::ParamsUniform>(), 80);
    }
}
