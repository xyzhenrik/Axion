//! Per-view CEF handlers. Every callback runs on the main thread during
//! `do_message_loop_work`, so plain `Rc`/`RefCell` state is enough.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use cef::rc::Rc as _;
use cef::sys::{cef_cursor_type_t, cef_window_open_disposition_t};
use cef::*;
use glam::Vec2;

use super::dmabuf::{self, ImportCache};
use super::network::{Network, build_request_handler};
use super::settle::Settle;
use crate::{
    CursorKind, EngineEvent, INTERNAL_SCHEME, IconLink, NewViewKind, PAGE_MESSAGE_PREFIX, PageMeta,
    ThemeColor, ViewId,
};

/// Console messages of the page meta script ([`PAGE_META_SCRIPT`]) start with this.
const PAGE_META_PREFIX: &str = "\u{1}axion-meta:";
/// Reports icon links and theme colours of every top-level document.
const PAGE_META_SCRIPT: &str = include_str!("page_meta.js");

/// Events from all views, tagged with the CEF browser they came from so events from a browser
/// that was replaced (see the software fallback) can be dropped.
#[derive(Default)]
pub(super) struct Shared {
    pub events: RefCell<Vec<(i32, EngineEvent)>>,
    /// Run in every document as it starts loading (see `Engine::set_page_script`).
    pub page_script: RefCell<Option<String>>,
    pub downloads: Rc<super::download::Downloads>,
    pub stats: super::stats::Stats,
    /// How long after Chromium hands over a GPU frame it is copied (`ViewContext::submit_gpu_frame`).
    pub frame_settle: Duration,
    /// DevTools browsers Chromium created for a view (see `Engine::open_devtools`), until the
    /// engine takes them in `pump`.
    pub devtools_created: RefCell<Vec<(ViewId, Browser)>>,
}

#[derive(Default)]
pub(super) struct ViewFrames {
    pub texture: RefCell<Option<wgpu::Texture>>,
    pub serial: Cell<u64>,
    pub imported_any: Cell<bool>,
    pub import_failures: Cell<u32>,
    /// Chromium's shared buffers, imported once each.
    imports: RefCell<ImportCache>,
    /// The newest GPU frame, waiting for its copy to be due (see `ViewContext::submit_gpu_frame`).
    scheduled: RefCell<Settle<(wgpu::Texture, Region)>>,
    /// GPU frames whose copy is submitted but not yet known to be drawn, oldest first.
    pending: RefCell<VecDeque<PendingFrame>>,
    /// Frame textures and read-back buffers to use again.
    spare_textures: RefCell<Vec<wgpu::Texture>>,
    spare_checks: RefCell<Vec<wgpu::Buffer>>,
    /// A frame never got drawn: Chromium should paint again (taken by the engine).
    pub wants_repaint: Cell<bool>,
    /// Frames with a black bar skipped in a row (see `ViewContext::upload`, `resolve_frames`).
    pub skipped_bars: Cell<u32>,
    /// The last frame shown had black edges of its own (a dark page, such as the dark start
    /// page): the letterbox check can't tell a bar from the page then and is left out.
    /// Without this every frame of such a page was skipped for as long as the window was being
    /// resized: the page froze.
    pub edges_black: Cell<bool>,
    /// Until when frames are checked for the letterboxed page of a resize (see
    /// `is_letterboxed`); `None` once a frame without bars came.
    pub letterbox_until: Cell<Option<Instant>>,
}

impl ViewFrames {
    /// Lets go of everything that refers to Chromium's buffers (the view's page closed).
    pub fn release_shared(&self) {
        self.imports.borrow_mut().clear();
        self.scheduled.borrow_mut().clear();
        self.pending.borrow_mut().clear();
        self.spare_textures.borrow_mut().clear();
        self.spare_checks.borrow_mut().clear();
    }

    /// When the frame waiting to be copied is due, if there is one.
    pub fn copy_due(&self) -> Option<Instant> {
        self.scheduled.borrow().due()
    }

    /// Bytes of the frame textures this view holds (its frame, spares and copies on their
    /// way), and how many of Chromium's buffers are imported (diagnostics).
    pub fn memory(&self) -> (u64, usize) {
        let bytes = |t: &wgpu::Texture| u64::from(t.width()) * u64::from(t.height()) * 4;
        let mut total = self.texture.borrow().as_ref().map_or(0, bytes);
        total += self.spare_textures.borrow().iter().map(bytes).sum::<u64>();
        total += self.pending.borrow().iter().map(|f| bytes(&f.texture)).sum::<u64>();
        (total, self.imports.borrow().len())
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.borrow().is_empty()
    }

    /// The view was resized: watch for frames of the old page scaled into the new size.
    pub fn resized(&self) {
        self.letterbox_until.set(Some(Instant::now() + LETTERBOX_WATCH));
    }
}

/// The part of a shared buffer that holds the page, and its size.
type Region = (wgpu::Origin3d, wgpu::Extent3d);

/// A shared GPU frame on its way: copied into `texture`, with a few rows read back into `check`
/// to see whether Chromium had finished drawing it.
struct PendingFrame {
    /// The imported shared buffer, and the part of it that holds the page.
    source: wgpu::Texture,
    region: (wgpu::Origin3d, wgpu::Extent3d),
    texture: wgpu::Texture,
    check: wgpu::Buffer,
    /// `MAP_*` below, set by the read-back.
    mapped: Arc<AtomicU8>,
    /// When Chromium handed the frame over.
    since: Instant,
}

