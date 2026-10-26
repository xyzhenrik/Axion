//! Chromium backend, via the Chromium Embedded Framework in off-screen rendering mode.
//!
//! Usage: call [`run_subprocess_if_needed`] first thing in `main` (Chromium re-launches the
//! browser binary for its renderer, GPU and utility processes), then create a wgpu device with
//! [`required_features`] and build a [`CefEngine`] on the main thread.

mod client;
mod dmabuf;
mod download;
mod drag;
mod gpu;
mod input;
mod network;
mod paste;
mod schedule;
mod scheme;
mod settle;
mod stats;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use cef::*;

pub use gpu::render_node_for;
pub use schedule::Waker;

use crate::{
    DropData, Engine, EngineError, EngineEvent, FindRequest, Frame, InputEvent, MediaViewOptions, Modifiers,
    PasteData, RequestPolicy, ViewId, ViewSize,
};
use client::{FaviconRequest, Shared, ViewContext, ViewFrames, ViewGeometry, build_client, favicon_callback};
use input::InputState;
use network::Network;
use schedule::{BrowserProcessHandlerBuilder, PumpSchedule};

/// How long [`CefEngine::shutdown`] waits for views to close before giving up.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
/// `browser_id` of a view without a page: matches no CEF browser.
const NO_BROWSER: i32 = i32::MIN;
/// Opaque white, ARGB.
const PAGE_BACKGROUND: u32 = 0xFFFF_FFFF;

#[derive(Clone)]
struct ProcessFlags {
    no_sandbox: bool,
    render_node: Option<PathBuf>,
    hardware_video_decoding: bool,
    /// The browser process's message pump schedule (`None` in helper processes).
    schedule: Option<Arc<PumpSchedule>>,
}

wrap_app! {
    struct AppBuilder {
        flags: ProcessFlags,
    }

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            let schedule = self.flags.schedule.clone()?;
            Some(BrowserProcessHandlerBuilder::new(schedule))
        }

        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if let Some(registrar) = registrar {
                scheme::register_scheme(registrar);
            }
        }

        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            // Only the browser process; it forwards relevant switches to its children.
            if process_type.is_some_and(|t| !t.to_string().is_empty()) {
                return;
            }
            let Some(command_line) = command_line else { return };
            command_line.append_switch(Some(&"noerrdialogs".into()));
            command_line.append_switch(Some(&"hide-crash-restore-bubble".into()));
            // Chromium would keep its cookie encryption key in the desktop keyring (KWallet,
            // GNOME Keyring) and pop up the wallet's unlock dialog. Axion has its own vault for
            // passwords, so the key stays in the profile instead.
            command_line.append_switch_with_value(Some(&"password-store".into()), Some(&"basic".into()));
            if self.flags.no_sandbox {
                command_line.append_switch(Some(&"no-sandbox".into()));
            }
            // Without a Wayland display (e.g. forced onto XWayland), Chromium must use X11 too.
            if on_x11() {
                command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&"x11".into()));
            }
            if self.flags.hardware_video_decoding {
                // VA-API video decoding (Chromium's Linux names for it since M131).
                command_line.append_switch_with_value(
                    Some(&"enable-features".into()),
                    Some(&"AcceleratedVideoDecodeLinuxGL,AcceleratedVideoDecodeLinuxZeroCopyGL".into()),
                );
            }
            // Debugging aid: extra Chromium switches, e.g. "autoplay-policy=no-user-gesture-required".
            if let Ok(extra) = std::env::var("AXION_CHROMIUM_SWITCHES") {
                for switch in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    match switch.split_once('=') {
                        Some((name, value)) => command_line
                            .append_switch_with_value(Some(&name.into()), Some(&value.into())),
                        None => command_line.append_switch(Some(&switch.into())),
                    }
                }
            }
            if let Some(node) = &self.flags.render_node {
                let node = node.to_string_lossy();
                command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&node.as_ref().into()));
            }
        }
    }
}

/// Whether Chromium runs on X11 (`--ozone-platform=x11`, see `on_before_command_line_processing`)
/// rather than its default, Wayland.
fn on_x11() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
}

