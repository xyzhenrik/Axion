//! One browser window: owns the GPU surface, the engine, the UI and the tabs, and routes input
//! between the chrome and the page.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axion_compositor::{Compositor, ImageDraw, Layer, Rect, Scene};
use axion_config::{Action, Config, ConfigPaths, ConfigWatcher, Key, Keymap, Modifiers, ThemeMode};
use axion_engine::cef::{CefEngine, CefOptions};
use axion_engine::{CursorKind, Engine, EngineEvent, InputEvent, MouseButton, NewViewKind, ViewId, ViewSize};
use axion_ui::{Paint, Theme, Ui, UiCursor, UiEvent, UiInput};
use glam::Vec2;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window};

use crate::chrome::{Chrome, ChromeAction};
use crate::favicons::{FETCH_SIZE, Favicons};
use crate::input::{self, ClickCounter};
use crate::overview::{Overview, OverviewAction};
use crate::suggestions::{SuggestionList, suggest};
use crate::tabs::{Tab, Tabs, ZOOM_STEP};
use crate::url::resolve_input;

const DEFAULT_WINDOW_SIZE: (f64, f64) = (1440.0, 900.0);
/// How long before a `--capture --overview` screenshot the overview opens.
const OVERVIEW_CAPTURE_LEAD: Duration = Duration::from_secs(2);
/// Pixels per wheel notch.
const LINE_SCROLL: f32 = 100.0;
/// Upper bound for the page's frame rate.
const MAX_FRAME_RATE: u32 = 120;
/// Height (logical px) of the page's top strip whose lightness picks the chrome appearance.
const PROBE_HEIGHT: f32 = 48.0;
/// The chrome turns dark below this page lightness, and light again above `LIGHT_ABOVE`.
const DARK_BELOW: f32 = 0.4;
/// Fraction of the way the window tint moves towards the page colour per frame.
const TINT_EASE: f32 = 0.2;
const LIGHT_ABOVE: f32 = 0.6;
/// PCI vendor id of NVIDIA, the one driver where zero-copy frame import is verified.
const NVIDIA_VENDOR_ID: u32 = 0x10DE;

pub struct Options {
    /// Tabs to open; the first one is active. Empty opens the homepage.
    pub urls: Vec<String>,
    /// Open the tab overview shortly before `--capture` (for screenshots of it).
    pub overview: bool,
    /// Type this into the address field shortly before `--capture` (for screenshots of it).
    pub address: Option<String>,
    /// Save a screenshot here after `capture_after`, then quit.
    pub capture: Option<PathBuf>,
    pub capture_after: Duration,
    /// Initial window size in logical pixels.
    pub size: Option<(f64, f64)>,
}

pub struct Browser {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    compositor: Compositor,
    engine: CefEngine,
    ui: Ui,
    chrome: Chrome,
    overview: Overview,
    suggestions: SuggestionList,
    favicons: Favicons,
    /// History and bookmarks; `None` if the database couldn't be opened (browsing still works).
    store: Option<axion_store::Store>,
    /// Search text the overview's cards were last built for.
    overview_query: String,
    last_frame: Instant,
    tabs: Tabs,
    config: Config,
    keymap: Keymap,
    paths: ConfigPaths,
    watcher: Option<ConfigWatcher>,
    dark: bool,
    modifiers: Modifiers,
    /// Pointer position in logical pixels.
    pointer: Option<Vec2>,
    /// A mouse press that started on the chrome keeps going to the chrome until release.
    pointer_captured_by_ui: bool,
    pointer_on_page: bool,
    clicks: ClickCounter,
    page_cursor: CursorKind,
    clipboard: Option<arboard::Clipboard>,
    tabs_changed: bool,
    layout_size: Vec2,
    capture: Option<(PathBuf, Instant)>,
    /// The page's top-edge colour, used as the window background (`appearance.tint_from_page`).
    page_tint: Option<[f32; 4]>,
    /// When to open the overview for a `--capture --overview` screenshot.
    overview_at: Option<Instant>,
    address_at: Option<(Instant, String)>,
}