/// Most software frames with a black bar skipped in a row after a resize.
const MAX_SKIPPED_BARS: u32 = 3;

/// Whether a BGRA frame has a black line along an edge but isn't black through the middle: the
/// page of another size scaled into the buffer with a bar (see `ViewContext::upload`).
fn has_black_bar(pixels: &[u8], width: usize, height: usize) -> bool {
    if width < 8 || height < 8 || pixels.len() < width * height * 4 {
        return false;
    }
    let black = |x: usize, y: usize| {
        let p = &pixels[(y * width + x) * 4..][..3];
        p == [0, 0, 0]
    };
    let row = |y: usize| (0..width).step_by(4).all(|x| black(x, y));
    let column = |x: usize| (0..height).step_by(4).all(|y| black(x, y));
    let edge = row(0) || row(height - 1) || column(0) || column(width - 1);
    edge && !(0..width).step_by(4).all(|x| black(x, height / 2))
}

const MAP_WAITING: u8 = 0;
const MAP_DONE: u8 = 1;
const MAP_FAILED: u8 = 2;

/// For how long a shared GPU frame that isn't drawn yet is copied again (see
/// `ViewContext::resolve_frames`) before it is given up (the previous frame stays and Chromium
/// is asked to paint again).
const FRAME_READY_TIMEOUT: Duration = Duration::from_millis(25);
/// How long after a resize frames are checked for the page of the old size scaled into one
/// of the new size (see `is_letterboxed`).
const LETTERBOX_WATCH: Duration = Duration::from_millis(250);
/// Usage of the textures frames are copied into (read back for the drawn check and snapshots).
const FRAME_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC);

pub(super) struct ViewGeometry {
    pub logical: Cell<Vec2>,
    pub scale: Cell<f32>,
}

#[derive(Clone)]
pub(super) struct ViewContext {
    pub view: ViewId,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub geometry: Rc<ViewGeometry>,
    pub frames: Rc<ViewFrames>,
    pub shared: Rc<Shared>,
    pub net: Arc<Network>,
    pub drag: Rc<super::drag::ViewDrag>,
    /// The view is Chromium's DevTools window of another view: it gets none of the scripts
    /// meant for web pages and reports no events.
    pub devtools: bool,
    /// The panel the DevTools window should show once it has loaded (`devtools` views).
    pub devtools_panel: Rc<RefCell<Option<&'static str>>>,
}

impl ViewContext {
    fn emit(&self, browser: Option<&mut Browser>, event: EngineEvent) {
        let id = browser.map_or(-1, |b| b.identifier());
        self.shared.events.borrow_mut().push((id, event));
    }

    fn store(&self, texture: wgpu::Texture) {
        if let Some(old) = self.frames.texture.replace(Some(texture)) {
            self.recycle_texture(old);
        }
        self.frames.serial.set(self.frames.serial.get() + 1);
        self.shared.stats.count(|c| c.page_frames += 1);
    }

    fn recycle_texture(&self, texture: wgpu::Texture) {
        let mut spare = self.frames.spare_textures.borrow_mut();
        // Two cover a frame on its way while another is checked; more would only hold memory.
        if spare.len() < 2 && texture.usage() == FRAME_USAGE {
            spare.push(texture);
        }
    }

    /// Takes a frame Chromium shares as a dma-buf: submits copying it into a texture of our own,
    /// without waiting for the GPU (waiting held up Chromium, whose UI thread is ours, and halved
    /// the page's frame rate). The copy becomes the view's frame once it is known to be drawn
    /// (`resolve_frames`).
    ///
    /// Chromium calls `on_accelerated_paint` once the frame's drawing is *submitted* on its GPU,
    /// not once it has finished, and NVIDIA's driver attaches no fence to the dma-buf that we
    /// could wait for. Copied at once, a buffer can still be empty (the first use of a pooled
    /// buffer, e.g. after every resize) or partly drawn, and the page flashed blank. So a few
    /// rows of the copy are read back, and a frame that isn't drawn is copied again from the
    /// (still imported) buffer shortly after. Chromium draws pages over an opaque background,
    /// so a drawn frame has no fully transparent pixels.
    fn submit_gpu_frame(&self, info: &AcceleratedPaintInfo) {
        let frames = &self.frames;
        // SAFETY: we are inside CEF's `on_accelerated_paint` for this frame (the caller), and
        // the wgpu device was created with VULKAN_EXTERNAL_MEMORY_DMA_BUF. If CEF renders on
        // another GPU the import fails cleanly and the engine falls back to software paint.
        let imported = unsafe { frames.imports.borrow_mut().texture(&self.device, info) };
        let source = match imported {
            Ok(texture) => texture,
            Err(error) => {
                if frames.import_failures.get() == 0 {
                    tracing::warn!(view = ?self.view, %error, "importing a GPU frame from Chromium failed");
                }
                frames.import_failures.set(frames.import_failures.get() + 1);
                return;
            }
        };
        frames.imported_any.set(true);
        let region = content_region(&source, &info.extra.content_rect);
        let settle = self.shared.frame_settle;
        if !settle.is_zero() {
            // A newer frame replaces one that hasn't been copied yet: it shows the page later.
            let now = Instant::now();
            if frames.scheduled.borrow().due().is_some() {
                self.shared.stats.count(|c| c.settle_replaced += 1);
            }
            if let Some((source, region)) = frames.scheduled.borrow_mut().offer((source, region), now, settle)
            {
                self.copy_gpu_frame(source, region);
            }
            return;
        }
        self.copy_gpu_frame(source, region);
    }

