//! Per-view CEF handlers. Every callback runs on the main thread during
//! `do_message_loop_work`, so plain `Rc`/`RefCell` state is enough.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cef::rc::Rc as _;
use cef::sys::{cef_cursor_type_t, cef_window_open_disposition_t};
use cef::*;
use glam::Vec2;

use super::network::{Network, build_request_handler};
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
}

#[derive(Default)]
pub(super) struct ViewFrames {
    pub texture: RefCell<Option<wgpu::Texture>>,
    pub serial: Cell<u64>,
    pub imported_any: Cell<bool>,
    pub import_failures: Cell<u32>,
}

/// How often a shared GPU frame that isn't drawn yet is copied again (see
/// `ViewContext::import_gpu_frame`) …
const FRAME_RETRY_INTERVAL: Duration = Duration::from_micros(500);
/// … and for how long before it is given up (the previous frame stays and Chromium is asked
/// to paint again).
const FRAME_READY_TIMEOUT: Duration = Duration::from_millis(25);

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
}

impl ViewContext {
    fn emit(&self, browser: Option<&mut Browser>, event: EngineEvent) {
        let id = browser.map_or(-1, |b| b.identifier());
        self.shared.events.borrow_mut().push((id, event));
    }

    fn store(&self, texture: wgpu::Texture) {
        self.frames.texture.replace(Some(texture));
        self.frames.serial.set(self.frames.serial.get() + 1);
    }

    /// Copies a frame Chromium shares as a dma-buf into a texture of our own. Returns `false`
    /// if no drawn frame could be taken (Chromium should then paint again).
    ///
    /// Chromium calls `on_accelerated_paint` once the frame's drawing is *submitted* on its GPU,
    /// not once it has finished, and NVIDIA's driver attaches no fence to the dma-buf that we
    /// could wait for. Copied at once, a buffer can still be empty (the first use of a pooled
    /// buffer, e.g. after every resize) or partly drawn, and the page flashed blank. The buffer
    /// can't be copied later instead: it goes back to Chromium's pool when the callback returns
    /// (copied on the next pump, even more frames came out empty). So the copy is checked, and
    /// taken again shortly until it is drawn. Chromium draws pages over an opaque background,
    /// so a drawn frame has no fully transparent pixels.
    fn import_gpu_frame(&self, info: &AcceleratedPaintInfo) -> bool {
        use cef::osr_texture_import::SharedTextureHandle;

        let frames = &self.frames;
        let started = Instant::now();
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            // SAFETY: we are inside CEF's `on_accelerated_paint` for this frame (the caller), and
            // the wgpu device was created with VULKAN_EXTERNAL_MEMORY_DMA_BUF. If CEF renders on
            // another GPU the import fails cleanly and the engine falls back to software paint.
            let imported =
                unsafe { SharedTextureHandle::new(info).import_texture(&self.device, &self.queue) };
            let texture = match imported {
                // Already a copy of our own (made and waited for by the import).
                Ok(texture) => crop_to_content(&self.device, &self.queue, texture, &info.extra.content_rect),
                Err(error) => {
                    if frames.import_failures.get() == 0 {
                        tracing::warn!(view = ?self.view, %error, "importing a GPU frame from Chromium failed");
                    }
                    frames.import_failures.set(frames.import_failures.get() + 1);
                    return true;
                }
            };
            frames.imported_any.set(true);
            match is_drawn(&self.device, &self.queue, &texture) {
                // Can't tell (read-back failed): show it as before.
                None | Some(true) => {
                    if attempts > 1 {
                        tracing::debug!(view = ?self.view, attempts, waited = ?started.elapsed(), "GPU frame was drawn late");
                    }
                    self.store(texture);
                    return true;
                }
                Some(false) if started.elapsed() >= FRAME_READY_TIMEOUT => {
                    tracing::debug!(view = ?self.view, attempts, "GPU frame never got drawn; keeping the previous one");
                    return false;
                }
                Some(false) => std::thread::sleep(FRAME_RETRY_INTERVAL),
            }
        }
    }

    /// Software paint: upload BGRA pixels, reusing the texture while the size is unchanged.
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
            let scaled = e.has_source_size != 0
                && (e.source_size.width, e.source_size.height) != (e.content_rect.width, e.content_rect.height);
            if (scaled || !self.ctx.import_gpu_frame(info))
                && let Some(host) = browser.and_then(|b| b.host())
            {
                host.invalidate(PaintElementType::default());
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
            // SAFETY: CEF guarantees `buffer` holds width * height BGRA pixels for the duration
            // of this callback; we copy out of it before returning.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            self.ctx.upload(pixels, width as u32, height as u32);
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

wrap_load_handler! {
    pub(super) struct LoadHandlerBuilder {
        ctx: ViewContext,
    }

    impl LoadHandler {
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            let Some(frame) = frame else { return };
            let url = CefString::from(&frame.url()).to_string();
            if frame.is_main() == 1 {
                frame.execute_java_script(Some(&PAGE_META_SCRIPT.into()), Some(&url.as_str().into()), 0);
            }
            // Axion's own pages get none of the scripts meant for web pages.
            if is_internal(&url) {
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

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser.as_ref() {
                self.ctx.net.set_browser(browser.identifier(), None);
            }
            self.ctx.emit(browser, EngineEvent::Closed { view: self.ctx.view });
        }
    }
}

/// Receives a downloaded favicon.
#[derive(Clone)]
pub(super) struct FaviconRequest {
    pub ctx: ViewContext,
    pub browser_id: i32,
    pub scale: f32,
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
            let failed = || {
                ctx.shared
                    .events
                    .borrow_mut()
                    .push((self.request.browser_id, EngineEvent::FaviconFailed { view: ctx.view, url: url.clone() }));
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
            ctx.shared.events.borrow_mut().push((
                self.request.browser_id,
                EngineEvent::FaviconReady {
                    view: ctx.view,
                    url,
                    width: width as u32,
                    height: height as u32,
                    rgba,
                },
            ));
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
                meta.icons.push(IconLink { url, media, sizes });
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

wrap_client! {
    pub(super) struct ClientBuilder {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
        life_span: LifeSpanHandler,
        request: RequestHandler,
        download: DownloadHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
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
            Some(self.request.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.download.clone())
        }
    }
}

pub(super) fn build_client(ctx: &ViewContext) -> Client {
    ClientBuilder::new(
        RenderHandlerBuilder::new(ctx.clone()),
        DisplayHandlerBuilder::new(ctx.clone()),
        LoadHandlerBuilder::new(ctx.clone()),
        LifeSpanHandlerBuilder::new(ctx.clone()),
        build_request_handler(&ctx.net, ctx.view),
        super::download::build_download_handler(&ctx.shared.downloads),
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
fn crop_to_content(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    frame: wgpu::Texture,
    content: &Rect,
) -> wgpu::Texture {
    let (width, height) = (frame.width(), frame.height());
    let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
        u32::try_from(content.x),
        u32::try_from(content.y),
        u32::try_from(content.width),
        u32::try_from(content.height),
    ) else {
        return frame;
    };
    let full = x == 0 && y == 0 && w == width && h == height;
    let inside = w > 0 && h > 0 && x.saturating_add(w) <= width && y.saturating_add(h) <= height;
    if full || !inside {
        return frame;
    }
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let cropped = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cef frame content"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: frame.format(),
        usage: frame.usage(),
        view_formats: &[],
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("cef frame crop") });
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &frame,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        cropped.as_image_copy(),
        size,
    );
    queue.submit([encoder.finish()]);
    cropped
}

