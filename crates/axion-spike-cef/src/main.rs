//! Phase 0 spike: a real web page rendered off-screen by CEF, imported into wgpu every frame
//! and composited under Liquid Glass.
//!
//! ```bash
//! cargo run -p axion-spike-cef --release -- [url]
//! ```
//!
//! Input goes to the page. F1 toggles a glass lens that follows the cursor, Alt+←/→
//! navigate, F5 reloads. Stats (CEF frame rate, import cost, compositor fps) are logged every
//! two seconds.
//!
//! `--capture=out.png` saves the composited frame after the page has had time to load, then exits.
//!
//! Environment: `AXION_GPU=low|high` picks the wgpu adapter, `AXION_OSR=software` forces
//! CEF's CPU paint path, `AXION_NO_SANDBOX=1` disables Chromium's sandbox.

mod cef_app;
mod input;

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axion_glass::{GlassFrame, GlassParams, GlassRenderer, GlassShape};
use cef::*;
use glam::Vec2;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use cef_app::{AppBuilder, ClientBuilder, FrameSlot, SpikeApp, SpikeRenderHandler, ViewGeometry};
use input::InputForwarder;

const DEFAULT_URL: &str = "https://en.wikipedia.org/wiki/Glass";
const STATS_INTERVAL: Duration = Duration::from_secs(2);
const LENS_RADIUS: f32 = 56.0;
/// Fraction of the remaining distance the lens covers per frame at 60 fps.
const LENS_FOLLOW: f32 = 0.25;
/// How long `--capture` waits for the page to load and settle.
const CAPTURE_AFTER: Duration = Duration::from_secs(7);

fn main() -> Result<()> {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = args::Args::new();
    let cmd = args.as_cmd_line().context("reading command line")?;
    let is_browser_process = cmd.has_switch(Some(&"type".into())) != 1;
    let no_sandbox = std::env::var_os("AXION_NO_SANDBOX").is_some();
    let mut app = AppBuilder::build(SpikeApp { no_sandbox });

    // Chromium re-launches this binary for its renderer/GPU/utility processes.
    let code = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    if !is_browser_process {
        std::process::exit(code);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn".into()),
        )
        .init();

    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .context("no cache directory")?
        .join("axion-spike");
    let settings = Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        no_sandbox: no_sandbox as _,
        root_cache_path: cache.to_string_lossy().as_ref().into(),
        ..Default::default()
    };
    if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
        bail!("CEF failed to initialize");
    }

    let url = std::env::args().skip(1).find(|a| !a.starts_with("--")).unwrap_or_else(|| DEFAULT_URL.into());
    let capture = std::env::args().find_map(|a| a.strip_prefix("--capture=").map(std::path::PathBuf::from));
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut spike = Spike { url, capture, state: None, error: None };
    event_loop.run_app(&mut spike)?;

    if let Some(state) = spike.state.take() {
        if let Some(host) = state.browser.host() {
            host.close_browser(1);
        }
        drop(state);
    }
    shutdown();
    spike.error.map_or(Ok(()), Err)
}

struct Spike {
    url: String,
    capture: Option<std::path::PathBuf>,
    state: Option<State>,
    error: Option<anyhow::Error>,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    glass: GlassRenderer,
    params: GlassParams,
    browser: Browser,
    handler: SpikeRenderHandler,
    url: String,
    /// Whether CEF is delivering GPU (dma-buf) frames rather than CPU bitmaps.
    accelerated: bool,
    geometry: Rc<ViewGeometry>,
    frames: Rc<FrameSlot>,
    input: InputForwarder,
    lens: Option<Vec2>,
    pointer: Vec2,
    started: Instant,
    last_frame: Instant,
    stats_since: Instant,
    rendered: u32,
}

