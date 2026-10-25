//! Per-view CEF handlers. Every callback runs on the main thread during
//! `do_message_loop_work`, so plain `Rc`/`RefCell` state is enough.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use cef::rc::Rc as _;
use cef::sys::{cef_cursor_type_t, cef_window_open_disposition_t};
use cef::*;
use glam::Vec2;

use super::network::{Network, build_request_handler};
use crate::{CursorKind, EngineEvent, NewViewKind, PAGE_MESSAGE_PREFIX, ViewId};

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

    /// Copies a frame shared by Chromium into a texture of our own (reused while the size and
    /// format stay the same).
    fn copy_frame(&self, shared: &wgpu::Texture) {
        let reusable = self
            .frames
            .texture
            .borrow()
            .as_ref()
            .filter(|t| {
                t.size() == shared.size()
                    && t.format() == shared.format()
                    && t.usage().contains(wgpu::TextureUsages::COPY_DST)
            })
            .cloned();
        let texture = reusable.unwrap_or_else(|| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("cef frame copy"),
                size: shared.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: shared.format(),
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("cef frame copy") });
        encoder.copy_texture_to_texture(shared.as_image_copy(), texture.as_image_copy(), shared.size());
        self.queue.submit([encoder.finish()]);
        self.store(texture);
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
            let frames = &self.ctx.frames;
            // SAFETY: we are inside CEF's `on_accelerated_paint` for this frame, and the wgpu
            // device was created with VULKAN_EXTERNAL_MEMORY_DMA_BUF. If CEF renders on another
            // GPU the import fails cleanly and the engine falls back to software paint.
            match unsafe { SharedTextureHandle::new(info).import_texture(&self.ctx.device, &self.ctx.queue) } {
                Ok(shared) => {
                    frames.imported_any.set(true);
                    // Chromium's buffer is only ours during this callback (it is reused or freed
                    // on the next frame, reload or resize), so the frame is copied out now.
                    self.ctx.copy_frame(&shared);
                }
                Err(error) => {
                    if frames.import_failures.get() == 0 {
                        tracing::warn!(view = ?self.ctx.view, %error, "importing a GPU frame from Chromium failed");
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
