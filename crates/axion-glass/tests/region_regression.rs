//! The glass passes only run where shapes are (scissored blur and composite, drawn in place).
//! This renders scenes both ways on the GPU and requires identical pixels: the full-screen
//! path (`set_full_screen(true)`, the original behaviour) is the reference.
//!
//! `cargo test -p axion-glass --release --test region_regression -- --nocapture`
//! (`AXION_GPU=low` picks the integrated GPU; the test passes silently without any GPU).

use axion_glass::{GlassFrame, GlassInPlace, GlassParams, GlassRenderer, GlassShape};
use glam::Vec2;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Not a multiple of the blur downsample on purpose.
const SIZE: (u32, u32) = (1003, 587);
/// Allowed per-channel difference (0 = bit identical).
const TOLERANCE: u8 = 0;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let power_preference = match std::env::var("AXION_GPU").as_deref() {
        Ok("low") => wgpu::PowerPreference::LowPower,
        _ => wgpu::PowerPreference::HighPerformance,
    };
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let name = adapter.get_info().name;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(Gpu { device, queue, name })
}

/// A busy backdrop: high-frequency noise over colour gradients, so any blur or sampling
/// difference shows.
fn backdrop_pixels(seed: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((SIZE.0 * SIZE.1 * 4) as usize);
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let mut h = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263) ^ seed.wrapping_mul(2246822519);
            h = (h ^ (h >> 13)).wrapping_mul(1274126177);
            let noise = (h >> 24) & 0xff;
            let band = if (x / 37 + y / 23) % 2 == 0 { 40 } else { 0 };
            out.push(((x * 255 / SIZE.0) + noise / 4 + band).min(255) as u8);
            out.push(((y * 255 / SIZE.1) + noise / 3).min(255) as u8);
            out.push(((255 - x * 255 / SIZE.0) / 2 + noise / 2 + band).min(255) as u8);
            out.push(255);
        }
    }
    out
}

fn texture(gpu: &Gpu, label: &str, pixels: Option<&[u8]>) -> wgpu::Texture {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    if let Some(pixels) = pixels {
        gpu.queue.write_texture(
            texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE.0 * 4), rows_per_image: None },
            wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
        );
    }
    texture
}

fn read(gpu: &Gpu, texture: &wgpu::Texture) -> Vec<u8> {
    let row = (4 * SIZE.0).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read"),
        size: (row * SIZE.1) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
        },
        wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| assert!(r.is_ok()));
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("mapped");
    let mut out = Vec::new();
    for y in 0..SIZE.1 as usize {
        out.extend_from_slice(&data[y * row as usize..y * row as usize + (4 * SIZE.0) as usize]);
    }
    out
}

type Layer = (Vec<GlassShape>, GlassParams);

/// Reference: the original full-screen passes, ping-ponging between two textures.
fn render_reference(gpu: &Gpu, seed: u32, layers: &[Layer]) -> Vec<u8> {
    let pixels = backdrop_pixels(seed);
    let textures = [texture(gpu, "a", Some(&pixels)), texture(gpu, "b", None)];
    let views = [textures[0].create_view(&Default::default()), textures[1].create_view(&Default::default())];
    let mut renderer = GlassRenderer::new(&gpu.device, FORMAT);
    renderer.set_full_screen(true);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut current = 0;
    for (shapes, params) in layers {
        renderer.render(
            &gpu.device,
            &mut encoder,
            GlassFrame {
                backdrop: &views[current],
                backdrop_size: SIZE,
                target: &views[1 - current],
                shapes,
                params,
            },
        );
        current = 1 - current;
    }
    gpu.queue.submit([encoder.finish()]);
    read(gpu, &textures[current])
}