impl Browser {
    pub fn new(event_loop: &ActiveEventLoop, options: Options) -> Result<Self> {
        let paths = ConfigPaths::discover()?;
        let config = axion_config::load_config(&paths).unwrap_or_else(|e| {
            tracing::error!("{e}; using defaults");
            Config::default()
        });
        let keymap = load_keymap(&paths);
        let watcher = ConfigWatcher::new(&paths)
            .inspect_err(|e| tracing::warn!("live config reload unavailable: {e}"))
            .ok();

        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Axion")
                    .with_inner_size(winit::dpi::LogicalSize::<f64>::from(
                        options.size.unwrap_or(DEFAULT_WINDOW_SIZE),
                    ))
                    .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 360.0)),
            )?,
        );

        place_on_monitor(event_loop, &window);
        let (surface, adapter, device, queue) = crate::gpu::init(event_loop, &window)?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // Pages arrive as 8-bit sRGB-encoded BGRA and the compositor blends in that space, so
        // the swapchain must not re-encode: prefer a non-sRGB format.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
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
        surface.configure(&device, &surface_config);

        let info = adapter.get_info();
        let accelerated = adapter.features().contains(axion_engine::cef::required_features())
            && match std::env::var("AXION_OSR").as_deref() {
                Ok("software") => false,
                Ok("gpu") => true,
                // Zero-copy import is only verified on NVIDIA's driver so far. On RADV (AMD) it
                // currently triggers a GPU memory fault and context loss, so other drivers use
                // software paint until that is understood (see plan.md, Phase 0 follow-ups).
                // Under X11/XWayland Chromium has no GBM device for shared buffers at all.
                _ => info.vendor == NVIDIA_VENDOR_ID && std::env::var_os("WAYLAND_DISPLAY").is_some(),
            };
        if !accelerated {
            tracing::info!(driver = %info.driver, "using software paint for pages (set AXION_OSR=gpu to force GPU sharing)");
        }
        let profile = profile_dir()?;
        let store = axion_store::Store::open(&profile.join("axion.sqlite"))
            .inspect_err(|e| tracing::error!("history unavailable: {e}"))
            .ok();
        let engine = CefEngine::new(
            device.clone(),
            queue.clone(),
            CefOptions {
                profile_dir: profile,
                accelerated,
                render_node: if std::env::var_os("AXION_NO_RENDER_NODE").is_some() {
                    None
                } else {
                    axion_engine::cef::render_node_for(info.vendor, info.device)
                },
                no_sandbox: std::env::var_os("AXION_NO_SANDBOX").is_some(),
                max_frame_rate: MAX_FRAME_RATE,
            },
        )?;

        let dark = is_dark(&config, &window);
        let scale = window.scale_factor() as f32;
        let mut ui = Ui::new(Theme::new(&config, dark), scale);
        let compositor = Compositor::new(&device, &queue, format, &mut ui.fonts());
        let queue = queue.clone();
        let chrome = Chrome::build(&mut ui, &config);
        let suggestions = SuggestionList::build(&mut ui);
        let favicons = Favicons::new(device.clone(), queue.clone());
        // Built after the chrome so it draws above it.
        let overview = Overview::build(&mut ui);

        let mut browser = Self {
            window,
            surface,
            device,
            queue,
            surface_config,
            compositor,
            engine,
            ui,
            chrome,
            overview,
            suggestions,
            store,
            favicons,
            overview_query: String::new(),
            last_frame: Instant::now(),
            page_tint: None,
            tabs: Tabs::default(),
            config,
            keymap,
            paths,
            watcher,
            dark,
            modifiers: Modifiers::default(),
            pointer: None,
            pointer_captured_by_ui: false,
            pointer_on_page: false,
            clicks: ClickCounter::default(),
            page_cursor: CursorKind::Default,
            clipboard: arboard::Clipboard::new().inspect_err(|e| tracing::warn!("no clipboard: {e}")).ok(),
            tabs_changed: true,
            layout_size: Vec2::ZERO,
            capture: options.capture.clone().map(|path| (path, Instant::now() + options.capture_after)),
            address_at: options.address.clone().filter(|_| options.capture.is_some()).map(|text| {
                (Instant::now() + options.capture_after.saturating_sub(OVERVIEW_CAPTURE_LEAD), text)
            }),
            overview_at: (options.overview && options.capture.is_some())
                .then(|| Instant::now() + options.capture_after.saturating_sub(OVERVIEW_CAPTURE_LEAD)),
        };
        browser.update_layout();
        let mut urls = options.urls;
        if urls.is_empty() {
            urls.push(browser.config.general.homepage.clone());
        }
        browser.open_tab(&urls[0], true)?;
        // Background tabs go in right after the active one, so add them last-first.
        for url in urls[1..].iter().rev() {
            browser.open_tab(url, false)?;
        }
        Ok(browser)
    }

    fn logical_size(&self) -> Vec2 {
        Vec2::new(self.surface_config.width as f32, self.surface_config.height as f32) / self.scale()
    }

    fn scale(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    fn page_rect(&self) -> Rect {
        self.chrome.page_rect(&self.config, self.logical_size())
    }

    fn view_size(&self) -> ViewSize {
        ViewSize { logical: self.page_rect().size, scale: self.scale() }
    }

    /// Re-applies the responsive layout and resizes pages when the window or chrome changed.
    fn update_layout(&mut self) {
        let size = self.logical_size();
        self.ui.set_viewport(size, self.scale());
        self.chrome.apply_layout(&mut self.ui, &self.config, size);
        self.overview.resize(&mut self.ui, &self.tabs, &self.favicons, size);
        let view_size = self.view_size();
        for tab in self.tabs.all() {
            self.engine.resize(tab.view, view_size);
        }
        self.layout_size = size;
    }

    // ---- tabs ----

    fn open_tab(&mut self, url: &str, activate: bool) -> Result<()> {
        let view = self.engine.create_view(url, self.view_size())?;
        let previous = self.tabs.active().map(|t| t.view);
        let mut tab = Tab::new(view, url);
        // Background tabs render until they have a thumbnail for the overview.
        tab.needs_snapshot = !activate;
        self.tabs.insert(tab, activate);
        if activate {
            self.on_active_changed(previous);
        }
        self.tabs_changed = true;
        Ok(())
    }

    fn close_tab(&mut self, index: usize) {
        let previous = self.tabs.active().map(|t| t.view);
        let Some(tab) = self.tabs.remove(index) else { return };
        self.engine.close_view(tab.view);
        self.tabs_changed = true;
        if self.tabs.is_empty() {
            let home = self.config.general.homepage.clone();
            if let Err(e) = self.open_tab(&home, true) {
                tracing::error!("opening a new tab failed: {e:#}");
            }
        } else if previous != self.tabs.active().map(|t| t.view) || previous == Some(tab.view) {
            self.on_active_changed(None);
        }
    }

    fn select_tab(&mut self, index: usize) {
        let previous = self.tabs.active().map(|t| t.view);
        if self.tabs.select(index) {
            self.on_active_changed(previous);
        }
    }

    fn on_active_changed(&mut self, previous: Option<ViewId>) {
        if let Some(view) = previous {
            self.engine.set_focus(view, false);
            self.engine.set_hidden(view, true);
        }
        if let Some(tab) = self.tabs.active() {
            let view = tab.view;
            self.engine.set_hidden(view, false);
            self.engine.resize(view, self.view_size());
            self.engine.set_focus(view, true);
        }
        self.chrome.set_editing(&mut self.ui, false);
        self.tabs_changed = true;
    }

    fn active_view(&self) -> Option<ViewId> {
        self.tabs.active().map(|t| t.view)
    }

    // ---- commands ----

    fn run(&mut self, action: Action) {
        let view = self.active_view();
        match action {
            Action::FocusAddressBar => self.edit_address(),
            Action::NewTab => {
                let home = self.config.general.homepage.clone();
                if let Err(e) = self.open_tab(&home, true) {
                    tracing::error!("opening a new tab failed: {e:#}");
                }
                self.edit_address();
            }
            Action::CloseTab => self.close_tab(self.tabs.active_index()),
            Action::ReopenClosedTab => {
                if let Some(url) = self.tabs.pop_closed()
                    && let Err(e) = self.open_tab(&url, true)
                {
                    tracing::error!("reopening a tab failed: {e:#}");
                }
            }
            Action::NextTab | Action::PreviousTab => {
                let previous = self.active_view();
                if self.tabs.select_relative(if action == Action::NextTab { 1 } else { -1 }) {
                    self.on_active_changed(previous);
                }
            }
            Action::SelectTab(n) => self.select_tab(n as usize - 1),
            Action::SelectLastTab => self.select_tab(self.tabs.len().saturating_sub(1)),
            Action::Reload | Action::HardReload => {
                if let Some(view) = view {
                    self.engine.reload(view, action == Action::HardReload);
                }
            }
            Action::Back => {
                if let Some(view) = view {
                    self.engine.go_back(view);
                }
            }
            Action::Forward => {
                if let Some(view) = view {
                    self.engine.go_forward(view);
                }
            }
            Action::ToggleSidebar => {
                self.chrome.toggle_sidebar();
                self.update_layout();
            }
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                if let Some(tab) = self.tabs.active_mut() {
                    tab.zoom = match action {
                        Action::ZoomIn => tab.zoom + ZOOM_STEP,
                        Action::ZoomOut => tab.zoom - ZOOM_STEP,
                        _ => 0.0,
                    };
                    let (view, zoom) = (tab.view, tab.zoom);
                    self.engine.set_zoom_level(view, zoom);
                }
            }
            Action::ToggleFullscreen => {
                let fullscreen = self.window.fullscreen().is_none().then_some(Fullscreen::Borderless(None));
                self.window.set_fullscreen(fullscreen);
            }
            Action::TabOverview => {
                if self.overview.is_open() {
                    self.close_overview(self.tabs.active_index());
                } else {
                    self.open_overview();
                }
            }
            Action::Find | Action::OpenSettings => {
                tracing::info!("{action} arrives in a later phase");
            }
            Action::Quit => {}
        }
    }

    fn open_overview(&mut self) {
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
        self.chrome.set_editing(&mut self.ui, false);
        let window = self.logical_size();
        let page = self.page_rect();
        let events = self.overview.open(&mut self.ui, &self.tabs, &self.favicons, &self.config, window, page);
        self.overview_query.clear();
        self.handle_ui_events(events);
    }

    /// Closes the overview into tab `index`, which becomes the active tab.
    fn close_overview(&mut self, index: usize) {
        self.select_tab(index);
        let page = self.page_rect();
        let events =
            self.overview.close(&mut self.ui, &self.tabs, &self.config, self.tabs.active_index(), page);
        self.handle_ui_events(events);
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, true);
        }
    }

    fn overview_action(&mut self, action: OverviewAction) {
        match action {
            OverviewAction::Select(i) => self.close_overview(i),
            OverviewAction::Done => self.close_overview(self.tabs.active_index()),
            OverviewAction::Close(i) => {
                self.close_tab(i);
                self.overview.rebuild(&mut self.ui, &self.tabs, &self.favicons);
            }
            OverviewAction::NewTab => {
                let home = self.config.general.homepage.clone();
                if let Err(e) = self.open_tab(&home, true) {
                    tracing::error!("opening a new tab failed: {e:#}");
                }
                self.overview.rebuild(&mut self.ui, &self.tabs, &self.favicons);
                self.close_overview(self.tabs.active_index());
            }
        }
    }

    fn edit_address(&mut self) {
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
        let events = self.chrome.set_editing(&mut self.ui, true);
        self.handle_ui_events(events);
    }

    fn navigate_from_address(&mut self, text: &str) {
        let Some(url) = resolve_input(text, &self.config.general.search_url) else { return };
        self.navigate_to(url);
    }

    fn navigate_to(&mut self, url: String) {
        self.suggestions.hide(&mut self.ui);
        if let Some(tab) = self.tabs.active_mut() {
            tab.url.clone_from(&url);
            let view = tab.view;
            self.engine.navigate(view, &url);
            self.engine.set_focus(view, true);
        }
        self.chrome.set_editing(&mut self.ui, false);
        self.tabs_changed = true;
    }

    fn handle_ui_events(&mut self, events: Vec<UiEvent>) {
        for event in events {
            match event {
                UiEvent::Clicked { action, .. } if OverviewAction::from_id(action).is_some() => {
                    if let Some(action) = OverviewAction::from_id(action) {
                        self.overview_action(action);
                    }
                }
                UiEvent::Submitted { node, .. } if node == self.overview.search_field() => {
                    if let Some(i) = self.overview.first_match() {
                        self.close_overview(i);
                    }
                }
                UiEvent::Cancelled { node } if node == self.overview.search_field() => {
                    self.close_overview(self.tabs.active_index());
                }
                UiEvent::Clicked { action, .. } if SuggestionList::row_action(action).is_some() => {
                    let url = SuggestionList::row_action(action)
                        .and_then(|i| self.suggestions.get(i))
                        .map(|s| s.url.clone());
                    if let Some(url) = url {
                        self.navigate_to(url);
                    }
                }
                UiEvent::Clicked { action, .. } => match ChromeAction::from_id(action) {
                    Some(ChromeAction::ToggleSidebar) => self.run(Action::ToggleSidebar),
                    Some(ChromeAction::Back) => self.run(Action::Back),
                    Some(ChromeAction::Forward) => self.run(Action::Forward),
                    Some(ChromeAction::ReloadOrStop) => {
                        if let Some(tab) = self.tabs.active() {
                            let (view, loading) = (tab.view, tab.loading);
                            if loading { self.engine.stop(view) } else { self.engine.reload(view, false) }
                        }
                    }
                    Some(ChromeAction::EditAddress) => self.edit_address(),
                    Some(ChromeAction::NewTab) => self.run(Action::NewTab),
                    Some(ChromeAction::Overview) => self.run(Action::TabOverview),
                    Some(ChromeAction::Shield) => tracing::info!("the Shields popover arrives in Phase 4"),
                    Some(ChromeAction::Share) => {
                        // Until the share sheet exists: copy the address.
                        let url = self.tabs.active().map(|t| t.url.clone());
                        if let (Some(url), Some(clipboard)) = (url, &mut self.clipboard) {
                            let _ = clipboard.set_text(url);
                            tracing::info!("address copied");
                        }
                    }
                    Some(ChromeAction::ToggleTabsSection) => self.chrome.toggle_section(&mut self.ui, true),
                    Some(ChromeAction::ToggleSavedSection) => self.chrome.toggle_section(&mut self.ui, false),
                    Some(
                        action @ (ChromeAction::PageMenu
                        | ChromeAction::NewTabGroup
                        | ChromeAction::Private
                        | ChromeAction::Bookmarks
                        | ChromeAction::ReadingList
                        | ChromeAction::History),
                    ) => tracing::info!("{action:?} arrives in a later phase"),
                    Some(ChromeAction::SelectTab(i)) => self.select_tab(i),
                    Some(ChromeAction::CloseTab(i)) => self.close_tab(i),
                    None => {}
                },
                UiEvent::Submitted { node, text } if node == self.chrome.address_field => {
                    match self.suggestions.selected() {
                        Some(s) => self.navigate_to(s.url.clone()),
                        None => self.navigate_from_address(&text),
                    }
                }
                UiEvent::Cancelled { .. } => {
                    self.suggestions.hide(&mut self.ui);
                    self.chrome.set_editing(&mut self.ui, false);
                    if let Some(view) = self.active_view() {
                        self.engine.set_focus(view, true);
                    }
                }
                UiEvent::FocusChanged { node: None } => {
                    self.suggestions.hide(&mut self.ui);
                    self.chrome.set_editing(&mut self.ui, false);
                    self.tabs_changed = true;
                }
                UiEvent::Copy(text) => {
                    if let Some(clipboard) = &mut self.clipboard {
                        let _ = clipboard.set_text(text);
                    }
                }
                UiEvent::PasteRequested { .. } => {
                    let text = self.clipboard.as_mut().and_then(|c| c.get_text().ok());
                    if let Some(text) = text {
                        self.ui.paste(&text);
                    }
                }
                _ => {}
            }
        }
    }

    // ---- engine ----

    /// Runs the engine and applies what happened. Call often (every loop iteration).
    pub fn pump(&mut self) {
        for event in self.engine.pump() {
            tracing::debug!(?event, "engine");
            match event {
                EngineEvent::AddressChanged { view, url } => {
                    if let Some(tab) = self.tabs.by_view_mut(view) {
                        tab.url = url;
                        self.tabs_changed = true;
                    }
                }
                EngineEvent::TitleChanged { view, title } => {
                    if let (Some(store), Some(tab)) =
                        (&self.store, self.tabs.all().iter().find(|t| t.view == view))
                        && let Err(e) = store.set_title(&tab.url, &title)
                    {
                        tracing::warn!("saving a title failed: {e}");
                    }
                    if let Some(tab) = self.tabs.by_view_mut(view) {
                        tab.title = title;
                        self.tabs_changed = true;
                    }
                }
                EngineEvent::LoadingStateChanged { view, loading, can_go_back, can_go_forward } => {
                    if !loading
                        && let (Some(store), Some(tab)) =
                            (&self.store, self.tabs.all().iter().find(|t| t.view == view))
                        && let Err(e) = store.record_visit(&tab.url, &tab.title, unix_now())
                    {
                        tracing::warn!("saving history failed: {e}");
                    }
                    if let Some(tab) = self.tabs.by_view_mut(view) {
                        tab.loading = loading;
                        tab.can_go_back = can_go_back;
                        tab.can_go_forward = can_go_forward;
                        if loading {
                            tab.progress = 0.0;
                        }
                        self.tabs_changed = true;
                    }
                }
                EngineEvent::LoadProgress { view, progress } => {
                    if let Some(tab) = self.tabs.by_view_mut(view) {
                        tab.progress = progress;
                        if Some(view) == self.active_view() {
                            self.chrome.sync_active(&mut self.ui, &self.tabs);
                        }
                    }
                }
                EngineEvent::CursorChanged { view, cursor } => {
                    if Some(view) == self.active_view() {
                        self.page_cursor = cursor;
                    }
                }
                EngineEvent::FullscreenRequested { view, fullscreen } => {
                    if Some(view) == self.active_view() {
                        self.chrome.set_page_fullscreen(fullscreen);
                        self.window.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
                        self.update_layout();
                    }
                }
                EngineEvent::NewViewRequested { url, kind, user_gesture, .. } => {
                    // Popup policy (Shields, Phase 4 will make this configurable): pages may only
                    // open tabs in response to a click or key press.
                    if !user_gesture {
                        tracing::info!(%url, "blocked a popup opened without user interaction");
                        continue;
                    }
                    let activate = kind != NewViewKind::BackgroundTab;
                    if let Err(e) = self.open_tab(&url, activate) {
                        tracing::error!("opening {url} failed: {e:#}");
                    }
                }
                EngineEvent::FaviconUrlsChanged { view, urls } => {
                    let page = self.tabs.all().iter().find(|t| t.view == view).map(|t| t.url.clone());
                    // CEF often reports no icon links; then use the site's /favicon.ico.
                    let icon =
                        |page: &str| urls.first().cloned().or_else(|| crate::favicons::fallback_url(page));
                    if let Some(page) = page
                        && let Some(icon) = icon(&page)
                        && self.favicons.should_fetch(&page)
                    {
                        self.engine.fetch_favicon(view, &icon, FETCH_SIZE);
                    }
                }
                EngineEvent::FaviconReady { view, width, height, rgba, .. } => {
                    if let Some(page) = self.tabs.all().iter().find(|t| t.view == view).map(|t| t.url.clone())
                    {
                        self.favicons.insert(&page, width, height, &rgba);
                        self.tabs_changed = true;
                        if self.overview.is_open() {
                            self.overview.rebuild(&mut self.ui, &self.tabs, &self.favicons);
                        }
                    }
                }
                EngineEvent::Closed { .. } => {}
            }
        }
        self.reload_config_if_changed();
    }

    fn reload_config_if_changed(&mut self) {
        let Some(watcher) = &self.watcher else { return };
        let changed = watcher.poll();
        if changed.config {
            match axion_config::load_config(&self.paths) {
                Ok(config) => {
                    tracing::info!("settings reloaded");
                    self.config = config;
                    self.apply_theme();
                    self.update_layout();
                    self.tabs_changed = true;
                }
                Err(e) => tracing::error!("{e}; keeping the previous settings"),
            }
        }
        if changed.keymap {
            self.keymap = load_keymap(&self.paths);
            tracing::info!("keyboard shortcuts reloaded");
        }
    }

    /// Follows the page's top edge: tints the window around the page with its colour (like
    /// Safari) and flips the chrome light/dark to stay legible (with hysteresis).
    fn adapt_appearance(&mut self, page: &wgpu::TextureView, texture: &wgpu::Texture) {
        let appearance = &self.config.appearance;
        let adapt = appearance.adapt_to_page && appearance.theme == ThemeMode::Auto;
        if !appearance.tint_from_page && !adapt {
            self.page_tint = None;
            return;
        }
        let edge = (PROBE_HEIGHT * self.scale() / texture.height() as f32).min(1.0);
        let Some(color) = self.compositor.probe_color(page, Rect::new(0.0, 0.0, 1.0, edge)) else { return };
        if self.config.appearance.tint_from_page {
            // Ease towards the new colour so navigations don't flash.
            let target = [color[0], color[1], color[2], 1.0];
            let current = self.page_tint.unwrap_or(target);
            self.page_tint = Some(std::array::from_fn(|i| current[i] + (target[i] - current[i]) * TINT_EASE));
        } else {
            self.page_tint = None;
        }
        if !adapt {
            return;
        }
        // Rec. 709 luma of the sRGB-encoded colour: what the eye judges as light or dark.
        let lightness = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
        let dark = if self.dark { lightness < LIGHT_ABOVE } else { lightness < DARK_BELOW };
        if dark != self.dark {
            self.dark = dark;
            self.ui.set_theme(Theme::new(&self.config, dark));
        }
    }

    fn apply_theme(&mut self) {
        self.dark = is_dark(&self.config, &self.window);
        self.ui.set_theme(Theme::new(&self.config, self.dark));
    }

    // ---- window events ----

    /// Returns `false` when the window should close.
    pub fn window_event(&mut self, event: WindowEvent) -> Result<bool> {
        match event {
            WindowEvent::CloseRequested => return Ok(false),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    self.surface_config.width = size.width;
                    self.surface_config.height = size.height;
                    self.surface.configure(&self.device, &self.surface_config);
                    self.update_layout();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => self.update_layout(),
            WindowEvent::ThemeChanged(_) => {
                if self.config.appearance.theme == ThemeMode::Auto {
                    self.apply_theme();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = input::modifiers(m.state()),
            WindowEvent::CursorMoved { position, .. } => {
                let p = Vec2::new(position.x as f32, position.y as f32) / self.scale();
                self.pointer_moved(p);
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
                self.ui.handle(UiInput::PointerLeave);
                self.leave_page();
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x, y) * LINE_SCROLL,
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32) / self.scale(),
                };
                if self.overview.is_open() {
                    self.overview.scroll_by(&mut self.ui, delta.y);
                } else if let (Some(p), Some(view)) = (self.pointer, self.active_view())
                    && !self.ui.covers(p)
                    && self.page_rect().contains(p)
                {
                    let pos = p - self.page_rect().origin;
                    let modifiers = input::engine_modifiers(self.modifiers);
                    self.engine.send_input(view, &InputEvent::Wheel { pos, delta, modifiers });
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                if self.keyboard(&event) {
                    return Ok(false);
                }
            }
            WindowEvent::Focused(focused) => {
                if let Some(view) = self.active_view()
                    && self.ui.focused().is_none()
                {
                    self.engine.set_focus(view, focused);
                }
            }
            WindowEvent::RedrawRequested => self.redraw()?,
            _ => {}
        }
        Ok(true)
    }

    fn pointer_moved(&mut self, p: Vec2) {
        self.pointer = Some(p);
        let (over_ui, events) = self.ui.handle(UiInput::PointerMove(p));
        self.handle_ui_events(events);
        if over_ui || self.pointer_captured_by_ui {
            self.leave_page();
        } else if let Some(view) = self.active_view() {
            let pos = p - self.page_rect().origin;
            let modifiers = input::engine_modifiers(self.modifiers);
            self.engine.send_input(view, &InputEvent::MouseMove { pos, modifiers });
            self.pointer_on_page = true;
        }
        self.update_cursor();
    }

    fn leave_page(&mut self) {
        if self.pointer_on_page {
            self.pointer_on_page = false;
            if let Some(view) = self.active_view() {
                let modifiers = input::engine_modifiers(self.modifiers);
                self.engine.send_input(view, &InputEvent::MouseLeave { modifiers });
            }
        }
    }

    fn update_cursor(&self) {
        let icon = match self.ui.cursor() {
            Some(UiCursor::Pointer) => Some(winit::window::CursorIcon::Pointer),
            Some(UiCursor::Text) => Some(winit::window::CursorIcon::Text),
            Some(UiCursor::Default) => Some(winit::window::CursorIcon::Default),
            None => input::cursor_icon(self.page_cursor),
        };
        self.window.set_cursor_visible(icon.is_some());
        if let Some(icon) = icon {
            self.window.set_cursor(icon);
        }
    }

    fn mouse_button(&mut self, state: ElementState, button: winit::event::MouseButton) {
        let Some(p) = self.pointer else { return };
        let pressed = state.is_pressed();
        let view = self.active_view();
        match button {
            winit::event::MouseButton::Back if pressed => return self.run(Action::Back),
            winit::event::MouseButton::Forward if pressed => return self.run(Action::Forward),
            _ => {}
        }
        let page_button = match button {
            winit::event::MouseButton::Left => MouseButton::Left,
            winit::event::MouseButton::Middle => MouseButton::Middle,
            winit::event::MouseButton::Right => MouseButton::Right,
            _ => return,
        };

        if button == winit::event::MouseButton::Left {
            if pressed {
                let (consumed, events) = self.ui.handle(UiInput::PointerDown);
                self.pointer_captured_by_ui = consumed;
                self.handle_ui_events(events);
                if consumed {
                    return;
                }
            } else {
                let captured = std::mem::take(&mut self.pointer_captured_by_ui);
                let (_, events) = self.ui.handle(UiInput::PointerUp);
                self.handle_ui_events(events);
                if captured {
                    return;
                }
            }
        } else if self.ui.covers(p) {
            return;
        }

        let Some(view) = view else { return };
        if !self.page_rect().contains(p) && pressed {
            return;
        }
        if pressed && self.chrome.dismiss_overlay() {
            // The first click on the page just closes the floating sidebar.
            self.update_layout();
            return;
        }
        let click_count = if pressed { self.clicks.press(button, p) } else { self.clicks.current() };
        if pressed {
            self.engine.set_focus(view, true);
        }
        let pos = p - self.page_rect().origin;
        let modifiers = input::engine_modifiers(self.modifiers);
        self.engine.send_input(
            view,
            &InputEvent::MouseButton { pos, button: page_button, pressed, click_count, modifiers },
        );
    }

    /// Returns `true` if the browser should quit.
    fn keyboard(&mut self, event: &winit::event::KeyEvent) -> bool {
        let pressed = event.state.is_pressed();
        if pressed
            && let Some(chord) = input::chord(event, self.modifiers)
            && let Some(action) = self.keymap.action_for(&chord)
        {
            if action == Action::Quit {
                return true;
            }
            self.run(action);
            return false;
        }

        if self.overview.is_open() && self.ui.focused().is_none() {
            // The overview owns the keyboard; Escape closes it, nothing reaches the page.
            if pressed && input::key(event) == Some(Key::Named("Escape".into())) {
                self.close_overview(self.tabs.active_index());
            }
            return false;
        }

        if pressed
            && self.ui.focused() == Some(self.chrome.address_field)
            && let Some(Key::Named(name)) = input::key(event)
            && (name == "Up" || name == "Down")
        {
            self.suggestions.move_selection(&mut self.ui, if name == "Up" { -1 } else { 1 });
            return false;
        }

        if self.ui.focused().is_some() {
            if pressed {
                if let Some(key) = input::key(event) {
                    let is_text = matches!(key, Key::Char(_)) && !self.modifiers.ctrl && !self.modifiers.logo;
                    if !is_text {
                        let (_, events) = self.ui.handle(UiInput::Key { key, modifiers: self.modifiers });
                        self.handle_ui_events(events);
                    }
                }
                if !self.modifiers.ctrl
                    && !self.modifiers.logo
                    && let Some(text) = event.text.as_ref().filter(|t| !t.chars().all(char::is_control))
                {
                    let (_, events) = self.ui.handle(UiInput::Text(text.to_string()));
                    self.handle_ui_events(events);
                }
            }
            return false;
        }

        if let Some(view) = self.active_view() {
            if let Some(key) = input::page_key(event, self.modifiers) {
                self.engine.send_input(view, &key);
            }
            if let Some(text) = input::page_text(event, self.modifiers) {
                self.engine.send_input(view, &text);
            }
        }
        false
    }

    // ---- drawing ----

    fn redraw(&mut self) -> Result<()> {
        if self.logical_size() != self.layout_size {
            self.update_layout();
        }
        if std::mem::take(&mut self.tabs_changed) {
            self.chrome.sync_tabs(&mut self.ui, &self.tabs, &self.favicons);
            let title = self.tabs.active().map(|t| t.label()).unwrap_or_default();
            self.window.set_title(
                if title.is_empty() { "Axion".to_owned() } else { format!("{title} — Axion") }.as_str(),
            );
        }
        let now = Instant::now();
        if self.overview_at.is_some_and(|at| now >= at) {
            self.overview_at = None;
            self.open_overview();
        }
        if let Some((_, text)) = self.address_at.take_if(|(at, _)| now >= *at) {
            self.edit_address();
            self.ui.set_field_text(self.chrome.address_field, &text);
        }
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if self.overview.is_open() {
            let query = self.ui.field_text(self.overview.search_field()).unwrap_or_default().to_owned();
            if query != self.overview_query {
                self.overview_query = query;
                self.overview.rebuild(&mut self.ui, &self.tabs, &self.favicons);
            }
            let engine = &self.engine;
            self.overview.update(&mut self.ui, dt, |view| {
                let frame = engine.frame(view)?;
                let size = (frame.texture.width(), frame.texture.height());
                Some((frame.texture.create_view(&Default::default()), size))
            });
        }
        self.update_suggestions();
        self.ui.layout();
        if let Some(view) = self.active_view() {
            self.engine.request_frame(view);
        }
        self.snapshot_background_tabs();

        let surface = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Validation => bail!("surface validation error"),
        };
        let output = surface.texture.create_view(&Default::default());

        let frame = self.active_view().and_then(|v| self.engine.frame(v));
        let page_view = frame.as_ref().map(|f| f.texture.create_view(&Default::default()));
        let s = self.scale();
        let page = self.page_rect();
        if let (Some(view), Some(frame)) = (&page_view, &frame) {
            self.adapt_appearance(view, &frame.texture);
        }
        let page_px = Rect { origin: page.origin * s, size: page.size * s };

        let theme = self.ui.theme();
        let mut layers = Vec::new();
        // The page ends where its rectangle ends (below the bar, beside a docked sidebar), with
        // rounded corners; square in page fullscreen (video).
        if let Some(view) = &page_view {
            let mut page = ImageDraw::new(view, page_px);
            if !self.chrome.page_fullscreen() {
                page.radius = self.config.layout.page_corner_radius * s;
            }
            layers.push(Layer::Image(page));
        }
        self.ui.paint(&mut layers);
        if tracing::enabled!(tracing::Level::TRACE) {
            let kinds: Vec<String> = layers
                .iter()
                .map(|l| match l {
                    Layer::Image(_) => "image".to_owned(),
                    Layer::Glass { shapes, params } => {
                        format!("glass×{}(blur {:.0})", shapes.len(), params.blur_radius)
                    }
                    Layer::Content(c) => {
                        format!("content(q{} i{} t{})", c.quads.len(), c.images.len(), c.text.len())
                    }
                })
                .collect();
            tracing::trace!(?kinds, "layers");
        }
        let clear = self.page_tint.filter(|_| !self.overview.is_open()).unwrap_or(theme.color(Paint::Ground));
        let scene = Scene { clear, layers };
        let size = (self.surface_config.width, self.surface_config.height);
        self.compositor.render(&scene, &mut self.ui.fonts(), &output, size);
        drop(scene);
        self.window.pre_present_notify();
        self.queue.present(surface);

        if let Some((path, at)) = &self.capture
            && Instant::now() >= *at
        {
            let (w, h, pixels) = self.compositor.read_back().context("reading back the frame")?;
            image::RgbaImage::from_raw(w, h, pixels)
                .context("frame has the wrong size")?
                .save(path)
                .with_context(|| format!("saving {}", path.display()))?;
            tracing::info!(path = %path.display(), "saved screenshot");
            bail!(CaptureDone);
        }
        Ok(())
    }

    /// Shows suggestions while the address field is being edited and differs from the page.
    fn update_suggestions(&mut self) {
        let editing = self.ui.focused() == Some(self.chrome.address_field) && !self.overview.is_open();
        let text = self.ui.field_text(self.chrome.address_field).unwrap_or_default().to_owned();
        let typed = editing && self.tabs.active().is_none_or(|t| t.url != text);
        if !typed {
            if self.suggestions.is_visible() {
                self.suggestions.hide(&mut self.ui);
            }
            return;
        }
        let anchor = self.chrome.address_frame(&self.ui);
        let (store, search_url) = (self.store.as_ref(), self.config.general.search_url.clone());
        self.suggestions
            .update(&mut self.ui, &text, anchor, || suggest(&text, store, &search_url, unix_now()));
        self.suggestions.reposition(&mut self.ui, anchor);
    }

    /// Background tabs render until loaded and painted once, then get hidden (throttled).
    fn snapshot_background_tabs(&mut self) {
        let active = self.active_view();
        for tab in self.tabs.all_mut() {
            if !tab.needs_snapshot || Some(tab.view) == active {
                tab.needs_snapshot = false;
                continue;
            }
            if !tab.loading && self.engine.frame(tab.view).is_some() {
                tab.needs_snapshot = false;
                self.engine.set_hidden(tab.view, true);
            } else {
                self.engine.request_frame(tab.view);
            }
        }
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }
}