impl ApplicationHandler for Spike {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop, &self.url) {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = state.window.inner_size();
                state.resize(size.width, size.height);
                if let Some(host) = state.browser.host() {
                    host.notify_screen_info_changed();
                }
            }
            WindowEvent::RedrawRequested => {
                let result = match &self.capture {
                    Some(path) if state.started.elapsed() > CAPTURE_AFTER => {
                        let result = state.capture(path);
                        event_loop.exit();
                        result
                    }
                    _ => state.frame(),
                };
                if let Err(e) = result {
                    self.error = Some(e);
                    event_loop.exit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.pointer = Vec2::new(position.x as f32, position.y as f32);
                state.input.handle(&state.browser, &event, state.geometry.scale.get());
            }
            WindowEvent::KeyboardInput { event: ref key, .. }
                if key.state.is_pressed() && key.logical_key == winit::keyboard::NamedKey::F1 =>
            {
                state.lens = match state.lens {
                    Some(_) => None,
                    None => Some(state.pointer),
                };
            }
            other => state.input.handle(&state.browser, &other, state.geometry.scale.get()),
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        do_message_loop_work();
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

/// Creates an off-screen browser whose frames go to `handler`. With `accelerated`, CEF shares
/// GPU textures (dma-buf); otherwise it paints into CPU bitmaps.
fn create_browser(handler: &SpikeRenderHandler, url: &str, accelerated: bool) -> Result<Browser> {
    let mut client = ClientBuilder::build(handler.clone());
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: accelerated as _,
        // We drive CEF's frames from our own render loop, in step with presentation.
        external_begin_frame_enabled: 1,
        ..Default::default()
    };
    let browser_settings = BrowserSettings { windowless_frame_rate: 60, ..Default::default() };
    let browser = browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut client),
        Some(&url.into()),
        Some(&browser_settings),
        None,
        None,
    )
    .context("CEF could not create the browser")?;
    if let Some(host) = browser.host() {
        host.set_focus(1);
    }
    Ok(browser)
}