/// If this process was launched by Chromium as a helper, runs it and returns its exit code;
/// the caller must then exit. Returns `None` in the browser process.
pub fn run_subprocess_if_needed() -> Option<i32> {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let is_helper = args.as_cmd_line().is_some_and(|cmd| cmd.has_switch(Some(&"type".into())) == 1);
    if !is_helper {
        return None;
    }
    let mut app = AppBuilder::new(ProcessFlags {
        no_sandbox: false,
        render_node: None,
        hardware_video_decoding: false,
        schedule: None,
    });
    Some(execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut()))
}

/// wgpu features the device must have for zero-copy GPU frames.
pub fn required_features() -> wgpu::Features {
    wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF
}

#[derive(Clone, Debug)]
pub struct CefOptions {
    /// Profile data (cookies, cache, local storage).
    pub profile_dir: PathBuf,
    /// Share frames as GPU textures. Needs [`required_features`] on the device; falls back to
    /// software paint automatically if importing fails.
    pub accelerated: bool,
    /// Pin Chromium's GPU process to this DRM render node (see [`render_node_for`]).
    pub render_node: Option<PathBuf>,
    pub no_sandbox: bool,
    /// Frames per second views render at most (see `Engine::set_frame_rate`).
    pub max_frame_rate: u32,
    /// How long after Chromium hands over a GPU frame it is copied (see `ViewContext::submit_gpu_frame`).
    pub frame_settle: std::time::Duration,
    /// Decode video on the GPU (VA-API).
    pub hardware_video_decoding: bool,
    /// Wakes the embedder's event loop when Chromium has work (see [`Engine::next_pump`]).
    pub waker: Option<Waker>,
}

struct ViewState {
    /// `None` while discarded.
    browser: Option<Browser>,
    private: bool,
    browser_id: i32,
    ctx: ViewContext,
    url: String,
    closing: bool,
}

pub struct CefEngine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    shared: Rc<Shared>,
    views: BTreeMap<ViewId, ViewState>,
    next_id: u32,
    accelerated: bool,
    max_frame_rate: u32,
    input: InputState,
    /// In-memory profile shared by private views, created on first use.
    private_context: Option<RequestContext>,
    net: Arc<Network>,
    /// Pages' colour scheme (`None` until the browser sets one: Chromium's default).
    dark: Option<bool>,
    schedule: Arc<PumpSchedule>,
    shut_down: bool,
}