/// Rows of a GPU frame read back to check that it is drawn.
const DRAWN_CHECK_ROWS: u32 = 8;

/// Whether a frame copied from Chromium is drawn: none of the pixels in a few rows across it
/// is fully transparent. `None` if it couldn't be read back.
fn is_drawn(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Option<bool> {
    let (width, height) = (texture.width(), texture.height());
    let rows = DRAWN_CHECK_ROWS.min(height);
    let row_bytes = (4 * width).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame drawn check"),
        size: u64::from(row_bytes * rows),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    for i in 0..rows {
        // Evenly spread, away from the edges.
        let y = height * (2 * i + 1) / (2 * rows);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: u64::from(i * row_bytes),
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d { width, height: 1, depth_or_array_layers: 1 },
        );
    }
    let submission = queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).ok()?;
    let data = buffer.slice(..).get_mapped_range().ok()?;
    let drawn = (0..rows as usize).all(|i| {
        let row = &data[i * row_bytes as usize..][..4 * width as usize];
        row.as_chunks::<4>().0.iter().all(|pixel| pixel[3] != 0)
    });
    Some(drawn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_meta() {
        let meta = parse_page_meta(
            "i\thttps://a.example/l.svg\t(prefers-color-scheme: light)\t\n\
             i\thttps://a.example/d.svg\t(prefers-color-scheme: dark)\tany\n\
             c\t13,17,23\t\nc\tbad\t\nx\ty",
        );
        assert_eq!(meta.icons.len(), 2);
        assert_eq!(meta.icons[1].media, "(prefers-color-scheme: dark)");
        assert_eq!(meta.icons[1].sizes, "any");
        assert_eq!(meta.theme_colors, vec![ThemeColor { rgb: [13, 17, 23], media: String::new() }]);
        assert!(
            is_internal("axion://home/") && !is_internal("axionx://home") && !is_internal("https://axion")
        );
    }
}
