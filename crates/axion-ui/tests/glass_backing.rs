//! Pixel test of the "every glass button is pure glass" rule: a glass button over a flat backdrop
//! must not change a single pixel outside its own shape (no halo, plate, shadow or bridge behind
//! or around it). Renders offscreen with the real compositor and compares the frame with the
//! same backdrop without the widget. Skipped when there is no GPU adapter.

use axion_compositor::{Compositor, Layer, Scene};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Theme, Ui, UiInput, Widget};
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

/// A glass pill (a control group: a panel without an action, as the Playlist's volume capsule)
/// with a round glass button inside it, or, for the baseline, the same layout with a plain empty
/// slot where the button is. Returns the button's frame.
fn nested(config: &Config, dark: bool, button: bool, level: GlassLevel) -> (Ui, Vec2, Vec2) {
    let mut ui = Ui::new(Theme::new(config, dark), 1.0);
    ui.set_viewport(Vec2::new(SIZE.0 as f32, SIZE.1 as f32), 1.0);
    let root = ui.root();
    let pill = ui.add(
        root,
        Widget::container()
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thin))
            .with_button_tint(),
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(100.0), top: length(80.0), right: auto(), bottom: auto() },
            size: Size { width: length(280.0), height: length(80.0) },
            align_items: AlignItems::CENTER,
            justify_content: JustifyContent::CENTER,
            ..Default::default()
        },
    );
    let mut slot = Widget::container().with_radius(CAPSULE);
    if button {
        slot = slot.with_background(Background::Glass(level)).with_action(ActionId(9));
    }
    let node = ui.add(
        pill,
        slot,
        Style { size: Size { width: length(64.0), height: length(64.0) }, ..Default::default() },
    );
    ui.layout();
    let frame = ui.frame(node);
    (ui, frame.origin, frame.size)
}