/// Same with the scissored blur but a full composite into a separate target.
fn render_separate(gpu: &Gpu, seed: u32, layers: &[Layer]) -> Vec<u8> {
    let pixels = backdrop_pixels(seed);
    let textures = [texture(gpu, "a", Some(&pixels)), texture(gpu, "b", None)];
    let views = [textures[0].create_view(&Default::default()), textures[1].create_view(&Default::default())];
    let mut renderer = GlassRenderer::new(&gpu.device, FORMAT);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut current = 0;
    for (shapes, params) in layers {
        renderer.render(
            &gpu.device,
            &mut encoder,
            GlassFrame {
                backdrop: &views[current],
                backdrop_size: SIZE,
                target: &views[1 - current],
                shapes,
                params,
            },
        );
        current = 1 - current;
    }
    gpu.queue.submit([encoder.finish()]);
    read(gpu, &textures[current])
}

/// The compositor's path: scissored blur, scissored composite drawn over the backdrop.
fn render_in_place(gpu: &Gpu, seed: u32, layers: &[Layer]) -> Vec<u8> {
    let pixels = backdrop_pixels(seed);
    let target = texture(gpu, "target", Some(&pixels));
    let view = target.create_view(&Default::default());
    let mut renderer = GlassRenderer::new(&gpu.device, FORMAT);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    for (shapes, params) in layers {
        renderer.render_in_place(
            &gpu.device,
            &mut encoder,
            GlassInPlace { texture: &target, view: &view, shapes, params },
        );
    }
    gpu.queue.submit([encoder.finish()]);
    read(gpu, &target)
}

struct Diff {
    max: u8,
    count: usize,
    first: Option<(u32, u32)>,
    bbox: (u32, u32, u32, u32),
}

fn diff(a: &[u8], b: &[u8]) -> Diff {
    let mut d = Diff { max: 0, count: 0, first: None, bbox: (u32::MAX, u32::MAX, 0, 0) };
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let e = x.abs_diff(*y);
        if e > 0 {
            d.count += 1;
            d.max = d.max.max(e);
            let px = (i / 4) as u32;
            let (x, y) = (px % SIZE.0, px / SIZE.0);
            d.first.get_or_insert((x, y));
            d.bbox = (d.bbox.0.min(x), d.bbox.1.min(y), d.bbox.2.max(x), d.bbox.3.max(y));
        }
    }
    d
}

