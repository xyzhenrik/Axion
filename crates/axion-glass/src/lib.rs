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