impl CefEngine {
    /// Initializes CEF. Must be called on the main thread, once per process.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, options: CefOptions) -> Result<Self, EngineError> {
        let args = args::Args::new();
        let schedule = Arc::new(PumpSchedule::default());
        schedule.set_waker(options.waker.clone());
        let mut app = AppBuilder::new(ProcessFlags {
            no_sandbox: options.no_sandbox,
            render_node: options.render_node.clone(),
            hardware_video_decoding: options.hardware_video_decoding,
            schedule: Some(schedule.clone()),
        });
        let profile = options.profile_dir.to_string_lossy().into_owned();
        let settings = Settings {
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            no_sandbox: options.no_sandbox as _,
            root_cache_path: profile.as_str().into(),
            cache_path: profile.as_str().into(),
            persist_session_cookies: 1,
            ..Default::default()
        };
        if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
            return Err(EngineError::Init("cef_initialize returned an error; see the log above".into()));
        }
        tracing::info!(accelerated = options.accelerated, render_node = ?options.render_node, "Chromium ready");
        let net = Arc::<Network>::default();
        scheme::install(&net, None, false);
        Ok(Self {
            device,
            queue,
            shared: Rc::new(Shared { frame_settle: options.frame_settle, ..Default::default() }),
            views: BTreeMap::new(),
            next_id: 1,
            accelerated: options.accelerated,
            max_frame_rate: options.max_frame_rate,
            input: InputState::default(),
            private_context: None,
            net,
            dark: None,
            schedule,
            shut_down: false,
        })
    }

    fn private_context(&mut self) -> Option<RequestContext> {
        if self.private_context.is_none() {
            // An empty cache path makes the context in-memory only.
            let settings = RequestContextSettings { persist_session_cookies: 0, ..Default::default() };
            self.private_context = request_context_create_context(Some(&settings), None);
            if let Some(context) = &self.private_context {
                scheme::install(&self.net, Some(context), true);
            }
            match (&self.private_context, self.dark) {
                (Some(context), Some(dark)) => apply_color_scheme(context, dark),
                (None, _) => tracing::error!("could not create the private browsing context"),
                _ => {}
            }
        }
        self.private_context.clone()
    }

    fn create_browser(
        &mut self,
        ctx: &ViewContext,
        url: &str,
        private: bool,
    ) -> Result<(Browser, i32), EngineError> {
        let mut context = if private {
            Some(self.private_context().ok_or_else(|| EngineError::CreateView("no private context".into()))?)
        } else {
            None
        };
        let mut client = build_client(ctx);
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            shared_texture_enabled: self.accelerated as _,
            // Chromium paces frames itself, at `windowless_frame_rate`. With begin frames sent
            // from our render loop instead, its compositor drew each one at once, so a page's
            // frame showed only on the next begin frame and the page's own frame rate (its
            // main thread: scripts, `requestAnimationFrame`) halved as soon as a frame took
            // more than a moment to make.
            external_begin_frame_enabled: 0,
            ..Default::default()
        };
        let settings = BrowserSettings {
            windowless_frame_rate: self.max_frame_rate as _,
            // Off-screen views are transparent by default; pages without a background of
            // their own must get white, as in any browser.
            background_color: PAGE_BACKGROUND,
            ..Default::default()
        };
        let browser = browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&url.into()),
            Some(&settings),
            None,
            context.as_mut(),
        )
        .ok_or_else(|| EngineError::CreateView(format!("Chromium refused to open {url}")))?;
        let id = browser.identifier();
        self.net.set_browser(id, Some(ctx.view));
        Ok((browser, id))
    }

    /// Takes the view's GPU frames whose copy has finished (see `ViewContext::resolve_frames`);
    /// asks Chromium to paint again if one never got drawn.
    fn resolve_frames(&self, state: &ViewState) {
        state.ctx.resolve_frames();
        if state.ctx.frames.wants_repaint.take()
            && !state.closing
            && let Some(host) = state.browser.as_ref().and_then(|b| b.host())
        {
            host.invalidate(PaintElementType::default());
        }
    }

    fn host(&self, view: ViewId) -> Option<BrowserHost> {
        self.browser(view).and_then(|b| b.host())
    }

    fn browser(&self, view: ViewId) -> Option<&Browser> {
        self.views.get(&view).filter(|v| !v.closing).and_then(|v| v.browser.as_ref())
    }

    /// If GPU frames can't be imported on this system, re-create every view with software paint.
    fn fall_back_to_software_if_needed(&mut self) {
        let broken = self.accelerated
            && self
                .views
                .values()
                .any(|v| v.ctx.frames.import_failures.get() > 0 && !v.ctx.frames.imported_any.get());
        if !broken {
            return;
        }
        tracing::warn!("GPU frame sharing doesn't work on this system; switching to software paint");
        self.accelerated = false;
        let ids: Vec<ViewId> = self.views.keys().copied().collect();
        for id in ids {
            let Some(state) = self.views.get(&id) else { continue };
            if state.closing {
                continue;
            }
            let Some(host) = state.browser.as_ref().and_then(|b| b.host()) else { continue };
            host.close_browser(1);
            let ctx = state.ctx.clone();
            let url = state.url.clone();
            let private = state.private;
            ctx.frames.import_failures.set(0);
            ctx.frames.release_shared();
            match self.create_browser(&ctx, &url, private) {
                Ok((browser, browser_id)) => {
                    if let (Some(dark), Some(host)) = (self.dark, browser.host()) {
                        emulate_color_scheme(&host, dark);
                    }
                    if let Some(state) = self.views.get_mut(&id) {
                        state.browser = Some(browser);
                        state.browser_id = browser_id;
                    }
                }
                Err(error) => tracing::error!(%error, "re-creating a view with software paint failed"),
            }
        }
    }

    /// Closes all views and shuts Chromium down. Called automatically on drop.
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        // Discarded views have no page to close.
        self.views.retain(|_, v| v.browser.is_some());
        for state in self.views.values_mut() {
            if !state.closing {
                state.closing = true;
                if let Some(host) = state.browser.as_ref().and_then(|b| b.host()) {
                    host.close_browser(1);
                }
            }
        }
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while !self.views.is_empty() && Instant::now() < deadline {
            self.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        if !self.views.is_empty() {
            tracing::warn!(open = self.views.len(), "views did not close in time");
        }
        self.views.clear();
        cef::shutdown();
    }
}

