//! CEF glue for the spike: process-level app, and a client whose render handler turns every
//! off-screen frame into a wgpu texture.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::rc::Rc as _;
use cef::*;

#[derive(Clone)]
pub struct SpikeApp {
    pub no_sandbox: bool,
}

wrap_app! {
    pub struct AppBuilder {
        app: SpikeApp,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(command_line) = command_line else { return };
            command_line.append_switch(Some(&"noerrdialogs".into()));
            command_line.append_switch(Some(&"hide-crash-restore-bubble".into()));
            if self.app.no_sandbox {
                command_line.append_switch(Some(&"no-sandbox".into()));
            }
        }
    }
}

impl AppBuilder {
    pub fn build(app: SpikeApp) -> cef::App {
        Self::new(app)
    }
}

/// Where the latest page frame lands, plus counters for the spike's measurements.
#[derive(Default)]
pub struct FrameSlot {
    pub texture: RefCell<Option<wgpu::Texture>>,
    pub accelerated_frames: Cell<u32>,
    pub software_frames: Cell<u32>,
    pub import_time: Cell<Duration>,
    pub import_failures: Cell<u32>,
    /// Whether any GPU frame has ever been imported; if not, failures mean the import path
    /// doesn't work on this system (e.g. CEF and wgpu on different GPUs).
    pub imported_any: Cell<bool>,
}

/// Logical (DIP) size of the view and its scale factor, shared with the window side.
pub struct ViewGeometry {
    pub width: Cell<f32>,
    pub height: Cell<f32>,
    pub scale: Cell<f32>,
}

#[derive(Clone)]
pub struct SpikeRenderHandler {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub geometry: Rc<ViewGeometry>,
    pub frames: Rc<FrameSlot>,
}

impl SpikeRenderHandler {
    fn store(&self, texture: wgpu::Texture) {
        self.frames.texture.replace(Some(texture));
    }

    /// CPU fallback: upload the BGRA buffer, reusing the texture while the size is unchanged.
    fn upload(&self, pixels: &[u8], width: u32, height: u32) {
        let reusable = self
            .frames
            .texture
            .borrow()
            .as_ref()
            .filter(|t| {
                t.width() == width
                    && t.height() == height
                    && t.usage().contains(wgpu::TextureUsages::COPY_DST)
            })
            .cloned();
        let texture = reusable.unwrap_or_else(|| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("cef software frame"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.store(texture);
    }
}

wrap_render_handler! {
    pub struct RenderHandlerBuilder {
        handler: SpikeRenderHandler,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else { return };
            let g = &self.handler.geometry;
            rect.width = (g.width.get() as i32).max(1);
            rect.height = (g.height.get() as i32).max(1);
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            let Some(screen_info) = screen_info else { return 0 };
            screen_info.device_scale_factor = self.handler.geometry.scale.get();
            1
        }

        fn on_accelerated_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            use cef::osr_texture_import::SharedTextureHandle;

            let Some(info) = info else { return };
            if type_ != PaintElementType::default() {
                return;
            }
            let start = Instant::now();
            let handle = SharedTextureHandle::new(info);
            // SAFETY: we are inside CEF's `on_accelerated_paint` for this frame, and the wgpu
            // device was created with VULKAN_EXTERNAL_MEMORY_DMA_BUF. If CEF's GPU process
            // runs on a different GPU the import fails and is counted, not undefined.
            match unsafe { handle.import_texture(&self.handler.device, &self.handler.queue) } {
                Ok(texture) => {
                    let frames = &self.handler.frames;
                    frames.accelerated_frames.set(frames.accelerated_frames.get() + 1);
                    frames.import_time.set(frames.import_time.get() + start.elapsed());
                    frames.imported_any.set(true);
                    self.handler.store(texture);
                }
                Err(error) => {
                    let frames = &self.handler.frames;
                    if frames.import_failures.get() == 0 {
                        tracing::error!(%error, "importing CEF shared texture failed");
                    }
                    frames.import_failures.set(frames.import_failures.get() + 1);
                }
            }
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if type_ != PaintElementType::default() || buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let len = width as usize * height as usize * 4;
            // SAFETY: CEF guarantees `buffer` holds width * height BGRA pixels for the
            // duration of this callback; we copy out of it before returning.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            self.handler.upload(pixels, width as u32, height as u32);
            let frames = &self.handler.frames;
            frames.software_frames.set(frames.software_frames.get() + 1);
        }
    }
}

wrap_client! {
    pub struct ClientBuilder {
        render_handler: RenderHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render_handler.clone())
        }
    }
}

impl ClientBuilder {
    pub fn build(handler: SpikeRenderHandler) -> Client {
        Self::new(RenderHandlerBuilder::new(handler))
    }
}