    /// Starts copying the scheduled frame once it is due.
    fn start_scheduled(&self) {
        let due = self.frames.scheduled.borrow_mut().take_due(Instant::now());
        if let Some((source, region)) = due {
            self.copy_gpu_frame(source, region);
        }
    }

    fn copy_gpu_frame(&self, source: wgpu::Texture, region: Region) {
        let frames = &self.frames;
        let texture = self.frame_texture(region.1, source.format());
        let check = self.check_buffer(region.1.width);
        let frame = PendingFrame {
            source,
            region,
            texture,
            check,
            mapped: Arc::new(AtomicU8::new(MAP_WAITING)),
            since: Instant::now(),
        };
        if let Some(frame) = self.submit_copy(frame) {
            frames.pending.borrow_mut().push_back(frame);
        }
    }

    /// Submits copying the frame and reading back its check rows; `None` if the commands
    /// couldn't be recorded (not Vulkan).
    fn submit_copy(&self, frame: PendingFrame) -> Option<PendingFrame> {
        let commands =
            dmabuf::copy_commands(&self.device, &frame.source, frame.region, &frame.texture, |encoder| {
                record_drawn_check(encoder, &frame.texture, &frame.check);
            })?;
        frame.mapped.store(MAP_WAITING, Ordering::Release);
        self.shared.stats.count(|c| c.gpu_copies += 1);
        self.queue.submit(commands);
        let mapped = frame.mapped.clone();
        frame.check.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            mapped.store(if result.is_ok() { MAP_DONE } else { MAP_FAILED }, Ordering::Release);
        });
        Some(frame)
    }

    /// Takes the GPU frames whose copy has finished: a drawn one becomes the view's frame; the
    /// newest one, if it isn't drawn yet, is copied again, or given up after
    /// `FRAME_READY_TIMEOUT`. Never waits for the GPU.
    pub fn resolve_frames(&self) {
        let frames = &self.frames;
        self.start_scheduled();
        if !frames.has_pending() {
            return;
        }
        let _ = self.device.poll(wgpu::PollType::Poll);
        loop {
            let frame = {
                let mut pending = frames.pending.borrow_mut();
                // The GPU finishes them in order: if the oldest isn't done, none is.
                match pending.front() {
                    Some(f) if f.mapped.load(Ordering::Acquire) != MAP_WAITING => pending.pop_front(),
                    _ => None,
                }
            };
            let Some(frame) = frame else { return };
            let newest = !frames.has_pending();
            // Can't tell (read-back failed): show it as before.
            let (drawn, letterboxed) = if frame.mapped.load(Ordering::Acquire) == MAP_FAILED {
                (true, false)
            } else {
                check_rows(&frame.check, frame.texture.width())
            };
            frame.check.unmap();
            let watching = frames.letterbox_until.get().filter(|until| Instant::now() < *until).is_some();
            let skipped = frames.skipped_bars.get();
            if drawn && watching && letterboxed && !frames.edges_black.get() && skipped < MAX_SKIPPED_BARS {
                frames.skipped_bars.set(skipped + 1);
                // Right after a resize Chromium can capture its compositor's frame of the old
                // size scaled into a buffer of the new size, with black bars, and reports it as
                // a normal frame: the page shrank for a frame. The real one follows.
                tracing::debug!(view = ?self.view, "skipped a letterboxed frame after a resize");
                self.shared.stats.dropped();
                frames.wants_repaint.set(newest);
                self.recycle_check(frame.check);
                self.recycle_texture(frame.texture);
                continue;
            }
            if drawn {
                // The page at its new size has come: no more scaled frames of the old one.
                if !letterboxed && self.is_page_size(frame.texture.width(), frame.texture.height()) {
                    frames.letterbox_until.set(None);
                }
                tracing::trace!(view = ?self.view, after = ?frame.since.elapsed(), "GPU frame taken");
                frames.skipped_bars.set(0);
                frames.edges_black.set(letterboxed);
                self.recycle_check(frame.check);
                self.store(frame.texture);
                continue;
            }
            self.shared.stats.late();
            if !newest {
                // A newer frame is on its way already.
                self.recycle_check(frame.check);
                self.recycle_texture(frame.texture);
                continue;
            }
            if frame.since.elapsed() >= FRAME_READY_TIMEOUT {
                tracing::debug!(view = ?self.view, "GPU frame never got drawn; keeping the previous one");
                self.shared.stats.dropped();
                frames.wants_repaint.set(true);
                self.recycle_check(frame.check);
                self.recycle_texture(frame.texture);
                return;
            }
            // Copy it again: by now Chromium has most likely finished drawing it.
            if let Some(frame) = self.submit_copy(frame) {
                frames.pending.borrow_mut().push_back(frame);
            }
            return;
        }
    }

    /// Whether a shared frame shows the page scaled to fit the buffer instead of at its real
    /// size. Around a resize Chromium can capture the page at one size into a buffer of the
    /// other (letterboxed: bars along one side); drawn, the page shrank or grew for a frame.
    /// A buffer of the old size with the page at its new size inside it is fine (it is
    /// cropped, see `content_region`).
    fn is_scaled(&self, e: &AcceleratedPaintInfoCommon) -> bool {
        let content = (e.content_rect.width, e.content_rect.height);
        if e.has_source_size != 0 && (e.source_size.width, e.source_size.height) != content {
            return true;
        }
        let coded = (e.coded_size.width, e.coded_size.height);
        let letterboxed = content != coded && (content.0 == coded.0 || content.1 == coded.1);
        if !letterboxed {
            return false;
        }
        !self.is_page_size(content.0.max(0) as u32, content.1.max(0) as u32)
    }

    /// Whether `width` × `height` pixels is the page's size as Chromium lays it out (see
    /// `view_rect`), give or take rounding.
    fn is_page_size(&self, width: u32, height: u32) -> bool {
        let scale = self.geometry.scale.get();
        let logical = self.geometry.logical.get();
        let expected = |l: f32| (l.round().max(1.0) * scale).round() as i64;
        let near = |a: u32, b: i64| (i64::from(a) - b).abs() <= 1;
        near(width, expected(logical.x)) && near(height, expected(logical.y))
    }

    fn frame_texture(&self, size: wgpu::Extent3d, format: wgpu::TextureFormat) -> wgpu::Texture {
        let mut spare = self.frames.spare_textures.borrow_mut();
        // Spares of another size are of a size the page had before a resize: kept, they would
        // fill the spare slots for good and every later frame would get a new texture.
        spare.retain(|t| t.size() == size && t.format() == format);
        if let Some(texture) = spare.pop() {
            return texture;
        }
        self.shared.stats.count(|c| c.frame_textures += 1);
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("page frame"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: FRAME_USAGE,
            view_formats: &[],
        })
    }

    fn check_buffer(&self, width: u32) -> wgpu::Buffer {
        let size = check_buffer_size(width);
        let mut spare = self.frames.spare_checks.borrow_mut();
        // As for frame textures: buffers of an old width would block the spare slots.
        spare.retain(|b| b.size() == size);
        if let Some(buffer) = spare.pop() {
            return buffer;
        }
        self.shared.stats.count(|c| c.check_buffers += 1);
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame drawn check"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }

    fn recycle_check(&self, buffer: wgpu::Buffer) {
        let mut spare = self.frames.spare_checks.borrow_mut();
        if spare.len() < 3 {
            spare.push(buffer);
        }
    }

    /// Software paint: upload BGRA pixels, reusing the texture while the size is unchanged.
    fn upload(&self, pixels: &[u8], width: u32, height: u32) {
        // Right after a resize Chromium can hand over the page of the old size scaled into the
        // new buffer with a black bar along one edge (as on the shared-buffer path, see
        // `is_scaled`): skip it, the real frame follows. At most a few in a row, so a page that
        // really has a black edge still gets drawn.
        let watching = self.frames.letterbox_until.get().is_some_and(|until| Instant::now() < until);
        let bar = has_black_bar(pixels, width as usize, height as usize);
        if watching && bar && !self.frames.edges_black.get() {
            let skipped = self.frames.skipped_bars.get();
            if skipped < MAX_SKIPPED_BARS {
                self.frames.skipped_bars.set(skipped + 1);
                self.shared.stats.dropped();
                self.frames.wants_repaint.set(true);
                return;
            }
        }
        self.frames.skipped_bars.set(0);
        self.frames.edges_black.set(bar);
        let reusable = self
            .frames
            .texture
            .borrow()
            .as_ref()
            .filter(|t| {
                t.width() == width
                    && t.height() == height
                    && t.format() == wgpu::TextureFormat::Bgra8Unorm
                    && t.usage().contains(wgpu::TextureUsages::COPY_DST)
            })
            .cloned();
        let texture = reusable.unwrap_or_else(|| {
            self.shared.stats.count(|c| c.frame_textures += 1);
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("cef software frame"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: FRAME_USAGE,
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
    pub(super) struct RenderHandlerBuilder {
        ctx: ViewContext,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else { return };
            let size = self.ctx.geometry.logical.get();
            rect.width = (size.x.round() as i32).max(1);
            rect.height = (size.y.round() as i32).max(1);
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            let Some(screen_info) = screen_info else { return 0 };
            screen_info.device_scale_factor = self.ctx.geometry.scale.get();
            1
        }

        fn on_accelerated_paint(
            &self,
            browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            let Some(info) = info else { return };
            if type_ != PaintElementType::default() {
                return;
            }
            let e = &info.extra;
            tracing::trace!(
                coded = ?(e.coded_size.width, e.coded_size.height),
                visible = ?(e.visible_rect.x, e.visible_rect.y, e.visible_rect.width, e.visible_rect.height),
                content = ?(e.content_rect.x, e.content_rect.y, e.content_rect.width, e.content_rect.height),
                source = ?(e.source_size.width, e.source_size.height),
                update = ?(e.capture_update_rect.x, e.capture_update_rect.y, e.capture_update_rect.width, e.capture_update_rect.height),
                "accelerated paint"
            );
            // Around a resize Chromium can capture the page at one size into a buffer of the
            // other, scaled to fit: skip those (the page would shrink or grow for a frame) and
            // wait for one at its real size.
            let started = Instant::now();
            if self.ctx.is_scaled(e) {
                self.ctx.shared.stats.dropped();
                if let Some(host) = browser.and_then(|b| b.host()) {
                    host.invalidate(PaintElementType::default());
                }
            } else {
                self.ctx.submit_gpu_frame(info);
            }
            self.ctx.shared.stats.paint(started.elapsed());
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
            // SAFETY: CEF guarantees `buffer` holds width * height BGRA pixels for the duration
            // of this callback; we copy out of it before returning.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            let started = Instant::now();
            self.ctx.upload(pixels, width as u32, height as u32);
            self.ctx.shared.stats.paint(started.elapsed());
        }

        fn start_dragging(
            &self,
            browser: Option<&mut Browser>,
            drag_data: Option<&mut DragData>,
            allowed_ops: DragOperationsMask,
            x: ::std::os::raw::c_int,
            y: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let (Some(host), Some(data)) = (browser.and_then(|b| b.host()), drag_data) else { return 0 };
            self.ctx.drag.start(&host, data, allowed_ops, x, y).into()
        }

        fn update_drag_cursor(&self, browser: Option<&mut Browser>, operation: DragOperationsMask) {
            let host = browser.as_ref().and_then(|b| b.host());
            let cursor = self.ctx.drag.set_operation(host.as_ref(), operation);
            self.ctx.emit(browser, EngineEvent::CursorChanged { view: self.ctx.view, cursor });
        }
    }
}

wrap_display_handler! {
    pub(super) struct DisplayHandlerBuilder {
        ctx: ViewContext,
    }

    impl DisplayHandler {
        fn on_console_message(
            &self,
            browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let message = message.map(|m| m.to_string()).unwrap_or_default();
            if let Some(meta) = message.strip_prefix(PAGE_META_PREFIX) {
                let meta = parse_page_meta(meta);
                self.ctx.emit(browser, EngineEvent::PageMetaChanged { view: self.ctx.view, meta });
                return 1;
            }
            if !message.starts_with(PAGE_MESSAGE_PREFIX) {
                return 0;
            }
            self.ctx.emit(browser, EngineEvent::PageMessage { view: self.ctx.view, message });
            // Ours: keep it out of the page's console output.
            1
        }

        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|f| f.is_main() == 1) {
                let url = url.map(|u| u.to_string()).unwrap_or_default();
                self.ctx.emit(browser, EngineEvent::AddressChanged { view: self.ctx.view, url });
            }
        }

        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(|t| t.to_string()).unwrap_or_default();
            self.ctx.emit(browser, EngineEvent::TitleChanged { view: self.ctx.view, title });
        }

        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            let urls = icon_urls.map(string_list).unwrap_or_default();
            self.ctx.emit(browser, EngineEvent::FaviconUrlsChanged { view: self.ctx.view, urls });
        }

        fn on_loading_progress_change(&self, browser: Option<&mut Browser>, progress: f64) {
            self.ctx.emit(browser, EngineEvent::LoadProgress { view: self.ctx.view, progress });
        }

        fn on_fullscreen_mode_change(&self, browser: Option<&mut Browser>, fullscreen: ::std::os::raw::c_int) {
            let fullscreen = fullscreen != 0;
            self.ctx.emit(browser, EngineEvent::FullscreenRequested { view: self.ctx.view, fullscreen });
        }

        fn on_cursor_change(
            &self,
            browser: Option<&mut Browser>,
            _cursor: ::std::os::raw::c_ulong,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            let cursor = cursor_kind(type_);
            self.ctx.emit(browser, EngineEvent::CursorChanged { view: self.ctx.view, cursor });
            1
        }
    }
}