impl Drop for CefEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Engine for CefEngine {
    fn create_view(&mut self, url: &str, size: ViewSize, private: bool) -> Result<ViewId, EngineError> {
        let view = self.create_discarded_view(url, size, private);
        if let Err(e) = self.restore(view) {
            self.views.remove(&view);
            return Err(e);
        }
        Ok(view)
    }

    fn create_discarded_view(&mut self, url: &str, size: ViewSize, private: bool) -> ViewId {
        let view = ViewId(self.next_id);
        self.next_id += 1;
        let ctx = ViewContext {
            view,
            device: self.device.clone(),
            queue: self.queue.clone(),
            geometry: Rc::new(ViewGeometry { logical: size.logical.into(), scale: size.scale.into() }),
            frames: Rc::new(ViewFrames::default()),
            shared: self.shared.clone(),
            net: self.net.clone(),
            drag: Rc::default(),
        };
        let state = ViewState {
            browser: None,
            private,
            browser_id: NO_BROWSER,
            ctx,
            url: url.to_owned(),
            closing: false,
        };
        self.views.insert(view, state);
        view
    }

    fn close_view(&mut self, view: ViewId) {
        let Some(state) = self.views.get_mut(&view) else { return };
        match &state.browser {
            None => {
                self.views.remove(&view);
            }
            Some(browser) if !state.closing => {
                state.closing = true;
                state.ctx.frames.release_shared();
                if let Some(host) = browser.host() {
                    host.close_browser(1);
                }
            }
            Some(_) => {}
        }
    }

    fn set_color_scheme(&mut self, dark: bool) {
        if self.dark == Some(dark) {
            return;
        }
        self.dark = Some(dark);
        let contexts = [request_context_get_global_context(), self.private_context.clone()];
        for context in contexts.iter().flatten() {
            apply_color_scheme(context, dark);
        }
        for state in self.views.values().filter(|v| !v.closing) {
            if let Some(host) = state.browser.as_ref().and_then(|b| b.host()) {
                emulate_color_scheme(&host, dark);
            }
        }
    }

    fn set_download_options(&mut self, options: crate::DownloadOptions) {
        self.shared.downloads.options.replace(Some(options));
    }

    fn download_action(&mut self, id: u32, action: crate::DownloadAction) {
        let control = self.shared.downloads.controls.borrow().get(&id).cloned();
        let Some(control) = control else { return };
        match action {
            crate::DownloadAction::Cancel => control.cancel(),
            crate::DownloadAction::Pause => control.pause(),
            crate::DownloadAction::Resume => control.resume(),
        }
    }

    fn start_download(&mut self, view: ViewId, url: &str, directory: std::path::PathBuf) {
        let Some(host) = self.host(view) else { return };
        self.shared.downloads.targets.borrow_mut().insert(url.to_owned(), directory);
        host.start_download(Some(&url.into()));
    }

