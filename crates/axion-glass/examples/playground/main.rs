//! Liquid Glass playground: glass UI over a scrolling fake web page, no CEF needed.
//!
//! ```bash
//! cargo run -p axion-glass --example playground --release [-- path/to/screenshot.png]
//! ```
//!
//! Drag anything. Scroll the page with the wheel/touchpad. Tune the material with the keys
//! below; current values are shown in the window title.
//!
//! | Keys  | Parameter            |
//! |-------|----------------------|
//! | Q / A | refraction           |
//! | W / S | bezel width          |
//! | E / D | dispersion           |
//! | R / F | frost                |
//! | T / G | merge smoothness     |
//! | U / J | blur radius          |
//! | I / K | rim highlight        |
//! | L     | light / dark page    |
//! | Space | pause page animation |
//! | 0     | reset parameters     |
//!
//! Set `AXION_GPU=low` to run on the integrated GPU, `AXION_VSYNC=0` to measure uncapped fps. `--demo` drags the big droplet along a
//! scripted path (through the tab bar) so merging and stretching can be checked hands-free.
//! `--bench` renders 1440p frames offscreen, prints the GPU time per frame and exits.
//! `--capture=out.png` renders 3.3 s of the demo drag offscreen and saves the last frame.

mod backdrop;
mod scene;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axion_glass::{GlassFrame, GlassParams, GlassRenderer};
use axion_physics::{Animated, Spring};
use glam::Vec2;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use backdrop::{Backdrop, PageState};
use scene::Scene;

/// Pixels scrolled per wheel "line".
const LINE_SCROLL: f32 = 80.0;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn".into()),
        )
        .init();

    let demo = std::env::args().any(|a| a == "--demo");
    let bench = std::env::args().any(|a| a == "--bench");
    let capture = std::env::args().find_map(|a| a.strip_prefix("--capture=").map(str::to_owned));
    let image = match std::env::args().skip(1).find(|a| !a.starts_with("--")) {
        Some(path) => Some(image::open(&path).with_context(|| format!("loading {path}"))?.to_rgba8()),
        None => None,
    };

    let event_loop = EventLoop::new()?;
    let mut app = App { image, demo, bench, capture, gpu: None, error: None };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct App {
    image: Option<image::RgbaImage>,
    demo: bool,
    bench: bool,
    capture: Option<String>,
    gpu: Option<Gpu>,
    error: Option<anyhow::Error>,
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    backdrop: Backdrop,
    glass: GlassRenderer,
    scene: Scene,
    params: GlassParams,
    page: PageState,
    scroll: Animated<f32>,
    animate_page: bool,
    adapter_name: String,
    demo: Option<Demo>,
    last_frame: Instant,
    fps_window: (Instant, u32),
    fps: f32,
}

