//! Mid-drag frames of glass controls at several pointer distances, light and dark: a button
//! with an icon (pulled sideways), a switch and a slider (knobs pulled past their ends).
//! Shows the drag curve (strong at first, then resisting, saturating at `glass.drag_stretch_max`)
//! and that the icon stays at its rest place and is stretched like the glass.
//!
//! `cargo run -p axion-ui --example drag_capture --release -- out_prefix`
//! writes `out_prefix_light.png` and `out_prefix_dark.png` (rows: 0, 6, 25, 80, 300 px).

use axion_compositor::{Compositor, Icon, ImageDraw, Layer, Rect as Frame, Scene};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Theme, Ui, UiInput, Widget};
use glam::Vec2;

const SCALE: f32 = 2.0;
/// One cell, logical px.
const CELL: (f32, f32) = (260.0, 110.0);
const DISTANCES: [f32; 5] = [0.0, 6.0, 25.0, 80.0, 300.0];
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Clone, Copy)]
enum Subject {
    Button,
    Switch,
    Slider,
}

fn build(config: &Config, dark: bool, subject: Subject) -> (Ui, Vec2) {
    let mut ui = Ui::new(Theme::new(config, dark), SCALE);
    ui.set_viewport(Vec2::new(CELL.0, CELL.1), SCALE);
    let root = ui.root();
    let at = |x: f32, y: f32, w: f32, h: f32| Style {
        position: Position::Absolute,
        inset: Rect { left: length(x), top: length(y), right: auto(), bottom: auto() },
        size: Size { width: length(w), height: length(h) },
        justify_content: JustifyContent::CENTER,
        align_items: AlignItems::CENTER,
        ..Default::default()
    };
    let grab = match subject {
        Subject::Button => {
            let b = ui.add(
                root,
                Widget::container()
                    .with_background(Background::Glass(GlassLevel::Regular))
                    .with_action(ActionId(1))
                    .with_radius(CAPSULE),
                at(50.0, 27.0, 56.0, 56.0),
            );
            ui.add(b, Widget::icon(Icon::Back, 24.0), Style::default());
            Vec2::new(78.0, 55.0)
        }
        Subject::Switch => {
            ui.add(root, Widget::toggle(true), at(100.0, 41.0, 63.0, 28.0));
            Vec2::new(100.0 + 63.0 - 2.5 - 17.0, 55.0)
        }
        Subject::Slider => {
            ui.add(root, Widget::slider(1.0), at(30.0, 43.0, 150.0, 24.0));
            Vec2::new(30.0 + 150.0 - 12.0, 55.0)
        }
    };
    ui.layout();
    (ui, grab)
}

fn page_pixels(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let stripe = (((x / 18) + (y / 18)) % 2) as f32;
            let c = [0.95 - 0.5 * fx, 0.35 + 0.45 * fy, 0.25 + 0.6 * fx * (1.0 - fy)];
            for v in c {
                px.push(((v * (0.75 + 0.25 * stripe)).clamp(0.0, 1.0) * 255.0) as u8);
            }
            px.push(255);
        }
    }
    px
}

fn main() {
    let prefix = std::env::args().nth(1).unwrap_or_else(|| "drag".into());
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
    .expect("a GPU adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
    let (cw, ch) = ((CELL.0 * SCALE) as u32, (CELL.1 * SCALE) as u32);
    let make = |usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cell"),
            size: wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        })
    };
    let output = make(
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    );
    let output_view = output.create_view(&Default::default());
    let page = make(
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
    );
    queue.write_texture(
        page.as_image_copy(),
        &page_pixels(cw, ch),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * cw), rows_per_image: Some(ch) },
        wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
    );
    let page_view = page.create_view(&Default::default());
    let config = Config::default();
    let row_bytes = (4 * cw).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);

    for (scheme, dark) in [("light", false), ("dark", true)] {
        let subjects = [Subject::Button, Subject::Switch, Subject::Slider];
        let mut montage = image::RgbaImage::new(cw * 3, ch * DISTANCES.len() as u32);
        for (row, distance) in DISTANCES.iter().enumerate() {
            for (col, subject) in subjects.iter().enumerate() {
                let (mut ui, grab) = build(&config, dark, *subject);
                ui.handle(UiInput::PointerMove(grab));
                ui.handle(UiInput::PointerDown);
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    ui.handle(UiInput::PointerMove(grab + Vec2::new(distance * t, 0.0)));
                    ui.animate(1.0 / 120.0);
                }
                for _ in 0..240 {
                    ui.animate(1.0 / 120.0);
                }
                let mut fonts = ui.fonts();
                let mut compositor = Compositor::new(&device, &queue, FORMAT, &mut fonts);
                drop(fonts);
                let mut layers = vec![Layer::Image(ImageDraw::new(
                    &page_view,
                    Frame::new(0.0, 0.0, cw as f32, ch as f32),
                ))];
                ui.paint(&mut layers);
                let scene = Scene { clear: [0.9, 0.9, 0.92, 1.0], layers };
                compositor.render(&scene, &mut ui.fonts(), &output_view, (cw, ch));
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: (row_bytes * ch) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    output.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row_bytes),
                            rows_per_image: Some(ch),
                        },
                    },
                    wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
                );
                queue.submit([encoder.finish()]);
                buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                let data = buffer.slice(..).get_mapped_range().expect("mapped");
                for y in 0..ch {
                    let start = (y * row_bytes) as usize;
                    for x in 0..cw {
                        let i = start + 4 * x as usize;
                        montage.put_pixel(
                            col as u32 * cw + x,
                            row as u32 * ch + y,
                            image::Rgba([data[i], data[i + 1], data[i + 2], 255]),
                        );
                    }
                }
            }
        }
        let path = format!("{prefix}_{scheme}.png");
        montage.save(&path).expect("saving the montage");
        println!("wrote {path}");
    }
}