    fn clear_data(&mut self, what: &crate::ClearData) {
        let Some(context) = request_context_get_global_context() else { return };
        if what.cache {
            download::clear_cache(&context);
        }
        if what.all_site_data {
            download::delete_cookies(&context, None);
        }
        for host in &what.sites {
            download::delete_cookies(&context, Some(host));
        }
        // Local storage, IndexedDB, service workers … per origin, through any open page.
        let host =
            self.views.values().filter(|v| !v.closing && !v.private).find_map(|v| v.browser.as_ref()?.host());
        if let Some(host) = host {
            for site in &what.sites {
                for origin in
                    [format!("https://{site}"), format!("https://www.{site}"), format!("http://{site}")]
                {
                    let Some(mut params) = dictionary_value_create() else { continue };
                    params.set_string(Some(&"origin".into()), Some(&origin.as_str().into()));
                    params.set_string(Some(&"storageTypes".into()), Some(&"all".into()));
                    host.execute_dev_tools_method(
                        0,
                        Some(&"Storage.clearDataForOrigin".into()),
                        Some(&mut params),
                    );
                }
            }
        }
        if what.all_site_data || !what.sites.is_empty() {
            tracing::info!(
                all = what.all_site_data,
                sites = what.sites.len(),
                "cleared cookies and site data"
            );
        }
    }

    fn request_site_data(&mut self) {
        match request_context_get_global_context() {
            Some(context) => download::visit_cookies(&context, &self.shared.downloads),
            None => self.shared.downloads.cookie_visit_done.set(true),
        }
    }

    fn set_audio_muted(&mut self, view: ViewId, muted: bool) {
        if let Some(host) = self.host(view) {
            host.set_audio_muted(muted as _);
        }
    }

    fn set_frozen(&mut self, view: ViewId, frozen: bool) {
        let Some(host) = self.host(view) else { return };
        let Some(mut params) = dictionary_value_create() else { return };
        let state = if frozen { "frozen" } else { "active" };
        params.set_string(Some(&"state".into()), Some(&state.into()));
        // Id 0: CEF assigns one; the result isn't needed.
        host.execute_dev_tools_method(0, Some(&"Page.setWebLifecycleState".into()), Some(&mut params));
    }

    fn set_page_script(&mut self, script: Option<String>) {
        self.shared.page_script.replace(script);
    }

    fn set_internal_pages(&mut self, pages: Option<Arc<dyn crate::InternalPages>>) {
        *self.net.internal.write().unwrap_or_else(PoisonError::into_inner) = pages;
    }

    fn run_script(&mut self, view: ViewId, script: &str, user_gesture: bool) {
        if !user_gesture {
            if let Some(frame) = self.browser(view).and_then(|b| b.main_frame()) {
                frame.execute_java_script(Some(&script.into()), None, 0);
            }
            return;
        }
        // Only DevTools can run a script as if the user had clicked (fullscreen, play).
        let Some(host) = self.host(view) else { return };
        let Some(mut params) = dictionary_value_create() else { return };
        params.set_string(Some(&"expression".into()), Some(&script.into()));
        params.set_bool(Some(&"userGesture".into()), 1);
        host.execute_dev_tools_method(0, Some(&"Runtime.evaluate".into()), Some(&mut params));
    }

    fn discard(&mut self, view: ViewId) {
        let Some(state) = self.views.get_mut(&view).filter(|v| !v.closing) else { return };
        let Some(browser) = state.browser.take() else { return };
        // Keep the last frame as the tab's snapshot. Chromium's shared GPU buffers are
        // recycled once the page closes, so those are copied; software frames are ours.
        let frames = &state.ctx.frames;
        let snapshot = frames.texture.borrow().as_ref().map(|t| keep_frame(&self.device, &self.queue, t));
        frames.texture.replace(snapshot);
        frames.release_shared();
        // Events of the closed page (its `Closed` included) no longer match and are dropped.
        state.browser_id = NO_BROWSER;
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }

    fn restore(&mut self, view: ViewId) -> Result<(), EngineError> {
        let Some(state) = self.views.get(&view).filter(|v| v.browser.is_none()) else { return Ok(()) };
        let (ctx, url, private) = (state.ctx.clone(), state.url.clone(), state.private);
        let (browser, browser_id) = self.create_browser(&ctx, &url, private)?;
        if let (Some(dark), Some(host)) = (self.dark, browser.host()) {
            emulate_color_scheme(&host, dark);
        }
        if let Some(state) = self.views.get_mut(&view) {
            state.browser = Some(browser);
            state.browser_id = browser_id;
        }
        Ok(())
    }

    fn is_discarded(&self, view: ViewId) -> bool {
        self.views.get(&view).is_some_and(|v| v.browser.is_none())
    }

