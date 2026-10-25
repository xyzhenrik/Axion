//! Liquid Glass material for Axion.
//!
//! Glass elements are described as signed distance fields ([`GlassShape`]). The
//! [`GlassRenderer`] composites them over a backdrop texture (the rendered page and any UI
//! beneath), refracting and blurring what's behind them:
//!
//! 1. The backdrop is blurred at quarter resolution (separable Gaussian).
//! 2. A full-screen pass evaluates the combined SDF of all shapes. Shapes are merged with a
//!    smooth minimum, so elements that come close flow into each other like liquid.
//! 3. Inside the shapes, the SDF gradient gives a surface normal; a circular bezel profile
//!    turns that into a refraction offset, with per-channel dispersion, frost, tint, a
//!    specular rim and a soft drop shadow outside.

mod params;
mod renderer;
mod shape;

pub use params::GlassParams;
pub use renderer::{GlassFrame, GlassRenderer, MAX_SHAPES};
pub use shape::GlassShape;

#[cfg(test)]
mod tests {
    use wgpu::naga;

    #[test]
    fn shaders_are_valid() {
        for (name, source) in [
            ("glass.wgsl", include_str!("../shaders/glass.wgsl")),
            ("blur.wgsl", include_str!("../shaders/blur.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}