fn v(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

fn scenes() -> Vec<(&'static str, Vec<Layer>)> {
    let light = GlassParams::default();
    let dark = GlassParams::dark();
    let strong = GlassParams { refraction: 60.0, dispersion: 0.5, blur_radius: 40.0, frost: 1.0, ..dark };
    let flipped = GlassParams { refraction: -45.0, merge_smoothness: 40.0, ..light };
    let clear = GlassParams { frost: 0.0, ..light };

    let buttons: Vec<_> =
        (0..6).map(|i| GlassShape::circle(v(80.0 + 150.0 * i as f32, 60.0), 22.0 + 2.0 * i as f32)).collect();

    let mut merged = vec![
        GlassShape::capsule(v(300.0, 300.0), v(140.0, 30.0)),
        GlassShape::capsule(v(470.0, 330.0), v(40.0, 40.0)),
        GlassShape::rounded_rect(v(560.0, 420.0), v(90.0, 60.0), 20.0),
    ];
    merged[1].tint_opacity = Some(0.4);
    merged[2].glow = 0.7;
    let mut bridge = GlassShape::rounded_rect(v(420.0, 360.0), v(150.0, 50.0), 25.0);
    bridge.merge = Some(60.0);
    bridge.join_only = true;
    merged.push(bridge);

    let sheet = vec![GlassShape::rounded_rect(v(260.0, 300.0), v(240.0, 270.0), 30.0)];
    let mut sheet_and_buttons = sheet.clone();
    sheet_and_buttons.push(GlassShape::capsule(v(700.0, 40.0), v(200.0, 24.0)));
    sheet_and_buttons.extend((0..4).map(|i| GlassShape::circle(v(640.0 + 70.0 * i as f32, 500.0), 22.0)));

    let edges = vec![
        GlassShape::circle(v(2.0, 3.0), 30.0),
        GlassShape::circle(v(SIZE.0 as f32 - 4.0, SIZE.1 as f32 - 2.0), 40.0),
        GlassShape::capsule(v(500.0, -10.0), v(120.0, 30.0)),
        GlassShape::rounded_rect(v(-30.0, 300.0), v(60.0, 90.0), 20.0),
        GlassShape::circle(v(-400.0, -400.0), 30.0),
        GlassShape::circle(v(5000.0, 300.0), 30.0),
        GlassShape::rounded_rect(v(SIZE.0 as f32 + 20.0, SIZE.1 as f32 - 100.0), v(80.0, 40.0), 20.0),
    ];

    let animated = |t: f32| {
        let mut a = GlassShape::capsule(v(300.0 + 200.0 * t, 250.0 + 40.0 * (t * 6.0).sin()), v(70.0, 28.0));
        a.stretch = v(1.0 + 0.6 * t, 1.0 / (1.0 + 0.6 * t));
        a.stretch_angle = 0.4 + 2.0 * t;
        let mut b = GlassShape::circle(v(520.0, 270.0), 34.0 * (1.0 - 0.4 * t));
        b.stretch = v(0.7, 1.4);
        b.stretch_angle = 5.0 * t;
        vec![a, b]
    };

    let many: Vec<_> = (0..40)
        .map(|i| GlassShape::circle(v(30.0 + 25.0 * i as f32, 100.0 + 12.0 * (i % 7) as f32 * 6.0), 12.0))
        .collect();

    let mut scenes = vec![
        ("small buttons", vec![(buttons.clone(), light)]),
        ("small buttons dark strong", vec![(buttons, strong)]),
        ("merged shapes with bridge, tints, glow", vec![(merged.clone(), light)]),
        ("merged, flipped lens, wide merge", vec![(merged, flipped)]),
        ("large sheet", vec![(sheet.clone(), dark)]),
        ("sheet and far-apart buttons", vec![(sheet_and_buttons, light)]),
        ("edges and off-screen shapes", vec![(edges.clone(), light), (edges, strong)]),
        ("40 shapes (over the cap)", vec![(many, light)]),
        ("no shapes", vec![(Vec::new(), light)]),
        ("clear glass (no frost)", vec![(sheet, clear)]),
    ];
    for (i, t) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
        let name = ["animated t=0", "animated t=.25", "animated t=.5", "animated t=.75", "animated t=1"][i];
        scenes.push((name, vec![(animated(t), light)]));
    }
    // Overlapping layers: glass on glass, the upper layer samples the lower one's output.
    let lower = vec![GlassShape::rounded_rect(v(400.0, 280.0), v(300.0, 200.0), 36.0)];
    let upper = vec![
        GlassShape::capsule(v(330.0, 250.0), v(120.0, 30.0)),
        GlassShape::circle(v(560.0, 330.0), 40.0),
        GlassShape::circle(v(60.0, 540.0), 30.0),
    ];
    let top = vec![GlassShape::capsule(v(450.0, 280.0), v(200.0, 60.0))];
    scenes.push(("overlapping layers", vec![(lower, dark), (upper, light), (top, strong)]));
    scenes
}

#[test]
fn scissored_passes_match_full_screen() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    eprintln!("adapter: {}", gpu.name);
    let mut worst = 0u8;
    let mut failures = Vec::new();
    for (i, (name, layers)) in scenes().iter().enumerate() {
        let reference = render_reference(&gpu, i as u32, layers);
        let separate = diff(&reference, &render_separate(&gpu, i as u32, layers));
        let in_place = diff(&reference, &render_in_place(&gpu, i as u32, layers));
        eprintln!(
            "{name:42} separate: max {} ({} values)   in place: max {} ({} values)",
            separate.max, separate.count, in_place.max, in_place.count
        );
        worst = worst.max(separate.max).max(in_place.max);
        if separate.max > TOLERANCE || in_place.max > TOLERANCE {
            failures.push(format!(
                "{name}: separate {:?} in place {:?}, differing pixels within {:?}",
                separate.first, in_place.first, in_place.bbox
            ));
        }
    }
    eprintln!("worst per-channel difference: {worst}/255");
    assert!(failures.is_empty(), "scissored output differs: {failures:?}");
}
