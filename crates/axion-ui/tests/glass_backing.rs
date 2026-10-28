//! Pixel test of the "every glass button is pure glass" rule: a glass button over a flat backdrop
//! must not change a single pixel outside its own shape (no halo, plate, shadow or bridge behind
//! or around it). Renders offscreen with the real compositor and compares the frame with the
//! same backdrop without the widget. Skipped when there is no GPU adapter.

use axion_compositor::{Compositor, Layer, Scene};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Theme, Ui, Widget};
use glam::Vec2;

const SIZE: (u32, u32) = (480, 240);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Pixels beyond the button's frame that may change: the glass's own rim light and anti-aliasing.
const RIM_ALLOWANCE: f32 = 2.0;
/// Largest channel difference outside that allowance (rounding of the backdrop copy).
const TOLERANCE: i32 = 2;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: wgpu::TextureView,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    let target = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("glass backing test"),
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default());
    Some(Gpu { device, queue, target })
}

/// One button: a glass capsule with a label, centred in the frame.
fn button(
    config: &Config,
    dark: bool,
    background: Background,
    level: Option<GlassLevel>,
) -> (Ui, Vec2, Vec2) {
    button_with(config, dark, background, level, true)
}

fn button_with(
    config: &Config,
    dark: bool,
    background: Background,
    level: Option<GlassLevel>,
    action: bool,
) -> (Ui, Vec2, Vec2) {
    let mut ui = Ui::new(Theme::new(config, dark), 1.0);
    ui.set_viewport(Vec2::new(SIZE.0 as f32, SIZE.1 as f32), 1.0);
    let root = ui.root();
    let mut widget = Widget::container().with_radius(CAPSULE).with_background(background);
    if action {
        widget = widget.with_action(ActionId(7));
    }
    if let Some(level) = level {
        widget = widget.with_glass_density(level);
    }
    let node = ui.add(
        root,
        widget,
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(150.0), top: length(100.0), right: auto(), bottom: auto() },
            size: Size { width: length(180.0), height: length(40.0) },
            align_items: AlignItems::CENTER,
            justify_content: JustifyContent::CENTER,
            ..Default::default()
        },
    );
    ui.add_label(node, "Retry", TextStyle::Callout, Style::default());
    ui.layout();
    let frame = ui.frame(node);
    (ui, frame.origin, frame.size)
}

fn render(gpu: &Gpu, ui: Option<&Ui>, clear: f32) -> Vec<u8> {
    let empty;
    let fonts_from = match ui {
        Some(ui) => ui,
        None => {
            empty = Ui::new(Theme::new(&Config::default(), false), 1.0);
            &empty
        }
    };
    let mut fonts = fonts_from.fonts();
    let mut compositor = Compositor::new(&gpu.device, &gpu.queue, FORMAT, &mut fonts);
    let mut layers = Vec::new();
    if let Some(ui) = ui {
        ui.paint(&mut layers);
    }
    let scene = Scene { clear: [clear, clear, clear, 1.0], layers: layers as Vec<Layer> };
    compositor.render(&scene, &mut fonts, &gpu.target, SIZE);
    compositor.read_back().map(|(_, _, pixels)| pixels).unwrap_or_default()
}

/// The largest channel difference between two frames outside `origin`..`origin + size` grown by
/// `margin`, with the position of that pixel.
fn worst_outside(a: &[u8], b: &[u8], origin: Vec2, size: Vec2, margin: f32) -> (i32, (u32, u32)) {
    let mut worst = (0, (0, 0));
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = px >= origin.x - margin
                && px <= origin.x + size.x + margin
                && py >= origin.y - margin
                && py <= origin.y + size.y + margin;
            if inside {
                continue;
            }
            let i = ((y * SIZE.0 + x) * 4) as usize;
            let d = (0..3).map(|c| (a[i + c] as i32 - b[i + c] as i32).abs()).max().unwrap_or(0);
            if d > worst.0 {
                worst = (d, (x, y));
            }
        }
    }
    worst
}

#[test]
fn glass_buttons_leave_the_backdrop_untouched_outside_their_shape() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let config = Config::default();
    let cases: Vec<(&str, Background, Option<GlassLevel>)> = vec![
        ("thin", Background::Glass(GlassLevel::Thin), None),
        ("regular", Background::Glass(GlassLevel::Regular), None),
        ("thick", Background::Glass(GlassLevel::Thin), Some(GlassLevel::Thick)),
        ("tinted", Background::TintedGlass(GlassLevel::Thin, Paint::Accent), None),
    ];
    let mut failures = Vec::new();
    for dark in [true, false] {
        for backdrop in [0.0_f32, 0.12, 0.5, 1.0] {
            let baseline = render(&gpu, None, backdrop);
            assert_eq!(baseline.len(), (SIZE.0 * SIZE.1 * 4) as usize, "read back");
            for (name, background, level) in &cases {
                let (ui, origin, size) = button(&config, dark, *background, *level);
                let frame = render(&gpu, Some(&ui), backdrop);
                let (diff, at) = worst_outside(&frame, &baseline, origin, size, RIM_ALLOWANCE);
                if diff > TOLERANCE {
                    failures.push(format!(
                        "{name} (dark={dark}, backdrop {backdrop}): pixel {at:?} differs by {diff} outside the button"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "glass leaves marks outside its shape:\n{}", failures.join("\n"));
}

/// The test above must be able to see a shadow: a glass *panel* (no action) still casts one.
#[test]
fn glass_panels_keep_their_shadow() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let config = Config::default();
    let baseline = render(&gpu, None, 0.5);
    let (ui, origin, size) = button_with(&config, true, Background::Glass(GlassLevel::Regular), None, false);
    let frame = render(&gpu, Some(&ui), 0.5);
    let (diff, _) = worst_outside(&frame, &baseline, origin, size, RIM_ALLOWANCE);
    assert!(diff > 8, "a panel's drop shadow vanished (difference {diff})");
}
