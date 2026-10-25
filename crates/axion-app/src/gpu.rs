//! GPU setup for the window.

use std::sync::Arc;

use anyhow::Result;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// Creates the surface and a Vulkan device. Picks the discrete GPU unless `AXION_GPU=low`, and
/// enables zero-copy frame import from Chromium when the driver supports it.
pub fn init(
    event_loop: &ActiveEventLoop,
    window: &Arc<Window>,
) -> Result<(wgpu::Surface<'static>, wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        // CEF's frame sharing on Linux needs Vulkan external memory.
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        ))
    });
    let surface = instance.create_surface(window.clone())?;
    let power_preference = match std::env::var("AXION_GPU").as_deref() {
        Ok("low") => wgpu::PowerPreference::LowPower,
        _ => wgpu::PowerPreference::HighPerformance,
    };
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))?;
    let info = adapter.get_info();
    tracing::info!(adapter = %info.name, driver = %info.driver, "using GPU");
    let wanted = axion_engine::cef::required_features();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("axion"),
        required_features: adapter.features() & wanted,
        ..Default::default()
    }))?;
    Ok((surface, adapter, device, queue))
}
