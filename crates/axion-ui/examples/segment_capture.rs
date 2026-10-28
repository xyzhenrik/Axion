//! A segmented control over a busy page at rest, held and 20/60/120 ms/settled after a plain
//! click on another segment, light and dark. Shows that the label under the pill stays visible
//! through it and that a click animates the pill.
//!
//! `cargo run -p axion-ui --example segment_capture --release -- out_prefix`
//! writes `out_prefix_light.png` and `out_prefix_dark.png` (one row per state).

use axion_compositor::{Compositor, ImageDraw, Layer, Rect as Area, Scene};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Theme, Ui, UiInput, Widget};
use glam::Vec2;

const SCALE: f32 = 2.0;
const CELL: (f32, f32) = (330.0, 56.0);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn build(config: &Config, dark: bool) -> (Ui, Vec<Vec2>) {
    let mut ui = Ui::new(Theme::new(config, dark), SCALE);
    ui.set_viewport(Vec2::new(CELL.0, CELL.1), SCALE);
    let root = ui.root();
    let seg = ui.add(
        root,
        Widget::segmented(0).with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(20.0), top: length(10.0), right: auto(), bottom: auto() },
            size: Size { width: auto(), height: length(36.0) },
            flex_direction: FlexDirection::Row,
            padding: Rect { left: length(3.0), right: length(3.0), top: length(3.0), bottom: length(3.0) },
            gap: Size { width: length(2.0), height: length(0.0) },
            ..Default::default()
        },
    );
    let mut nodes = Vec::new();
    for (i, label) in ["Auto", "Hell", "Dunkel"].iter().enumerate() {
        let c = ui.add(
            seg,
            Widget::container().with_action(ActionId(i as u32 + 1)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(30.0) },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        ui.add_label(c, label, TextStyle::Body, Style::default());
        nodes.push(c);
    }
    if std::env::var("SUBJECT").as_deref() == Ok("switch") {
        // A switch on a flat card: its held knob should show the card's edge through it.
        let card = ui.add(
            root,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(220.0), top: length(10.0), right: auto(), bottom: auto() },
                size: Size { width: length(100.0), height: length(36.0) },
                ..Default::default()
            },
        );
        let t = ui.add(
            card,
            Widget::toggle(false),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(18.0), top: length(4.0), right: auto(), bottom: auto() },
                size: Size { width: length(63.0), height: length(28.0) },
                ..Default::default()
            },
        );
        ui.layout();
        let c = ui.frame(t).center();
        return (ui, vec![c, c, c]);
    }
    ui.layout();
    let centres = nodes.iter().map(|&c| ui.frame(c).center()).collect();
    (ui, centres)
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
    let prefix = std::env::args().nth(1).unwrap_or_else(|| "segment".into());
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
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
    // (held, ms): held = pointer down on segment 0 for `ms`; else a click on segment 2 and `ms`.
    let states: [(bool, u32); 6] =
        [(false, 0), (true, 400), (false, 20), (false, 60), (false, 120), (false, 1500)];
    for (scheme, dark) in [("light", false), ("dark", true)] {
        let mut montage = image::RgbaImage::new(cw, ch * states.len() as u32);
        for (row, (held, ms)) in states.iter().enumerate() {
            let (mut ui, centres) = build(&config, dark);
            if *held {
                ui.handle(UiInput::PointerMove(centres[0]));
                ui.handle(UiInput::PointerDown);
                for _ in 0..(ms / 8) {
                    ui.animate(0.008);
                }
            } else if *ms > 0 {
                ui.handle(UiInput::PointerMove(centres[2]));
                ui.handle(UiInput::PointerDown);
                ui.handle(UiInput::PointerUp);
                for _ in 0..(ms / 4) {
                    ui.animate(0.004);
                }
            }
            let mut fonts = ui.fonts();
            let mut compositor = Compositor::new(&device, &queue, FORMAT, &mut fonts);
            drop(fonts);
            let mut layers =
                vec![Layer::Image(ImageDraw::new(&page_view, Area::new(0.0, 0.0, cw as f32, ch as f32)))];
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
                        x,
                        row as u32 * ch + y,
                        image::Rgba([data[i], data[i + 1], data[i + 2], 255]),
                    );
                }
            }
        }
        let path = format!("{prefix}_{scheme}.png");
        montage.save(&path).expect("saving the montage");
        println!("wrote {path}");
    }
}