/// Switches a DevTools window to a panel (`"console"`, …) through the frontend's host API
/// (what Chromium's own "Console" shortcut calls). The frontend sets that API up a moment
/// after its document loads, so this retries for a few seconds.
pub(super) fn show_devtools_panel(frame: &Frame, panel: &str) {
    // Called again a moment later too: the frontend restores its last panel while it starts,
    // which would undo an earlier call.
    let script = format!(
        "(function t(n){{ if (window.DevToolsAPI && DevToolsAPI.showPanel) {{ \
         DevToolsAPI.showPanel('{panel}'); \
         setTimeout(function(){{ DevToolsAPI.showPanel('{panel}'); }}, 1500); }} \
         else if (n < 50) {{ setTimeout(function(){{ t(n + 1); }}, 100); }} }})(0);"
    );
    frame.execute_java_script(Some(&script.as_str().into()), None, 0);
}

wrap_load_handler! {
    pub(super) struct LoadHandlerBuilder {
        ctx: ViewContext,
    }

    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code: ::std::os::raw::c_int) {
            let Some(frame) = frame else { return };
            if self.ctx.devtools
                && frame.is_main() == 1
                && let Some(panel) = self.ctx.devtools_panel.borrow_mut().take()
            {
                show_devtools_panel(frame, panel);
            }
        }

        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            let Some(frame) = frame else { return };
            let url = CefString::from(&frame.url()).to_string();
            if frame.is_main() == 1 && !self.ctx.devtools {
                frame.execute_java_script(Some(&PAGE_META_SCRIPT.into()), Some(&url.as_str().into()), 0);
            }
            // Axion's own pages and the DevTools frontend get none of the scripts meant for web
            // pages.
            if self.ctx.devtools || is_internal(&url) {
                return;
            }
            let page_script = self.ctx.shared.page_script.borrow().clone();
            if let Some(script) = page_script {
                frame.execute_java_script(Some(&script.as_str().into()), Some(&url.as_str().into()), 0);
            }
            // Shields' cosmetic filters, injected as the document starts so ads never show.
            if let Some(script) = self.ctx.net.policy().and_then(|p| p.page_script(&url)) {
                frame.execute_java_script(Some(&script.as_str().into()), Some(&url.as_str().into()), 0);
            }
        }

        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            self.ctx.emit(browser, EngineEvent::LoadingStateChanged {
                view: self.ctx.view,
                loading: is_loading != 0,
                can_go_back: can_go_back != 0,
                can_go_forward: can_go_forward != 0,
            });
        }
    }
}

