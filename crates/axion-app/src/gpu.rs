//! GPU setup for the window.

use std::sync::Arc;

use anyhow::Result;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::hardware::{self, AdapterChoice};

/// The GPU objects the browser's windows share.
pub type Gpu = (wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter, wgpu::Device, wgpu::Queue);

/// Creates the surface and a Vulkan device. Picks the card `hardware.gpu` (or `AXION_GPU`)
/// asks for, else the discrete GPU, and enables zero-copy frame import from Chromium when the
/// driver supports it.
pub fn init(event_loop: &ActiveEventLoop, window: &Arc<Window>, choice: &AdapterChoice) -> Result<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        // CEF's frame sharing on Linux needs Vulkan external memory.
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        ))
    });
    let surface = instance.create_surface(window.clone())?;
    let listed = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let summaries: Vec<(String, wgpu::DeviceType)> = listed
        .iter()
        .map(|a| {
            let info = a.get_info();
            (info.name, info.device_type)
        })
        .collect();
    let picked = hardware::pick_adapter(&summaries, choice)
        .and_then(|i| listed.get(i))
        .filter(|a| a.is_surface_supported(&surface))
        .cloned();
    if picked.is_none() && *choice != AdapterChoice::Auto {
        tracing::warn!(?choice, "no matching graphics card; letting wgpu choose");
    }
    let adapter = match picked {
        Some(adapter) => adapter,
        None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: hardware::power_preference(choice),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))?,
    };
    let info = adapter.get_info();
    tracing::info!(adapter = %info.name, driver = %info.driver, "using GPU");
    let mut wanted = axion_engine::cef::required_features();
    // `AXION_PERF_LOG`: GPU time of the window's passes (see `perf_log`).
    if std::env::var_os("AXION_PERF_LOG").is_some() {
        wanted |= wgpu::Features::TIMESTAMP_QUERY;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("axion"),
        required_features: adapter.features() & wanted,
        ..Default::default()
    }))?;
    // wgpu's default handler panics on the first GPU error (a validation error in one frame, an
    // out-of-memory); a panic in the render path kills the browser with every open tab. Log it
    // instead: the frame is wrong at worst, and the next one is drawn again.
    device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
        tracing::error!(%error, "GPU error (frame skipped or wrong; not fatal)");
    }));
    device.set_device_lost_callback(|reason, message| {
        tracing::error!(?reason, %message, "GPU device lost: the window can no longer draw");
    });
    Ok((instance, surface, adapter, device, queue))
}

/// Every Vulkan adapter on this machine.
pub fn adapters(instance: &wgpu::Instance) -> Vec<hardware::AdapterLine> {
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .iter()
        .map(|a| hardware::AdapterLine::of(&a.get_info()))
        .collect()
}
