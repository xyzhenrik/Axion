//! GPU time per 1440p frame of a browser-like chrome (sidebar with rows, toolbar groups, a
//! popover) over a page texture, in Liquid Glass and in flat mode. Flat mode skips every glass
//! pass (no backdrop copies, no blur chain), so it is the performance mode for weak GPUs.
//!
//! `cargo run -p axion-ui --example flat_bench --release` (`AXION_GPU=low`: the integrated GPU).

use std::time::Instant;

use axion_compositor::{Compositor, ImageDraw, Layer, Rect as Frame, Scene};
use axion_config::{Config, GlassStyle};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, TextStyle, Theme, Ui, Widget};
use glam::Vec2;

const SIZE: (u32, u32) = (2560, 1440);
const SCALE: f32 = 1.5;
const WARMUP: u32 = 30;
const FRAMES: u32 = 200;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn build(config: &Config) -> Ui {
    let mut ui = Ui::new(Theme::new(config, false), SCALE);
    ui.set_viewport(Vec2::new(SIZE.0 as f32 / SCALE, SIZE.1 as f32 / SCALE), SCALE);
    let root = ui.root();
    let abs = |l: f32, t: f32, w: f32, h: f32| Style {
        position: Position::Absolute,
        inset: Rect { left: length(l), top: length(t), right: auto(), bottom: auto() },
        size: Size { width: length(w), height: length(h) },
        flex_direction: FlexDirection::Column,
        padding: Rect { left: length(12.0), right: length(12.0), top: length(12.0), bottom: length(12.0) },
        gap: Size { width: length(0.0), height: length(4.0) },
        ..Default::default()
    };
    let sidebar = ui.add(
        root,
        Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(26.0),
        abs(16.0, 16.0, 316.0, 928.0),
    );
    for i in 0..14 {
        let row = ui.add(
            sidebar,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style { size: Size { width: auto(), height: length(40.0) }, ..Default::default() },
        );
        ui.add_label(row, &format!("Tab number {i}"), TextStyle::Body, Style::default());
    }
    for (x, w) in [(348.0, 140.0), (504.0, 700.0), (1220.0, 220.0)] {
        let group = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            abs(x, 16.0, w, 46.0),
        );
        ui.add_label(group, "example.com", TextStyle::Body, Style::default());
    }
    let popover = ui.add(
        root,
        Widget::container()
            .with_background(Background::Glass(GlassLevel::Thin))
            .with_glass_density(GlassLevel::Thick)
            .with_radius(22.0),
        abs(900.0, 76.0, 300.0, 300.0),
    );
    for i in 0..6 {
        ui.add_label(popover, &format!("Menu item {i}"), TextStyle::Body, Style::default());
    }
    ui.layout();
    ui
}

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let power_preference = match std::env::var("AXION_GPU").as_deref() {
        Ok("low") => wgpu::PowerPreference::LowPower,
        _ => wgpu::PowerPreference::HighPerformance,
    };
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    })) else {
        eprintln!("no GPU adapter");
        return;
    };
    let name = adapter.get_info().name;
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
    else {
        eprintln!("no device");
        return;
    };
    let make = |usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("bench"),
                size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let output = make(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let page = make(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);

    for (label, style) in [("liquid", GlassStyle::Liquid), ("flat", GlassStyle::Flat)] {
        let mut config = Config::default();
        config.glass.style = style;
        let ui = build(&config);
        let mut fonts = ui.fonts();
        let mut compositor = Compositor::new(&device, &queue, FORMAT, &mut fonts);
        drop(fonts);
        let mut start = Instant::now();
        let mut glass_layers = 0;
        for i in 0..WARMUP + FRAMES {
            if i == WARMUP {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                start = Instant::now();
            }
            let mut layers = vec![Layer::Image(ImageDraw::new(
                &page,
                Frame::new(332.0, 80.0, SIZE.0 as f32 - 332.0, SIZE.1 as f32 - 80.0),
            ))];
            ui.paint(&mut layers);
            glass_layers = layers.iter().filter(|l| matches!(l, Layer::Glass { .. })).count();
            let scene = Scene { clear: [0.9, 0.9, 0.92, 1.0], layers };
            compositor.render(&scene, &mut ui.fonts(), &output, SIZE);
        }
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let ms = start.elapsed().as_secs_f32() * 1000.0 / FRAMES as f32;
        println!(
            "{name}: {label:6} 2560x1440 {ms:6.2} ms/frame ({glass_layers} glass passes), {:.0} fps",
            1000.0 / ms
        );
    }
}