    fn navigate(&mut self, view: ViewId, url: &str) {
        if let Some(state) = self.views.get_mut(&view)
            && state.browser.is_none()
        {
            // Loads when the view is restored.
            state.url = url.to_owned();
            return;
        }
        if let Some(frame) = self.browser(view).and_then(|b| b.main_frame()) {
            frame.load_url(Some(&url.into()));
        }
    }

    fn go_back(&mut self, view: ViewId) {
        if let Some(b) = self.browser(view) {
            b.go_back();
        }
    }

    fn go_forward(&mut self, view: ViewId) {
        if let Some(b) = self.browser(view) {
            b.go_forward();
        }
    }

    fn reload(&mut self, view: ViewId, ignore_cache: bool) {
        if let Some(b) = self.browser(view) {
            if ignore_cache { b.reload_ignore_cache() } else { b.reload() }
        }
    }

    fn stop(&mut self, view: ViewId) {
        if let Some(b) = self.browser(view) {
            b.stop_load();
        }
    }

    fn set_zoom_level(&mut self, view: ViewId, level: f64) {
        if let Some(host) = self.host(view) {
            host.set_zoom_level(level);
        }
    }

    fn resize(&mut self, view: ViewId, size: ViewSize) {
        let Some(state) = self.views.get(&view) else { return };
        let geometry = &state.ctx.geometry;
        let scale_changed = geometry.scale.get() != size.scale;
        tracing::trace!(?view, from = ?geometry.logical.get(), to = ?size.logical, "resize view");
        if geometry.logical.get() != size.logical || scale_changed {
            state.ctx.frames.resized();
        }
        geometry.logical.set(size.logical);
        geometry.scale.set(size.scale);
        if let Some(host) = self.host(view) {
            if scale_changed {
                host.notify_screen_info_changed();
            }
            host.was_resized();
        }
    }

    fn set_hidden(&mut self, view: ViewId, hidden: bool) {
        let Some(host) = self.host(view) else { return };
        if hidden && let Some(state) = self.views.get(&view) {
            state.ctx.drag.cancel(&host);
        }
        host.was_hidden(hidden as _);
    }

    fn set_focus(&mut self, view: ViewId, focused: bool) {
        if let Some(host) = self.host(view) {
            host.set_focus(focused as _);
        }
    }

    fn send_input(&mut self, view: ViewId, event: &InputEvent) {
        let Some(host) = self.host(view) else { return };
        let Some(state) = self.views.get(&view) else { return };
        self.shared.stats.input_sent();
        if !state.ctx.drag.page_input(&host, &mut self.input, event) {
            self.input.send(&host, event);
        }
    }

    fn find(&mut self, view: ViewId, request: &FindRequest) {
        let Some(host) = self.host(view) else { return };
        if request.text.is_empty() {
            host.stop_finding(1);
            return;
        }
        host.find(
            Some(&request.text.as_str().into()),
            request.forward.into(),
            request.match_case.into(),
            request.next.into(),
        );
    }

    fn stop_finding(&mut self, view: ViewId) {
        if let Some(host) = self.host(view) {
            // Keep the selected match selected, as browsers do when find closes.
            host.stop_finding(0);
        }
    }

    fn drag_enter(&mut self, view: ViewId, data: &DropData, pos: glam::Vec2, modifiers: Modifiers) {
        let (Some(host), Some(state)) = (self.host(view), self.views.get(&view)) else { return };
        state.ctx.drag.external_enter(&host, &self.input, &data.files, pos, modifiers);
    }

    fn drag_over(&mut self, view: ViewId, pos: glam::Vec2, modifiers: Modifiers) {
        let (Some(host), Some(state)) = (self.host(view), self.views.get(&view)) else { return };
        state.ctx.drag.external_over(&host, &self.input, pos, modifiers);
    }

    fn drag_leave(&mut self, view: ViewId) {
        let (Some(host), Some(state)) = (self.host(view), self.views.get(&view)) else { return };
        state.ctx.drag.external_leave(&host);
    }