wrap_life_span_handler! {
    pub(super) struct LifeSpanHandlerBuilder {
        ctx: ViewContext,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let kind = match *target_disposition.as_ref() {
                cef_window_open_disposition_t::CEF_WOD_NEW_BACKGROUND_TAB => NewViewKind::BackgroundTab,
                cef_window_open_disposition_t::CEF_WOD_NEW_FOREGROUND_TAB => NewViewKind::ForegroundTab,
                _ => NewViewKind::Popup,
            };
            let url = target_url.map(|u| u.to_string()).unwrap_or_default();
            let opener_frame =
                frame.filter(|f| f.is_main() == 0).map(|f| CefString::from(&f.url()).to_string());
            self.ctx.emit(browser, EngineEvent::NewViewRequested {
                opener: self.ctx.view,
                url,
                kind,
                user_gesture: user_gesture != 0,
                opener_frame,
            });
            // Never let CEF open windows itself; the browser decides.
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            // A DevTools browser is created by Chromium (not by `create_browser`): hand it to the
            // engine, which keeps it in the view.
            if self.ctx.devtools
                && let Some(browser) = browser
            {
                self.ctx.shared.devtools_created.borrow_mut().push((self.ctx.view, browser.clone()));
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser.as_ref() {
                self.ctx.net.set_browser(browser.identifier(), None);
            }
            self.ctx.emit(browser, EngineEvent::Closed { view: self.ctx.view });
        }
    }
}

