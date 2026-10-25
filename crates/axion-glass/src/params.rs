use bytemuck::{Pod, Zeroable};
use glam::Vec2;

/// Look of the glass material. Every field maps to a user setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassParams {
    /// Maximum refraction displacement at the rim, in pixels. Negative flips the lens.
    pub refraction: f32,
    /// Width of the curved bezel in pixels; inside it the glass is flat and undistorted.
    pub bezel_width: f32,
    /// Chromatic dispersion: 0 = none, 0.3 = clearly visible colour fringes at the rim.
    pub dispersion: f32,
    /// Smooth-union radius in pixels: how far apart two shapes start to connect.
    pub merge_smoothness: f32,
    /// 0 = perfectly clear glass, 1 = fully frosted (blurred backdrop).
    pub frost: f32,
    /// Blur radius of the frosted backdrop, in pixels.
    pub blur_radius: f32,
    /// Tint colour (linear RGB) and its strength in `a`.
    pub tint: [f32; 4],
    /// Strength of the specular rim highlight.
    pub rim: f32,
    /// Strength of the drop shadow.
    pub shadow: f32,
    /// Direction the light comes from, in screen space (y down).
    pub light_dir: Vec2,
}

impl Default for GlassParams {
    fn default() -> Self {
        Self {
            refraction: 30.0,
            bezel_width: 22.0,
            dispersion: 0.25,
            merge_smoothness: 20.0,
            frost: 0.3,
            blur_radius: 18.0,
            tint: [1.0, 1.0, 1.0, 0.06],
            rim: 0.55,
            shadow: 0.4,
            light_dir: Vec2::new(-0.55, -0.85),
        }
    }
}

impl GlassParams {
    /// Dark-mode default: glass darkens the backdrop slightly instead of brightening it.
    pub fn dark() -> Self {
        Self { tint: [0.02, 0.02, 0.03, 0.18], rim: 0.4, ..Self::default() }
    }
}

/// GPU layout of the parameters; must match `Params` in `glass.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ParamsUniform {
    pub tint: [f32; 4],
    pub resolution: [f32; 2],
    pub light_dir: [f32; 2],
    pub refraction: f32,
    pub bezel: f32,
    pub dispersion: f32,
    pub merge_k: f32,
    pub rim: f32,
    pub shadow: f32,
    pub frost: f32,
    pub shape_count: u32,
    /// 1 when some shape has its own tint strength (the shader blends them only then).
    pub own_tints: u32,
    pub _pad: [u32; 3],
}

impl ParamsUniform {
    pub fn new(params: &GlassParams, resolution: [f32; 2], shape_count: u32, own_tints: bool) -> Self {
        Self {
            tint: params.tint,
            resolution,
            light_dir: params.light_dir.normalize_or(Vec2::NEG_Y).to_array(),
            refraction: params.refraction,
            bezel: params.bezel_width.max(1.0),
            dispersion: params.dispersion,
            merge_k: params.merge_smoothness.max(0.0),
            rim: params.rim,
            shadow: params.shadow,
            frost: params.frost.clamp(0.0, 1.0),
            shape_count,
            own_tints: own_tints as u32,
            _pad: [0; 3],
        }
    }
}