    fn drag_drop(&mut self, view: ViewId, pos: glam::Vec2, modifiers: Modifiers) {
        let (Some(host), Some(state)) = (self.host(view), self.views.get(&view)) else { return };
        state.ctx.drag.external_drop(&host, &self.input, pos, modifiers);
    }

    fn has_system_clipboard(&self) -> bool {
        // On Wayland only the client with keyboard focus sees the clipboard (and may set it),
        // and off-screen Chromium has no surface to get focus.
        on_x11()
    }

    fn paste(&mut self, view: ViewId, data: &PasteData) {
        if data.is_empty() {
            return;
        }
        // The frame with focus: the field may be inside an iframe.
        let Some(frame) = self.browser(view).and_then(|b| b.focused_frame().or_else(|| b.main_frame()))
        else {
            return;
        };
        frame.execute_java_script(Some(&paste::script(data).as_str().into()), None, 0);
    }

    fn fetch_favicon(&mut self, view: ViewId, url: &str, max_size: u32) {
        let Some(state) = self.views.get(&view).filter(|v| !v.closing) else { return };
        let Some(host) = state.browser.as_ref().and_then(|b| b.host()) else { return };
        let mut callback = favicon_callback(FaviconRequest {
            ctx: state.ctx.clone(),
            browser_id: state.browser_id,
            scale: state.ctx.geometry.scale.get(),
        });
        host.download_image(Some(&url.into()), 1, max_size, 0, Some(&mut callback));
    }

    fn set_request_policy(&mut self, policy: Option<Arc<dyn RequestPolicy>>) {
        *self.net.policy.write().unwrap_or_else(PoisonError::into_inner) = policy;
    }

    fn set_media_view(&mut self, view: ViewId, options: Option<MediaViewOptions>) {
        self.net.set_media_view(view, options);
    }

    fn request_frame(&mut self, _view: ViewId) {
        // Chromium renders on its own timer (see `create_browser`).
    }

    fn set_frame_rate(&mut self, fps: u32) {
        let fps = fps.max(1);
        if fps == self.max_frame_rate {
            return;
        }
        tracing::debug!(fps, "page frame rate");
        self.max_frame_rate = fps;
        for state in self.views.values().filter(|v| !v.closing) {
            if let Some(host) = state.browser.as_ref().and_then(|b| b.host()) {
                host.set_windowless_frame_rate(fps as _);
            }
        }
    }

    fn frame(&self, view: ViewId) -> Option<Frame> {
        let state = self.views.get(&view)?;
        // Frames whose copy has finished since the last pump show now.
        self.resolve_frames(state);
        let frames = &state.ctx.frames;
        let texture = frames.texture.borrow().clone()?;
        Some(Frame { texture, serial: frames.serial.get() })
    }

    fn next_pump(&self) -> Option<Instant> {
        // A frame waiting to be copied is due sooner than Chromium's own schedule.
        let due = self.views.values().filter_map(|v| v.ctx.frames.copy_due()).min();
        Some(due.map_or(self.schedule.next(), |d| d.min(self.schedule.next())))
    }