wrap_find_handler! {
    pub(super) struct FindHandlerBuilder {
        ctx: ViewContext,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            browser: Option<&mut Browser>,
            _identifier: ::std::os::raw::c_int,
            count: ::std::os::raw::c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: ::std::os::raw::c_int,
            final_update: ::std::os::raw::c_int,
        ) {
            self.ctx.emit(browser, EngineEvent::FindResult {
                view: self.ctx.view,
                active: active_match_ordinal.max(0) as u32,
                count: count.max(0) as u32,
                final_update: final_update != 0,
            });
        }
    }
}

/// Receives a downloaded favicon.
#[derive(Clone)]
pub(super) struct FaviconRequest {
    pub ctx: ViewContext,
    pub browser_id: i32,
    pub scale: f32,
    /// An image of the page (for copying it), not a favicon: reported as `ImageReady`.
    pub page_image: bool,
}

wrap_download_image_callback! {
    pub(super) struct FaviconCallbackBuilder {
        request: FaviconRequest,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            image_url: Option<&CefString>,
            _http_status_code: ::std::os::raw::c_int,
            image: Option<&mut Image>,
        ) {
            use cef::sys::{cef_alpha_type_t, cef_color_type_t};

            let ctx = &self.request.ctx;
            let url = image_url.map(|u| u.to_string()).unwrap_or_default();
            let page_image = self.request.page_image;
            let failed = || {
                let event = if page_image {
                    EngineEvent::ImageFailed { view: ctx.view, url: url.clone() }
                } else {
                    EngineEvent::FaviconFailed { view: ctx.view, url: url.clone() }
                };
                ctx.shared.events.borrow_mut().push((self.request.browser_id, event));
            };
            let Some(image) = image else { return failed() };
            let (mut width, mut height) = (0, 0);
            let Some(bitmap) = image.as_bitmap(
                self.request.scale,
                cef_color_type_t::CEF_COLOR_TYPE_RGBA_8888.into(),
                cef_alpha_type_t::CEF_ALPHA_TYPE_POSTMULTIPLIED.into(),
                Some(&mut width),
                Some(&mut height),
            ) else {
                return failed();
            };
            if width <= 0 || height <= 0 {
                return failed();
            }
            let mut rgba = vec![0; bitmap.size()];
            bitmap.data(Some(&mut rgba), 0);
            if rgba.len() != width as usize * height as usize * 4 {
                return failed();
            }
            let (view, width, height) = (ctx.view, width as u32, height as u32);
            let event = if page_image {
                EngineEvent::ImageReady { view, url, width, height, rgba }
            } else {
                EngineEvent::FaviconReady { view, url, width, height, rgba }
            };
            ctx.shared.events.borrow_mut().push((self.request.browser_id, event));
        }
    }
}

fn is_internal(url: &str) -> bool {
    url.strip_prefix(INTERNAL_SCHEME).is_some_and(|rest| rest.starts_with(':'))
}