/// Signals that `--capture` finished (not an error for the user).
#[derive(Debug)]
pub struct CaptureDone;

impl std::fmt::Display for CaptureDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("capture done")
    }
}

impl std::error::Error for CaptureDone {}

/// Moves the window onto the monitor named by `AXION_MONITOR` (a substring of its name, e.g.
/// "HDMI-A-1"). Only X11 lets applications place windows; on Wayland the compositor decides.
fn place_on_monitor(event_loop: &ActiveEventLoop, window: &Window) {
    let Ok(wanted) = std::env::var("AXION_MONITOR") else { return };
    let monitor = event_loop
        .available_monitors()
        .find(|m| m.name().is_some_and(|n| n.to_lowercase().contains(&wanted.to_lowercase())));
    let Some(monitor) = monitor else {
        let names: Vec<String> = event_loop.available_monitors().filter_map(|m| m.name()).collect();
        tracing::warn!(%wanted, ?names, "AXION_MONITOR matches no monitor");
        return;
    };
    let position = monitor.position();
    let size = monitor.size();
    let window_size = window.outer_size();
    let x = position.x + (size.width.saturating_sub(window_size.width) / 2) as i32;
    let y = position.y + (size.height.saturating_sub(window_size.height) / 2) as i32;
    window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
}

fn load_keymap(paths: &ConfigPaths) -> Keymap {
    let keymap = axion_config::load_keymap(paths).unwrap_or_else(|e| {
        tracing::error!("{e}; using the default shortcuts");
        Keymap::default()
    });
    for c in keymap.conflicts() {
        tracing::warn!("{} is bound to both {} and {}; using {}", c.chord, c.kept, c.ignored, c.kept);
    }
    keymap
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn is_dark(config: &Config, window: &Window) -> bool {
    match config.appearance.theme {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::Auto => window.theme() == Some(winit::window::Theme::Dark),
    }
}

/// `~/.local/share/axion/profile` (or `AXION_PROFILE_DIR`).
fn profile_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("AXION_PROFILE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let dirs = directories::ProjectDirs::from("", "", "axion").context("no home directory")?;
    Ok(dirs.data_dir().join("profile"))
}