    fn pump(&mut self) -> Vec<EngineEvent> {
        let started = Instant::now();
        self.schedule.pumping();
        do_message_loop_work();
        self.shared.stats.pump(started.elapsed());
        self.fall_back_to_software_if_needed();
        for state in self.views.values() {
            self.resolve_frames(state);
        }
        for state in self.views.values().filter(|v| !v.closing && v.ctx.drag.is_waiting()) {
            if let Some(host) = state.browser.as_ref().and_then(|b| b.host()) {
                state.ctx.drag.flush(&host);
            }
        }

        let mut raw = std::mem::take(&mut *self.shared.events.borrow_mut());
        raw.extend(self.net.take_events());
        let mut events = Vec::with_capacity(raw.len());
        for (browser_id, event) in raw {
            let view = match &event {
                EngineEvent::NewViewRequested { opener, .. } => *opener,
                EngineEvent::AddressChanged { view, .. }
                | EngineEvent::TitleChanged { view, .. }
                | EngineEvent::LoadingStateChanged { view, .. }
                | EngineEvent::LoadProgress { view, .. }
                | EngineEvent::FaviconUrlsChanged { view, .. }
                | EngineEvent::PageMetaChanged { view, .. }
                | EngineEvent::InternalMessage { view, .. }
                | EngineEvent::FaviconReady { view, .. }
                | EngineEvent::FaviconFailed { view, .. }
                | EngineEvent::CursorChanged { view, .. }
                | EngineEvent::FullscreenRequested { view, .. }
                | EngineEvent::PageMessage { view, .. }
                | EngineEvent::RequestBlocked { view, .. }
                | EngineEvent::MediaSniffed { view, .. }
                | EngineEvent::NavigationBlocked { view, .. }
                | EngineEvent::FindResult { view, .. }
                | EngineEvent::Closed { view } => *view,
                // Not tied to a view (and never queued here).
                EngineEvent::DownloadUpdated(_) | EngineEvent::SiteData(_) => {
                    events.push(event);
                    continue;
                }
            };
            let Some(state) = self.views.get_mut(&view) else { continue };
            // Events from a browser we replaced (software fallback) are stale.
            if state.browser_id != browser_id {
                continue;
            }
            match &event {
                EngineEvent::AddressChanged { url, .. } => state.url.clone_from(url),
                EngineEvent::Closed { .. } => {
                    self.net.set_media_view(view, None);
                    self.views.remove(&view);
                }
                _ => {}
            }
            events.push(event);
        }
        // Downloads go on when their tab closes, so they aren't filtered by view.
        let downloads = &self.shared.downloads;
        events.extend(downloads.updates.borrow_mut().drain(..).map(EngineEvent::DownloadUpdated));
        if downloads.cookie_visit_done.replace(false) || downloads.cookie_visit_was_empty() {
            downloads.cookie_visit_started.set(None);
            let sites = downloads.cookie_sites.borrow().iter().map(|(h, n)| (h.clone(), *n)).collect();
            events.push(EngineEvent::SiteData(sites));
        }
        events
    }
}

/// A copy of `frame` that stays valid after Chromium recycles it (software frames are already
/// our own textures and are returned as they are).
fn keep_frame(device: &wgpu::Device, queue: &wgpu::Queue, frame: &wgpu::Texture) -> wgpu::Texture {
    if frame.usage().contains(wgpu::TextureUsages::COPY_DST)
        || !frame.usage().contains(wgpu::TextureUsages::COPY_SRC)
    {
        return frame.clone();
    }
    let copy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("discarded view snapshot"),
        size: frame.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: frame.format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("snapshot") });
    encoder.copy_texture_to_texture(frame.as_image_copy(), copy.as_image_copy(), frame.size());
    queue.submit([encoder.finish()]);
    copy
}

/// Sets a profile's colour scheme; Chromium passes it to its pages' `prefers-color-scheme`
/// at once, without a reload.
fn apply_color_scheme(context: &RequestContext, dark: bool) {
    use cef::sys::cef_color_variant_t;
    let variant = if dark {
        cef_color_variant_t::CEF_COLOR_VARIANT_DARK
    } else {
        cef_color_variant_t::CEF_COLOR_VARIANT_LIGHT
    };
    // Colour 0: keep Chromium's default accent; only the light/dark mode matters here.
    context.set_chrome_color_scheme(variant.into(), 0);
}

/// Sets the view's `prefers-color-scheme` through DevTools: off-screen views don't follow the
/// profile's colour scheme by themselves. Applies at once (pages get a `change` event) and
/// stays for later navigations of the view.
fn emulate_color_scheme(host: &BrowserHost, dark: bool) {
    let (Some(mut params), Some(mut features), Some(mut feature)) =
        (dictionary_value_create(), list_value_create(), dictionary_value_create())
    else {
        return;
    };
    feature.set_string(Some(&"name".into()), Some(&"prefers-color-scheme".into()));
    feature.set_string(Some(&"value".into()), Some(&(if dark { "dark" } else { "light" }).into()));
    features.set_dictionary(0, Some(&mut feature));
    params.set_list(Some(&"features".into()), Some(&mut features));
    host.execute_dev_tools_method(0, Some(&"Emulation.setEmulatedMedia".into()), Some(&mut params));
}