/// Parses the page meta script's report (see `page_meta.js`).
fn parse_page_meta(text: &str) -> PageMeta {
    let mut meta = PageMeta::default();
    for line in text.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("i") => {
                let url = fields.next().unwrap_or_default().to_owned();
                if url.is_empty() {
                    continue;
                }
                let media = fields.next().unwrap_or_default().to_owned();
                let sizes = fields.next().unwrap_or_default().to_owned();
                let mime = fields.next().unwrap_or_default().to_owned();
                meta.icons.push(IconLink { url, media, sizes, mime });
            }
            Some("c") => {
                let channels: Vec<u8> = fields
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|c| c.trim().parse().ok())
                    .collect();
                let Ok(rgb) = <[u8; 3]>::try_from(channels) else { continue };
                let media = fields.next().unwrap_or_default().to_owned();
                meta.theme_colors.push(ThemeColor { rgb, media });
            }
            _ => {}
        }
    }
    meta
}

/// The strings of a list CEF lends to a callback. (The `cef` crate's `clone().into_iter()`
/// of a borrowed list yields nothing, so the list is read in place.)
fn string_list(list: &mut CefStringList) -> Vec<String> {
    let raw: *mut cef::sys::_cef_string_list_t = list.into();
    if raw.is_null() {
        return Vec::new();
    }
    // SAFETY: `raw` is the live list CEF passed to the running callback; each value is a copy
    // we own and clear after converting it.
    unsafe {
        (0..cef::sys::cef_string_list_size(raw))
            .filter_map(|i| {
                let mut value = std::mem::zeroed();
                if cef::sys::cef_string_list_value(raw, i, &mut value) == 0 {
                    return None;
                }
                let text = CefString::from(std::ptr::from_ref(&value)).to_string();
                cef::sys::cef_string_utf16_clear(&mut value);
                Some(text)
            })
            .collect()
    }
}

pub(super) fn favicon_callback(request: FaviconRequest) -> DownloadImageCallback {
    FaviconCallbackBuilder::new(request)
}

// A media view may reach the local network and this computer (a video on a home server, a
// local test page): Chromium asks for that permission, and Alloy style would ignore the ask.
wrap_permission_handler! {
    pub(super) struct PermissionHandlerBuilder {
        net: Arc<Network>,
        view: ViewId,
    }

    impl PermissionHandler {
        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            _requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            use cef::sys::cef_permission_request_types_t as types;
            let network = types::CEF_PERMISSION_TYPE_LOCAL_NETWORK as u32
                | types::CEF_PERMISSION_TYPE_LOOPBACK_NETWORK as u32;
            if self.net.media_view(self.view).is_none()
                || requested_permissions == 0
                || requested_permissions & !network != 0
            {
                return 0;
            }
            if let Some(callback) = callback {
                callback.cont(cef::sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_ACCEPT.into());
            }
            1
        }
    }
}

// Off-screen Chromium has no native context menu (it logs "Window handle is required" and
// cancels): the click is reported and Axion's own glass menu takes over.
wrap_context_menu_handler! {
    pub(super) struct ContextMenuHandlerBuilder {
        ctx: ViewContext,
    }

    impl ContextMenuHandler {
        fn run_context_menu(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            use cef::sys::cef_context_menu_edit_state_flags_t as edit;
            use cef::sys::cef_context_menu_media_type_t as media_type;

            let Some(params) = params else { return 0 };
            let text = |s: CefStringUserfree| CefString::from(&s).to_string();
            let media = match *params.media_type().as_ref() {
                media_type::CM_MEDIATYPE_IMAGE => crate::ContextMedia::Image,
                media_type::CM_MEDIATYPE_VIDEO => crate::ContextMedia::Video,
                media_type::CM_MEDIATYPE_AUDIO => crate::ContextMedia::Audio,
                media_type::CM_MEDIATYPE_CANVAS => crate::ContextMedia::Canvas,
                media_type::CM_MEDIATYPE_NONE => crate::ContextMedia::None,
                _ => crate::ContextMedia::Other,
            };
            let flags = params.edit_state_flags().as_ref().0;
            let has = |flag: edit| flags & flag.0 != 0;
            let event = crate::ContextMenuParams {
                pos: Vec2::new(params.xcoord() as f32, params.ycoord() as f32),
                media,
                source_url: text(params.source_url()),
                has_image_contents: params.has_image_contents() != 0,
                link_url: text(params.unfiltered_link_url()),
                selection_text: text(params.selection_text()),
                editable: params.is_editable() != 0,
                edit: crate::EditFlags {
                    undo: has(edit::CM_EDITFLAG_CAN_UNDO),
                    redo: has(edit::CM_EDITFLAG_CAN_REDO),
                    cut: has(edit::CM_EDITFLAG_CAN_CUT),
                    copy: has(edit::CM_EDITFLAG_CAN_COPY),
                    paste: has(edit::CM_EDITFLAG_CAN_PASTE),
                    delete: has(edit::CM_EDITFLAG_CAN_DELETE),
                    select_all: has(edit::CM_EDITFLAG_CAN_SELECT_ALL),
                },
                page_url: text(params.page_url()),
                frame_url: text(params.frame_url()),
            };
            self.ctx.emit(browser, EngineEvent::ContextMenu { view: self.ctx.view, params: event });
            // Ours to answer: nothing to run natively.
            if let Some(callback) = callback {
                callback.cancel();
            }
            1
        }
    }
}

wrap_client! {
    pub(super) struct ClientBuilder {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
        life_span: LifeSpanHandler,
        // What the network stack asks of the browser: requests and permissions.
        request: (RequestHandler, PermissionHandler),
        download: DownloadHandler,
        // Find in page, and the page's context menu (reported, not shown).
        find: (FindHandler, ContextMenuHandler),
    }

    impl Client {
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.find.1.clone())
        }

        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.request.1.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.0.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.download.clone())
        }

        fn find_handler(&self) -> Option<FindHandler> {
            Some(self.find.0.clone())
        }
    }
}