struct Demo {
    start: Option<Instant>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, self.image.take()) {
            Ok(mut gpu) => {
                gpu.demo = self.demo.then_some(Demo { start: None });
                if let Some(path) = &self.capture {
                    if let Err(e) = gpu.capture(path) {
                        self.error = Some(e);
                    }
                    event_loop.exit();
                } else if self.bench {
                    if let Err(e) = gpu.bench() {
                        self.error = Some(e);
                    }
                    event_loop.exit();
                }
                self.gpu = Some(gpu);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gpu) = &mut self.gpu else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => gpu.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = gpu.window.inner_size();
                gpu.resize(size.width, size.height);
            }
            WindowEvent::CursorMoved { position, .. } => {
                gpu.scene.pointer_moved(Vec2::new(position.x as f32, position.y as f32));
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => gpu.scene.pointer_down(),
                ElementState::Released => gpu.scene.pointer_up(),
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * LINE_SCROLL * gpu.window.scale_factor() as f32,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                gpu.scroll.target = (gpu.scroll.target - dy).max(0.0);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                gpu.key(&event.logical_key);
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = gpu.frame() {
                    self.error = Some(e);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

impl Gpu {
    fn new(event_loop: &ActiveEventLoop, image: Option<image::RgbaImage>) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Axion — Glass Playground")
                    .with_inner_size(winit::dpi::LogicalSize::new(1440.0, 900.0)),
            )?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        ));
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
        let adapter_name = adapter.get_info().name;
        tracing::info!(adapter = %adapter_name, backend = ?adapter.get_info().backend, "using GPU");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: match std::env::var("AXION_VSYNC").as_deref() {
                Ok("0") => wgpu::PresentMode::AutoNoVsync,
                _ => wgpu::PresentMode::AutoVsync,
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let backdrop = Backdrop::new(&device, &queue, (config.width, config.height), image);
        let glass = GlassRenderer::new(&device, format);
        let mut scene = Scene::new();
        scene.layout(Vec2::new(config.width as f32, config.height as f32), window.scale_factor() as f32);

        let now = Instant::now();
        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            backdrop,
            glass,
            scene,
            params: GlassParams::default(),
            page: PageState { scroll: 0.0, time: 0.0, dark: false },
            scroll: Animated::new(0.0, Spring::new(0.35, 1.0)),
            animate_page: true,
            adapter_name,
            demo: None,
            last_frame: now,
            fps_window: (now, 0),
            fps: 0.0,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.backdrop.resize(&self.device, (width, height));
        self.scene.layout(Vec2::new(width as f32, height as f32), self.window.scale_factor() as f32);
    }

    fn key(&mut self, key: &Key) {
        let p = &mut self.params;
        match key {
            Key::Named(NamedKey::Space) => self.animate_page = !self.animate_page,
            Key::Character(c) => match c.to_lowercase().as_str() {
                "q" => p.refraction += 2.0,
                "a" => p.refraction -= 2.0,
                "w" => p.bezel_width += 2.0,
                "s" => p.bezel_width = (p.bezel_width - 2.0).max(2.0),
                "e" => p.dispersion += 0.05,
                "d" => p.dispersion = (p.dispersion - 0.05).max(0.0),
                "r" => p.frost = (p.frost + 0.1).min(1.0),
                "f" => p.frost = (p.frost - 0.1).max(0.0),
                "t" => p.merge_smoothness += 4.0,
                "g" => p.merge_smoothness = (p.merge_smoothness - 4.0).max(0.0),
                "u" => p.blur_radius += 2.0,
                "j" => p.blur_radius = (p.blur_radius - 2.0).max(0.0),
                "i" => p.rim += 0.05,
                "k" => p.rim = (p.rim - 0.05).max(0.0),
                "l" => {
                    self.page.dark = !self.page.dark;
                    let fresh = if self.page.dark { GlassParams::dark() } else { GlassParams::default() };
                    p.tint = fresh.tint;
                }
                "0" => *p = if self.page.dark { GlassParams::dark() } else { GlassParams::default() },
                _ => {}
            },
            _ => {}
        }
    }

    fn frame(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.scroll.step(dt);
        self.page.scroll = self.scroll.value.max(0.0);
        if self.animate_page {
            self.page.time += dt;
        }
        self.drive_demo(now);
        self.scene.update(dt);
        self.update_title(now);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Validation => anyhow::bail!("surface validation error"),
        };
        let target = frame.texture.create_view(&Default::default());

        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.backdrop.render(&self.queue, &mut encoder, &self.page);
        let shapes = self.scene.shapes();
        self.glass.render(
            &self.device,
            &self.queue,
            &mut encoder,
            GlassFrame {
                backdrop: &self.backdrop.view,
                backdrop_size: (self.config.width, self.config.height),
                target: &target,
                shapes: &shapes,
                params: &self.params,
            },
        );
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
        Ok(())
    }

    /// Runs the demo offscreen for a moment and writes the final frame to `path`.
    fn capture(&mut self, path: &str) -> Result<()> {
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
        const SECONDS: f32 = 3.3;
        let (width, height) = (self.config.width, self.config.height);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target = texture.create_view(&Default::default());
        let mut glass = GlassRenderer::new(&self.device, FORMAT);
        self.demo = Some(Demo { start: None });
        let start = Instant::now();
        let dt = 1.0 / 120.0;
        let frames = (SECONDS / dt) as u32;
        for i in 0..frames {
            self.page.time += dt;
            self.drive_demo(start + std::time::Duration::from_secs_f32(i as f32 * dt));
            self.scene.update(dt);
        }

        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.backdrop.render(&self.queue, &mut encoder, &self.page);
        let shapes = self.scene.shapes();
        glass.render(
            &self.device,
            &self.queue,
            &mut encoder,
            GlassFrame {
                backdrop: &self.backdrop.view,
                backdrop_size: (width, height),
                target: &target,
                shapes: &shapes,
                params: &self.params,
            },
        );
        let row = (4 * width).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely())?;

        let data = buffer.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity((4 * width * height) as usize);
        for y in 0..height as usize {
            let start = y * row as usize;
            pixels.extend_from_slice(&data[start..start + 4 * width as usize]);
        }
        image::RgbaImage::from_raw(width, height, pixels)
            .context("capture buffer has the wrong size")?
            .save(path)
            .with_context(|| format!("saving {path}"))?;
        tracing::info!(path, "captured frame");
        Ok(())
    }

    /// Renders frames offscreen at 1440p with the demo drag running and reports GPU time.
    fn bench(&mut self) -> Result<()> {
        const WARMUP: u32 = 30;
        const FRAMES: u32 = 300;
        let size = (2560, 1440);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target = texture.create_view(&Default::default());
        self.backdrop.resize(&self.device, size);
        self.scene.layout(Vec2::new(size.0 as f32, size.1 as f32), 1.5);
        self.demo = Some(Demo { start: None });

        let mut start = Instant::now();
        for i in 0..WARMUP + FRAMES {
            if i == WARMUP {
                self.device.poll(wgpu::PollType::wait_indefinitely())?;
                start = Instant::now();
            }
            let dt = 1.0 / 120.0;
            self.page.time += dt;
            self.drive_demo(start + std::time::Duration::from_secs_f32(i as f32 * dt));
            self.scene.update(dt);
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.backdrop.render(&self.queue, &mut encoder, &self.page);
            let shapes = if std::env::var_os("AXION_BENCH_NO_SHAPES").is_some() {
                vec![]
            } else {
                self.scene.shapes()
            };
            self.glass.render(
                &self.device,
                &self.queue,
                &mut encoder,
                GlassFrame {
                    backdrop: &self.backdrop.view,
                    backdrop_size: size,
                    target: &target,
                    shapes: &shapes,
                    params: &self.params,
                },
            );
            self.queue.submit([encoder.finish()]);
        }
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        let ms = start.elapsed().as_secs_f32() * 1000.0 / FRAMES as f32;
        tracing::info!(
            adapter = %self.adapter_name,
            "bench 2560x1440: {ms:.2} ms/frame (backdrop + blur + glass) ≈ {:.0} fps",
            1000.0 / ms
        );
        Ok(())
    }

    /// Grabs the big droplet and swings it up through the tab bar and back, forever.
    fn drive_demo(&mut self, now: Instant) {
        let Some(demo) = &mut self.demo else { return };
        let Some(start) = demo.start else {
            demo.start = Some(now);
            self.scene.pointer_moved(self.scene.droplet_center());
            self.scene.pointer_down();
            return;
        };
        let t = now.duration_since(start).as_secs_f32();
        let home = Vec2::new(self.config.width as f32, self.config.height as f32) * Vec2::new(0.62, 0.55);
        let bar = self.scene.bar_center();
        // 0 → home, 1 → just below the bar's right end, where it touches the bar.
        let k = 0.5 - 0.5 * (t * 0.9).cos();
        let target = bar + Vec2::new(150.0, 40.0) * self.window.scale_factor() as f32;
        let p = home.lerp(target, k) + Vec2::new((t * 2.1).sin() * 60.0, 0.0);
        self.scene.pointer_moved(p);
    }

    fn update_title(&mut self, now: Instant) {
        self.fps_window.1 += 1;
        let elapsed = now.duration_since(self.fps_window.0).as_secs_f32();
        if elapsed < 0.5 {
            return;
        }
        self.fps = self.fps_window.1 as f32 / elapsed;
        self.fps_window = (now, 0);
        tracing::debug!(fps = self.fps, size = ?(self.config.width, self.config.height), "frame rate");
        let p = &self.params;
        self.window.set_title(&format!(
            "Axion Glass — {:.0} fps on {} | refr {:.0} bezel {:.0} disp {:.2} frost {:.1} merge {:.0} blur {:.0} rim {:.2}",
            self.fps,
            self.adapter_name,
            p.refraction,
            p.bezel_width,
            p.dispersion,
            p.frost,
            p.merge_smoothness,
            p.blur_radius,
            p.rim
        ));
    }
}
