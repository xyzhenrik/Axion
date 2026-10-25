//! Chromium backend, via the Chromium Embedded Framework in off-screen rendering mode.
//!
//! Usage: call [`run_subprocess_if_needed`] first thing in `main` (Chromium re-launches the
//! browser binary for its renderer, GPU and utility processes), then create a wgpu device with
//! [`required_features`] and build a [`CefEngine`] on the main thread.

mod client;
mod gpu;
mod input;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::*;

pub use gpu::render_node_for;

use crate::{Engine, EngineError, EngineEvent, Frame, InputEvent, ViewId, ViewSize};
use client::{Shared, ViewContext, ViewFrames, ViewGeometry, build_client};
use input::InputState;

/// How long [`CefEngine::shutdown`] waits for views to close before giving up.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
struct ProcessFlags {
    no_sandbox: bool,
    render_node: Option<PathBuf>,
}

wrap_app! {
    struct AppBuilder {
        flags: ProcessFlags,
    }

    impl App {
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
            if self.flags.no_sandbox {
                command_line.append_switch(Some(&"no-sandbox".into()));
            }
            // Without a Wayland display (e.g. forced onto XWayland), Chromium must use X11 too.
            if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some() {
                command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&"x11".into()));
            }
            if let Some(node) = &self.flags.render_node {
                let node = node.to_string_lossy();
                command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&node.as_ref().into()));
            }
        }
    }
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
    let mut app = AppBuilder::new(ProcessFlags { no_sandbox: false, render_node: None });
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
    /// Upper bound for frames per second per view.
    pub max_frame_rate: u32,
}

struct ViewState {
    browser: Browser,
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
    shut_down: bool,
}

impl CefEngine {
    /// Initializes CEF. Must be called on the main thread, once per process.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, options: CefOptions) -> Result<Self, EngineError> {
        let args = args::Args::new();
        let mut app = AppBuilder::new(ProcessFlags {
            no_sandbox: options.no_sandbox,
            render_node: options.render_node.clone(),
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
        Ok(Self {
            device,
            queue,
            shared: Rc::default(),
            views: BTreeMap::new(),
            next_id: 1,
            accelerated: options.accelerated,
            max_frame_rate: options.max_frame_rate,
            input: InputState::default(),
            shut_down: false,
        })
    }

    fn create_browser(&self, ctx: &ViewContext, url: &str) -> Result<(Browser, i32), EngineError> {
        let mut client = build_client(ctx);
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            shared_texture_enabled: self.accelerated as _,
            // Frames are requested by our render loop, in step with presentation.
            external_begin_frame_enabled: 1,
            ..Default::default()
        };
        let settings =
            BrowserSettings { windowless_frame_rate: self.max_frame_rate as _, ..Default::default() };
        let browser = browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&url.into()),
            Some(&settings),
            None,
            None,
        )
        .ok_or_else(|| EngineError::CreateView(format!("Chromium refused to open {url}")))?;
        let id = browser.identifier();
        Ok((browser, id))
    }

    fn host(&self, view: ViewId) -> Option<BrowserHost> {
        self.views.get(&view).filter(|v| !v.closing).and_then(|v| v.browser.host())
    }

    fn browser(&self, view: ViewId) -> Option<&Browser> {
        self.views.get(&view).filter(|v| !v.closing).map(|v| &v.browser)
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
            if let Some(host) = state.browser.host() {
                host.close_browser(1);
            }
            let ctx = state.ctx.clone();
            let url = state.url.clone();
            ctx.frames.import_failures.set(0);
            match self.create_browser(&ctx, &url) {
                Ok((browser, browser_id)) => {
                    if let Some(state) = self.views.get_mut(&id) {
                        state.browser = browser;
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
        for state in self.views.values_mut() {
            if !state.closing {
                state.closing = true;
                if let Some(host) = state.browser.host() {
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
    fn create_view(&mut self, url: &str, size: ViewSize) -> Result<ViewId, EngineError> {
        let view = ViewId(self.next_id);
        self.next_id += 1;
        let ctx = ViewContext {
            view,
            device: self.device.clone(),
            queue: self.queue.clone(),
            geometry: Rc::new(ViewGeometry { logical: size.logical.into(), scale: size.scale.into() }),
            frames: Rc::new(ViewFrames::default()),
            shared: self.shared.clone(),
        };
        let (browser, browser_id) = self.create_browser(&ctx, url)?;
        self.views.insert(view, ViewState { browser, browser_id, ctx, url: url.to_owned(), closing: false });
        Ok(view)
    }

    fn close_view(&mut self, view: ViewId) {
        if let Some(state) = self.views.get_mut(&view)
            && !state.closing
        {
            state.closing = true;
            if let Some(host) = state.browser.host() {
                host.close_browser(1);
            }
        }
    }

    fn navigate(&mut self, view: ViewId, url: &str) {
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
        if let Some(host) = self.host(view) {
            host.was_hidden(hidden as _);
        }
    }

    fn set_focus(&mut self, view: ViewId, focused: bool) {
        if let Some(host) = self.host(view) {
            host.set_focus(focused as _);
        }
    }

    fn send_input(&mut self, view: ViewId, event: &InputEvent) {
        if let Some(host) = self.host(view) {
            self.input.send(&host, event);
        }
    }

    fn request_frame(&mut self, view: ViewId) {
        if let Some(host) = self.host(view) {
            host.send_external_begin_frame();
        }
    }

    fn frame(&self, view: ViewId) -> Option<Frame> {
        let frames = &self.views.get(&view)?.ctx.frames;
        let texture = frames.texture.borrow().clone()?;
        Some(Frame { texture, serial: frames.serial.get() })
    }

    fn pump(&mut self) -> Vec<EngineEvent> {
        do_message_loop_work();
        self.fall_back_to_software_if_needed();

        let raw = std::mem::take(&mut *self.shared.events.borrow_mut());
        let mut events = Vec::with_capacity(raw.len());
        for (browser_id, event) in raw {
            let view = match &event {
                EngineEvent::NewViewRequested { opener, .. } => *opener,
                EngineEvent::AddressChanged { view, .. }
                | EngineEvent::TitleChanged { view, .. }
                | EngineEvent::LoadingStateChanged { view, .. }
                | EngineEvent::LoadProgress { view, .. }
                | EngineEvent::FaviconUrlsChanged { view, .. }
                | EngineEvent::CursorChanged { view, .. }
                | EngineEvent::FullscreenRequested { view, .. }
                | EngineEvent::Closed { view } => *view,
            };
            let Some(state) = self.views.get_mut(&view) else { continue };
            // Events from a browser we replaced (software fallback) are stale.
            if state.browser_id != browser_id {
                continue;
            }
            match &event {
                EngineEvent::AddressChanged { url, .. } => state.url.clone_from(url),
                EngineEvent::Closed { .. } => {
                    self.views.remove(&view);
                }
                _ => {}
            }
            events.push(event);
        }
        events
    }
}