pub(super) fn build_client(ctx: &ViewContext) -> Client {
    ClientBuilder::new(
        RenderHandlerBuilder::new(ctx.clone()),
        DisplayHandlerBuilder::new(ctx.clone()),
        LoadHandlerBuilder::new(ctx.clone()),
        LifeSpanHandlerBuilder::new(ctx.clone()),
        (
            build_request_handler(&ctx.net, ctx.view),
            PermissionHandlerBuilder::new(Arc::clone(&ctx.net), ctx.view),
        ),
        super::download::build_download_handler(&ctx.shared.downloads),
        (FindHandlerBuilder::new(ctx.clone()), ContextMenuHandlerBuilder::new(ctx.clone())),
    )
}

fn cursor_kind(cursor: CursorType) -> CursorKind {
    use cef_cursor_type_t::*;
    match *cursor.as_ref() {
        CT_HAND => CursorKind::Pointer,
        CT_IBEAM | CT_VERTICALTEXT => CursorKind::Text,
        CT_CROSS => CursorKind::Crosshair,
        CT_WAIT => CursorKind::Wait,
        CT_PROGRESS => CursorKind::Progress,
        CT_HELP => CursorKind::Help,
        CT_MOVE => CursorKind::Move,
        CT_GRAB => CursorKind::Grab,
        CT_GRABBING => CursorKind::Grabbing,
        CT_NOTALLOWED | CT_NODROP => CursorKind::NotAllowed,
        CT_EASTWESTRESIZE | CT_EASTRESIZE | CT_WESTRESIZE | CT_COLUMNRESIZE => CursorKind::ResizeHorizontal,
        CT_NORTHSOUTHRESIZE | CT_NORTHRESIZE | CT_SOUTHRESIZE | CT_ROWRESIZE => CursorKind::ResizeVertical,
        CT_NONE => CursorKind::Hidden,
        _ => CursorKind::Default,
    }
}

/// The part of a shared frame that holds the page. Right after a resize Chromium can still
/// deliver a buffer of the old size with the page at its new size letterboxed inside it (black
/// bars around `content_rect`); drawn whole, the page jumped sideways next to a black band.
fn content_region(frame: &wgpu::Texture, content: &Rect) -> (wgpu::Origin3d, wgpu::Extent3d) {
    let (width, height) = (frame.width(), frame.height());
    let whole = (wgpu::Origin3d::ZERO, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
    let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
        u32::try_from(content.x),
        u32::try_from(content.y),
        u32::try_from(content.width),
        u32::try_from(content.height),
    ) else {
        return whole;
    };
    let inside = w > 0 && h > 0 && x.saturating_add(w) <= width && y.saturating_add(h) <= height;
    if !inside {
        return whole;
    }
    (wgpu::Origin3d { x, y, z: 0 }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 })
}

/// Rows of a GPU frame read back to check it: its first and last, and some evenly spread
/// between them (see `check_rows`).
const DRAWN_CHECK_ROWS: u32 = 10;

fn check_row_bytes(width: u32) -> u32 {
    (4 * width).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

fn check_buffer_size(width: u32) -> u64 {
    u64::from(check_row_bytes(width) * DRAWN_CHECK_ROWS)
}

/// Records copying a few rows spread across `texture` into `buffer` (see `is_drawn`).
fn record_drawn_check(encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, buffer: &wgpu::Buffer) {
    let (width, height) = (texture.width(), texture.height());
    let row_bytes = check_row_bytes(width);
    for i in 0..DRAWN_CHECK_ROWS {
        // The edges, and evenly spread between them (the same row more than once on tiny
        // frames).
        let y = match i {
            0 => 0,
            i if i == DRAWN_CHECK_ROWS - 1 => height.saturating_sub(1),
            i => height * (2 * i - 1) / (2 * (DRAWN_CHECK_ROWS - 2)),
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: u64::from(i * row_bytes),
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d { width, height: 1, depth_or_array_layers: 1 },
        );
    }
}

/// What the rows read back into the mapped `buffer` (see `record_drawn_check`) say about a
/// frame: whether it is drawn (none of their pixels fully transparent: Chromium draws pages
/// over an opaque background), and whether it looks letterboxed (black bars along its top and
/// bottom, or its left and right edges).
fn check_rows(buffer: &wgpu::Buffer, width: u32) -> (bool, bool) {
    let row_bytes = check_row_bytes(width) as usize;
    let Ok(data) = buffer.slice(..).get_mapped_range() else { return (true, false) };
    let rows: Vec<&[[u8; 4]]> = (0..DRAWN_CHECK_ROWS as usize)
        .map(|i| data[i * row_bytes..][..4 * width as usize].as_chunks::<4>().0)
        .collect();
    let drawn = rows.iter().all(|row| row.iter().all(|pixel| pixel[3] != 0));
    let black = |pixel: &[u8; 4]| pixel[..3] == [0, 0, 0];
    let bars_above_below = [rows[0], rows[rows.len() - 1]].iter().all(|row| row.iter().all(black));
    let bars_beside = rows.iter().all(|row| row.first().is_some_and(black) && row.last().is_some_and(black));
    (drawn, bars_above_below || bars_beside)
}