/// A glass button inside a glass container (the Playlist's play button on a glass row, a button
/// in a popover) must not mark anything outside its own shape either: no second darker lens,
/// halo or plate around it, in the container or beyond.
#[test]
fn glass_buttons_inside_glass_leave_the_container_untouched_outside_their_shape() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let config = Config::default();
    let mut failures = Vec::new();
    for dark in [true, false] {
        for backdrop in [0.0_f32, 0.12, 0.5, 1.0] {
            for level in [GlassLevel::Thin, GlassLevel::Regular, GlassLevel::Thick] {
                let (base_ui, ..) = nested(&config, dark, false, level);
                let baseline = render(&gpu, Some(&base_ui), backdrop);
                let (ui, origin, size) = nested(&config, dark, true, level);
                let frame = render(&gpu, Some(&ui), backdrop);
                let (diff, at) = worst_outside(&frame, &baseline, origin, size, RIM_ALLOWANCE);
                if diff > TOLERANCE {
                    failures.push(format!(
                        "{level:?} (dark={dark}, backdrop {backdrop}): pixel {at:?} differs by {diff} outside the nested button"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "nested glass leaves marks outside its shape:\n{}", failures.join("\n"));
}

/// A round glass button of `side` px, centred in the frame.
fn round_button(config: &Config, dark: bool, level: GlassLevel, side: f32) -> (Ui, Vec2, Vec2) {
    let mut ui = Ui::new(Theme::new(config, dark), 1.0);
    ui.set_viewport(Vec2::new(SIZE.0 as f32, SIZE.1 as f32), 1.0);
    let root = ui.root();
    let node = ui.add(
        root,
        Widget::container()
            .with_radius(CAPSULE)
            .with_background(Background::Glass(level))
            .with_action(ActionId(7)),
        Style {
            position: Position::Absolute,
            inset: Rect {
                left: length((SIZE.0 as f32 - side) / 2.0),
                top: length((SIZE.1 as f32 - side) / 2.0),
                right: auto(),
                bottom: auto(),
            },
            size: Size { width: length(side), height: length(side) },
            ..Default::default()
        },
    );
    ui.layout();
    let frame = ui.frame(node);
    (ui, frame.origin, frame.size)
}

/// Largest channel lift over the backdrop inside a round button's shape, from its middle out
/// to `inner` of its radius (the rim light and highlight band at the edge are left out).
fn worst_lift_inside(frame: &[u8], origin: Vec2, size: Vec2, backdrop: u8, inner: f32) -> (i32, (u32, u32)) {
    let centre = origin + size / 2.0;
    let reach = size.x / 2.0 * inner;
    let mut worst = (0, (0, 0));
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let d = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - centre;
            if d.length() > reach {
                continue;
            }
            let i = ((y * SIZE.0 + x) * 4) as usize;
            let lift = (0..3).map(|c| frame[i + c] as i32 - backdrop as i32).max().unwrap_or(0);
            if lift > worst.0 {
                worst = (lift, (x, y));
            }
        }
    }
    worst
}

/// A glass button over a black (or near-black) backdrop is nearly invisible inside its shape:
/// the backdrop plus a few levels, never a grey disc. Checked at rest and with the pointer on it
/// (a hover must not paint a plate on glass either).
#[test]
fn glass_button_over_black_is_not_a_grey_disc() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let config = Config::default();
    // The most a button may lift its backdrop: its tint (`glass.button_opacity` of white over a
    // light scheme's glass) plus rounding. The dark scheme's tint is nearly black.
    let allowed = |dark: bool| if dark { 4 } else { (config.glass.button_opacity * 255.0).ceil() as i32 + 3 };
    let mut failures = Vec::new();
    for dark in [true, false] {
        for backdrop in [0.0_f32, 0.06, 0.12] {
            for level in [GlassLevel::Thin, GlassLevel::Regular] {
                for hover in [false, true] {
                    let (mut ui, origin, size) = round_button(&config, dark, level, 68.0);
                    if hover {
                        let _ = ui.handle(axion_ui::UiInput::PointerMove(origin + size / 2.0));
                    }
                    let frame = render(&gpu, Some(&ui), backdrop);
                    let level_255 = (backdrop * 255.0).round() as u8;
                    let (lift, at) = worst_lift_inside(&frame, origin, size, level_255, 0.5);
                    if lift > allowed(dark) {
                        failures.push(format!(
                            "{level:?} (dark={dark}, backdrop {level_255}, hover={hover}): pixel {at:?} is {lift} levels lighter than the backdrop"
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "a glass button reads as a grey disc:\n{}", failures.join("\n"));
}

// ---- what lies under a glass knob or card stays visible through it ----

/// Largest minus smallest luminance (0-255) inside `origin`..`origin + size`: how much detail
/// (a label's strokes, an image's edges) the region shows.
fn detail_in(frame: &[u8], origin: Vec2, size: Vec2) -> f32 {
    let (mut lo, mut hi) = (255.0_f32, 0.0_f32);
    let (x0, y0) = (origin.x.max(0.0) as u32, origin.y.max(0.0) as u32);
    let (x1, y1) = (((origin.x + size.x) as u32).min(SIZE.0), ((origin.y + size.y) as u32).min(SIZE.1));
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * SIZE.0 + x) * 4) as usize;
            let l = 0.2126 * frame[i] as f32 + 0.7152 * frame[i + 1] as f32 + 0.0722 * frame[i + 2] as f32;
            lo = lo.min(l);
            hi = hi.max(l);
        }
    }
    hi - lo
}

/// A segmented control (Auto | Hell | Dunkel) with the pill on `selected`. Returns the Ui, the
/// frame of the middle segment's label and the centre of that segment.
fn segmented_control(config: &Config, dark: bool, selected: usize) -> (Ui, Vec2, Vec2, Vec2) {
    let mut ui = Ui::new(Theme::new(config, dark), 1.0);
    ui.set_viewport(Vec2::new(SIZE.0 as f32, SIZE.1 as f32), 1.0);
    let root = ui.root();
    let seg = ui.add(
        root,
        Widget::segmented(selected)
            .with_background(Background::Fill(Paint::Label, 0.07))
            .with_radius(CAPSULE),
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(60.0), top: length(100.0), right: auto(), bottom: auto() },
            size: Size { width: auto(), height: length(36.0) },
            flex_direction: FlexDirection::Row,
            padding: Rect { left: length(3.0), right: length(3.0), top: length(3.0), bottom: length(3.0) },
            gap: Size { width: length(2.0), height: length(0.0) },
            ..Default::default()
        },
    );
    let mut labels = Vec::new();
    let mut cells = Vec::new();
    for (i, text) in ["Auto", "Hell", "Dunkel"].iter().enumerate() {
        let cell = ui.add(
            seg,
            Widget::container().with_action(ActionId(i as u32 + 1)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(30.0) },
                padding: Rect {
                    left: length(30.0),
                    right: length(30.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        labels.push(ui.add_label(cell, text, TextStyle::Body, Style::default()));
        cells.push(cell);
    }
    ui.layout();
    let label = ui.frame(labels[1]);
    let at = ui.frame(cells[1]).center();
    (ui, label.origin, label.size, at)
}

/// The label under a held knob (the pill of a segmented control, as a lens) shows through it:
/// the lens must neither hide it nor blur it away, with Reduce Transparency on or off, light or
/// dark. (Reduce Transparency used to tint the lens opaque: a flat grey capsule over the label.)
#[test]
fn a_held_segmented_pill_keeps_the_label_under_it_visible() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut failures = Vec::new();
    for reduce in [false, true] {
        for dark in [false, true] {
            let mut config = Config::default();
            config.glass.reduce_transparency = reduce;
            // The label of the middle segment with the pill elsewhere: the reference detail.
            let (rest, origin, size, _) = segmented_control(&config, dark, 0);
            let reference = detail_in(&render(&gpu, Some(&rest), 0.5), origin, size);
            // Held on that segment (selected, so the lens lies right over its label).
            let (mut held, _, _, at) = segmented_control(&config, dark, 1);
            held.handle(UiInput::PointerMove(at));
            held.handle(UiInput::PointerDown);
            for _ in 0..90 {
                held.animate(1.0 / 120.0);
            }
            let under_lens = detail_in(&render(&gpu, Some(&held), 0.5), origin, size);
            if reference < 40.0 || under_lens < reference * 0.5 {
                failures.push(format!(
                    "reduce={reduce} dark={dark}: label detail {under_lens:.0} under the lens vs {reference:.0} bare"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "the lens hides its label:\n{}", failures.join("\n"));
}

/// An overview-like card (glass, no action) over a page picture: the picture shows through.
#[test]
fn a_glass_card_over_a_picture_shows_the_picture() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut pixels = Vec::new();
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let on = ((x / 6) + (y / 6)) % 2 == 0;
            let v = if on { 235 } else { 25 };
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let page = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("page"),
        size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        page.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * SIZE.0),
            rows_per_image: Some(SIZE.1),
        },
        wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
    );
    let view = page.create_view(&Default::default());
    let draw = |ui: Option<&Ui>| {
        let empty;
        let host = match ui {
            Some(ui) => ui,
            None => {
                empty = Ui::new(Theme::new(&Config::default(), false), 1.0);
                &empty
            }
        };
        let mut fonts = host.fonts();
        let mut compositor = Compositor::new(&gpu.device, &gpu.queue, FORMAT, &mut fonts);
        let mut layers: Vec<Layer> = vec![Layer::Image(axion_compositor::ImageDraw::new(
            &view,
            axion_compositor::Rect::new(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
        ))];
        if let Some(ui) = ui {
            ui.paint(&mut layers);
        }
        let scene = Scene { clear: [0.5, 0.5, 0.5, 1.0], layers };
        compositor.render(&scene, &mut fonts, &gpu.target, SIZE);
        compositor.read_back().map(|(_, _, p)| p).unwrap_or_default()
    };
    let bare = draw(None);
    let mut failures = Vec::new();
    for dark in [false, true] {
        for level in [GlassLevel::Thin, GlassLevel::Regular] {
            let (ui, origin, size) =
                button_with(&Config::default(), dark, Background::Glass(level), None, false);
            let (inner_o, inner_s) = (origin + Vec2::splat(12.0), size - Vec2::splat(24.0));
            let reference = detail_in(&bare, inner_o, inner_s);
            let seen = detail_in(&draw(Some(&ui)), inner_o, inner_s);
            if seen < reference * 0.15 {
                failures.push(format!(
                    "{level:?} dark={dark}: picture detail {seen:.0} through the card vs {reference:.0}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "the card hides the picture:\n{}", failures.join("\n"));
}