impl State {
    fn new(event_loop: &ActiveEventLoop, url: &str) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Axion — CEF spike")
                    .with_inner_size(winit::dpi::LogicalSize::new(1440.0, 900.0)),
            )?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
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
        let dmabuf = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
        let accelerated =
            adapter.features().contains(dmabuf) && std::env::var("AXION_OSR").as_deref() != Ok("software");
        if !accelerated {
            tracing::warn!("using CEF software paint (dma-buf import unavailable or disabled)");
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: if accelerated { dmabuf } else { wgpu::Features::empty() },
            ..Default::default()
        }))?;

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // CEF frames are 8-bit BGRA with sRGB-encoded values; compositing in that space keeps
        // the page's colours exact. Prefer a non-sRGB swapchain so they aren't re-encoded.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let scale = window.scale_factor() as f32;
        let geometry = Rc::new(ViewGeometry {
            width: (size.width as f32 / scale).into(),
            height: (size.height as f32 / scale).into(),
            scale: scale.into(),
        });
        let frames = Rc::new(FrameSlot::default());
        let handler = SpikeRenderHandler {
            device: device.clone(),
            queue: queue.clone(),
            geometry: geometry.clone(),
            frames: frames.clone(),
        };
        let browser = create_browser(&handler, url, accelerated)?;

        let now = Instant::now();
        let lens_start = Vec2::new(size.width as f32 * 0.7, size.height as f32 * 0.45);
        Ok(Self {
            window,
            surface,
            glass: GlassRenderer::new(&device, format),
            device,
            queue,
            config,
            params: GlassParams::default(),
            browser,
            handler,
            url: url.to_owned(),
            accelerated,
            geometry,
            frames,
            input: InputForwarder::default(),
            lens: Some(lens_start),
            pointer: lens_start,
            started: now,
            last_frame: now,
            stats_since: now,
            rendered: 0,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        let scale = self.window.scale_factor() as f32;
        self.geometry.scale.set(scale);
        self.geometry.width.set(width as f32 / scale);
        self.geometry.height.set(height as f32 / scale);
        if let Some(host) = self.browser.host() {
            host.was_resized();
        }
    }

    /// Browser chrome stand-ins: tab bar capsule with two buttons, a media capsule, the lens.
    fn shapes(&self) -> Vec<GlassShape> {
        let s = self.geometry.scale.get();
        let w = self.config.width as f32;
        let h = self.config.height as f32;
        let top = (16.0 + 24.0) * s;
        let bar_half = (w * 0.3).min(320.0 * s);
        let button = 24.0 * s;
        let gap = 14.0 * s;
        let mut shapes = vec![
            GlassShape::capsule(Vec2::new(w / 2.0, top), Vec2::new(bar_half, 24.0 * s)),
            GlassShape::circle(Vec2::new(w / 2.0 - bar_half - gap - button, top), button),
            GlassShape::circle(Vec2::new(w / 2.0 + bar_half + gap + button, top), button),
            GlassShape::capsule(Vec2::new(w / 2.0, h - 52.0 * s), Vec2::new(200.0 * s, 28.0 * s)),
        ];
        if let Some(lens) = self.lens {
            shapes.push(GlassShape::circle(lens, LENS_RADIUS * s));
        }
        shapes
    }

    fn frame(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if let Some(lens) = &mut self.lens {
            let k = 1.0 - (1.0 - LENS_FOLLOW).powf(dt * 60.0);
            *lens += (self.pointer - *lens) * k;
        }

        if self.accelerated && self.frames.import_failures.get() > 0 && !self.frames.imported_any.get() {
            tracing::warn!("GPU frame import does not work here; restarting the page with software paint");
            if let Some(host) = self.browser.host() {
                host.close_browser(1);
            }
            self.accelerated = false;
            self.frames.import_failures.set(0);
            self.browser = create_browser(&self.handler, &self.url, false)?;
        }

        // Ask CEF for the next frame; it arrives via the render handler during message work.
        if let Some(host) = self.browser.host() {
            host.send_external_begin_frame();
        }

        let Some(page) = self.frames.texture.borrow().clone() else {
            return Ok(());
        };
        let surface = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Validation => bail!("surface validation error"),
        };
        let target = surface.texture.create_view(&Default::default());
        let encoder = self.composite(&page, &target, None);
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(surface);
        self.rendered += 1;
        self.log_stats(now, &page);
        Ok(())
    }

    /// Records the glass composite of `page` into `target`. `renderer` overrides the
    /// swapchain-format renderer (for offscreen targets of another format).
    fn composite(
        &mut self,
        page: &wgpu::Texture,
        target: &wgpu::TextureView,
        renderer: Option<&mut GlassRenderer>,
    ) -> wgpu::CommandEncoder {
        let backdrop = page.create_view(&Default::default());
        let shapes = self.shapes();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        renderer.unwrap_or(&mut self.glass).render(
            &self.device,
            &self.queue,
            &mut encoder,
            GlassFrame {
                backdrop: &backdrop,
                backdrop_size: (self.config.width, self.config.height),
                target,
                shapes: &shapes,
                params: &self.params,
            },
        );
        encoder
    }

    /// Composites the current page frame offscreen and saves it as PNG.
    fn capture(&mut self, path: &std::path::Path) -> Result<()> {
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        let page = self.frames.texture.borrow().clone().context("no page frame arrived from CEF")?;
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
        let mut renderer = GlassRenderer::new(&self.device, FORMAT);
        let mut encoder =
            self.composite(&page, &texture.create_view(&Default::default()), Some(&mut renderer));
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
            .with_context(|| format!("saving {}", path.display()))?;
        tracing::info!(path = %path.display(), "captured frame");
        Ok(())
    }

    fn log_stats(&mut self, now: Instant, page: &wgpu::Texture) {
        let elapsed = now.duration_since(self.stats_since);
        if elapsed < STATS_INTERVAL {
            return;
        }
        let secs = elapsed.as_secs_f32();
        let f = &self.frames;
        let accelerated = f.accelerated_frames.take();
        let software = f.software_frames.take();
        let import_ms = if accelerated > 0 {
            f.import_time.take().as_secs_f32() * 1000.0 / accelerated as f32
        } else {
            0.0
        };
        tracing::info!(
            compositor_fps = format!("{:.0}", self.rendered as f32 / secs),
            cef_gpu_fps = format!("{:.0}", accelerated as f32 / secs),
            cef_cpu_fps = format!("{:.0}", software as f32 / secs),
            import_ms = format!("{import_ms:.2}"),
            import_failures = f.import_failures.get(),
            page = format!("{}x{} {:?}", page.width(), page.height(), page.format()),
            "stats"
        );
        self.rendered = 0;
        self.stats_since = now;
    }
}
