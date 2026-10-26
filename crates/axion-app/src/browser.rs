//! One browser window: owns the GPU surface, the engine, the UI and the tabs, and routes input
//! between the chrome and the page.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axion_accounts::{Accounts, Bases};
use axion_compositor::Icon;
use axion_compositor::{Compositor, ImageDraw, Layer, Rect, Scene};
use axion_config::{
    Action, Config, ConfigPaths, ConfigWatcher, GlassStyle, Key, Keymap, Modifiers, SidebarSlideResize,
    TabBarStyle, ThemeMode, UiLanguage,
};
use axion_engine::cef::{CefEngine, CefOptions};
use axion_engine::{
    CursorKind, DevToolsOptions, DevToolsPanel, Engine, EngineEvent, InputEvent, MouseButton, NewViewKind,
    ViewId, ViewSize,
};
use axion_i18n::{tr, tr_fmt, tr_mark};
use axion_shields::Level;
use axion_tabs::{Lifecycle, MemoryMonitor, TabInfo, Transition};
use axion_ui::{Paint, Theme, Ui, UiCursor, UiEvent, UiInput};
use glam::Vec2;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window};

use crate::accounts::{AddAccountSheet, Relaunch, SheetAction};
use crate::avatars::{Avatars, PhotoChooser, PhotoFor};
use crate::chrome::{Chrome, ChromeAction, MoveKey, NowPlaying, PanelItem, Section, Surface, TabItem};
use crate::context_menu::ContextMenus;
use crate::desktop::DesktopTheme;
use crate::downloads::{self, Downloads, List};
use crate::downloads_popover::{DownloadsAction, DownloadsPopover};
use crate::favicons::{FETCH_SIZE, Favicons};
use crate::filedrop::{DropOutcome, FileDrop};
use crate::find::{FindAction, FindBar};
use crate::frame_rate::{self, RefreshWatch};
use crate::home::{HomeAction, HomePage, REFRESH_SCRIPT, is_home_url};
use crate::import::Importer;
use crate::input::{self, ClickCounter};
use crate::lifecycle;
use crate::lockscreen::LockScreen;
use crate::menu::{
    ACCOUNT_MENU_BASE, MAX_ITEMS, Menu, MenuAction, MenuClick, MenuControl, MenuItem, PAGE_MENU_BASE,
    Placement,
};
use crate::morph::Origin;
use crate::mpris::{Mpris, MprisCommand, Track};
use crate::now_playing::{Art, ArtStore, first_web_url};
use crate::onboarding::Onboarding;
use crate::overview::{Overview, OverviewAction};
use crate::page_color::{PageTint, TintParams, lightness};
use crate::passwords::{Outcome as PasswordsOutcome, Passwords};
use crate::perf_log::{FrameOutcome, PerfLog};
use crate::pip::{PipControl, PipOutcome, PipWindow};
use crate::platform::clipboard::ClipboardReader;
use crate::platform::pointer::PointerProbe;
use crate::player::{
    Context as PlayerContext, Control as PlayerControl, Mode as PlayerMode, Outcome as PlayerOutcome,
    PlayerOverlay,
};
use crate::private::{PrivateLock, TabDrop};
use crate::resize_bench::{self, FrameTimes, Recorder, ResizeBench, ResizeCounters};
use crate::settings::{AccountInfo, Category, Outcome, SettingsAction, SettingsSheet};
use crate::shields::ShieldsState;
use crate::shields_popover::{PopoverAction, ShieldsInfo, ShieldsPopover};
use crate::suggestions::{SuggestionList, suggest};
use crate::tabdrag::{TabDrag, Target};
use crate::tabs::{Tab, Tabs, ZOOM_STEP};
use crate::toast::Toast;
use crate::tooltip::Tooltip;
use crate::typing_probe;
use crate::update::{InstallMode, Notice, Phase, Updater};
use crate::update_alert::{AlertAction, AlertView, Tone, UpdateAlert};
use crate::url::display_host;
use crate::url::resolve_input;
use crate::window_resize::{Debounce, ResizePacer};

mod context;
mod import;
mod onboarding;
mod page_context;
mod permissions;
mod playlist;

const DEFAULT_WINDOW_SIZE: (f64, f64) = (1440.0, 900.0);
/// `AXION_RESIZE_LOG`: a resize's timings are logged once no resize came for this long.
const RESIZE_LOG_QUIET: Duration = Duration::from_millis(500);
/// How long before a `--capture --overview` screenshot the overview opens.
const OVERVIEW_CAPTURE_LEAD: Duration = Duration::from_secs(2);
/// Settings changes are written to disk once they've been still for this long.
const CONFIG_SAVE_DELAY: Duration = Duration::from_millis(250);
/// Saving the open tabs waits until they stop changing for this long.
/// Frame times, logged at debug level every few seconds (`RUST_LOG=axion::browser=debug`).
#[derive(Default)]
struct FrameStats {
    count: u32,
    total: Duration,
    worst: Duration,
    since: Option<Instant>,
    first_serial: Option<u64>,
    /// Time spent applying engine events (`Browser::pump`, without Chromium's own work).
    events: u32,
    events_total: Duration,
    events_worst: Duration,
}

impl FrameStats {
    fn record(&mut self, took: Duration, page_serial: u64) {
        let since = *self.since.get_or_insert_with(Instant::now);
        let first = *self.first_serial.get_or_insert(page_serial);
        self.count += 1;
        self.total += took;
        self.worst = self.worst.max(took);
        if since.elapsed() >= Duration::from_secs(2) {
            tracing::debug!(
                fps = self.count as f32 / since.elapsed().as_secs_f32(),
                avg_ms = self.total.as_secs_f32() * 1000.0 / self.count.max(1) as f32,
                worst_ms = self.worst.as_secs_f32() * 1000.0,
                page_fps = page_serial.saturating_sub(first) as f32 / since.elapsed().as_secs_f32(),
                events_per_s = self.events as f32 / since.elapsed().as_secs_f32(),
                events_ms_per_s = self.events_total.as_secs_f32() * 1000.0 / since.elapsed().as_secs_f32(),
                events_worst_ms = self.events_worst.as_secs_f32() * 1000.0,
                "frames"
            );
            *self = Self::default();
        }
    }
}

/// A running typing probe.
struct ProbeRun {
    spec: typing_probe::Spec,
    started: Instant,
    tracker: typing_probe::Tracker,
    baseline: Option<Vec<u8>>,
    last_serial: u64,
    reported: bool,
}

impl ProbeRun {
    fn new(spec: typing_probe::Spec) -> Self {
        Self {
            spec,
            started: Instant::now(),
            tracker: Default::default(),
            baseline: None,
            last_serial: 0,
            reported: false,
        }
    }

    fn char_due(&self, now: Instant) -> Option<char> {
        let i = self.tracker.sent_count();
        let at = self.started + self.spec.start_after + self.spec.interval * i as u32;
        self.spec.text.chars().nth(i).filter(|_| now >= at)
    }
}

/// A click into a site player that should start it.
struct SiteClick {
    view: ViewId,
    player: String,
    tries: u32,
    at: Instant,
}

/// How long a click into a site player gets to start it before we click again.
const SITE_CLICK_RETRY: Duration = Duration::from_millis(1200);
const SITE_CLICK_TRIES: u32 = 3;
const DAY_SECONDS: i64 = 24 * 60 * 60;

/// While the page's frame is smaller than its rectangle, the column this far (logical px)
/// from the frame's right edge fills the rest: just left of a classic scrollbar.
const EDGE_SAMPLE_INSET: f32 = 20.0;

/// Time between the steps of a capture's `--open=a,b`.
const OPEN_STEP_INTERVAL: Duration = Duration::from_millis(2500);
const SESSION_SAVE_DELAY: Duration = Duration::from_secs(2);
/// How many of the saved session's tabs are pinned (they come first), in the profile's meta.
const SESSION_PINNED_KEY: &str = "session_pinned_tabs";
/// History rows shown in the sidebar panel.
const PANEL_HISTORY_LIMIT: usize = 300;
/// Most bookmarks a sidebar search lists.
const PANEL_SEARCH_LIMIT: usize = 1000;
/// The History panel's clear button, and what it says while asking to confirm.
const CLEAR_HISTORY: &str = tr_mark("Clear History…");
const CLEAR_HISTORY_CONFIRM: &str = tr_mark("Click Again to Clear");
/// Chromium's zoom levels are powers of this factor.
const ZOOM_BASE: f64 = 1.2;
/// Pixels per wheel notch.
const LINE_SCROLL: f32 = 100.0;
/// Slide anchors (see `Browser::slide_anchor`): the frame's right edge or its middle stays on
/// the page rectangle's.
const SLIDE_ANCHOR_STILL: f32 = 1.0;
const SLIDE_ANCHOR_CENTRED: f32 = 0.5;

/// How fast the measured delay of a live-resized page's frames follows each new measurement.
const PAGE_LAG_SMOOTHING: f32 = 0.25;
/// Opacity below which the page's frame from before a sidebar slide is gone (see `slide_fade`).
const SLIDE_FADE_DONE: f32 = 0.01;
/// The chrome turns dark on pages whose colour is darker than this.
const DARK_BELOW: f32 = 0.5;

pub struct Options {
    /// Tabs to open; the first one is active. Empty opens the homepage.
    pub urls: Vec<String>,
    /// Open the tab overview shortly before `--capture` (for screenshots of it).
    pub overview: bool,
    /// Type this into the address field shortly before `--capture` (for screenshots of it).
    pub address: Option<String>,
    /// Open this UI (`menu`, `history`, `bookmarks`) shortly before `--capture`.
    pub open: Option<String>,
    /// How long before the capture `--open`/`--address`/`--overview` happen (default 2 s);
    /// short leads catch animations mid-way.
    pub lead: Option<Duration>,
    /// Save a screenshot here after `capture_after`, then quit.
    pub capture: Option<PathBuf>,
    pub capture_after: Duration,
    /// Initial window size in logical pixels.
    pub size: Option<(f64, f64)>,
    /// Initial window position (physical px; X11 only), and whether it starts maximized.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    /// The account to open (`--account=ID`); the last one used otherwise.
    pub account: Option<String>,
    /// Show the first-run introduction (`--onboarding`), whatever the profile says.
    pub onboarding: bool,
    /// Run the scripted resize benchmark (`--bench-resize`, see `resize_bench`), then quit.
    pub bench_resize: bool,
    /// Wakes the event loop when the engine has work.
    pub waker: Option<axion_engine::cef::Waker>,
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
    menu: Menu,
    shields: ShieldsState,
    shields_popover: ShieldsPopover,
    downloads_popover: DownloadsPopover,
    /// Site permissions: the prompt, the recording indicator's popover and the decisions.
    permissions: permissions::PermissionsUi,
    toast: Toast,
    updater: Updater,
    update_alert: UpdateAlert,
    /// The graphics settings' confirmation (shared GPU memory on AMD) is showing in the alert.
    gpu_confirm: bool,
    /// What the GPU session actually uses (Settings › Hardware, diagnostics).
    hardware_status: crate::hardware::Status,
    /// When the first automatic update check runs (a moment after start; none when not due).
    update_check_at: Option<Instant>,
    /// Where the install stood when the About page was last drawn.
    update_phase: Phase,
    /// The app icon as a texture for the About page (made when first needed).
    about_icon: Option<crate::home::Image>,
    tooltip: Tooltip,
    /// What the sidebar panel's search field held when the list was last built.
    panel_query: String,
    find: FindBar,
    /// Files dragged over the window from the desktop.
    file_drop: FileDrop,
    /// Asks the X server where the pointer is during such a drag (made on first use).
    pointer_probe: Option<PointerProbe>,
    /// Test aid: where a simulated file drag is (window, logical px), instead of the pointer.
    file_drag_at: Option<Vec2>,
    /// Clipboard reads for pastes into pages (where the engine can't read it itself).
    clipboard_reader: ClipboardReader,
    /// Pastes into the chrome's text fields (the address, searches), read off the main thread
    /// like the page's: the app owning the clipboard may take long to send it.
    field_paste: ClipboardReader<axion_ui::taffy::NodeId>,
    player: PlayerOverlay,
    /// The Playlist: its videos, and its page, popover and thumbnails.
    playlist: crate::playlist::Playlist,
    playlist_ui: playlist::PlaylistUi,
    /// The active page's last reported scroll position (CSS px).
    /// Frames saved so far by the `AXION_CAPTURE_FRAMES` debug aid.
    capture_frame: u32,
    /// The next frame is audited for glass and glass look-alikes (`flat-audit` capture aid).
    flat_audit: bool,
    /// Typing latency probe (`AXION_TYPE_PROBE`, see `typing_probe`).
    probe: Option<ProbeRun>,
    /// Those frames, kept in memory until the sequence is complete (saving each one at once
    /// would stall the frame loop and change the timing being looked at).
    captured_frames: Vec<image::RgbaImage>,
    /// Picture in Picture (made with the first one; its window only exists while PiP is on).
    pip: Option<PipWindow>,
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    site_click: Option<SiteClick>,
    frame_stats: FrameStats,
    /// The tab whose media the Now Playing card and the desktop (MPRIS) show.
    now_playing: Option<ViewId>,
    /// The Now Playing card's artwork.
    now_art: ArtStore,
    mpris: Option<Mpris>,
    memory: MemoryMonitor,
    lifecycle_checked: Instant,
    session_dirty: bool,
    session_changed: Instant,
    settings: SettingsSheet,
    passwords: Passwords,
    /// Unsaved settings changes since this time (saving is throttled while dragging).
    config_dirty_since: Option<Instant>,
    /// What we last wrote to config.toml, so the file watcher can ignore our own writes.
    last_saved_config: Option<String>,
    /// URLs of the open sidebar panel's rows, by index.
    panel_urls: Vec<String>,
    /// The open sidebar panel, and whether its clear button asks for confirmation.
    panel: Option<(Panel, bool)>,
    favicons: Favicons,
    downloads: Downloads,
    /// Importing from other browsers into this account's store.
    importer: Importer,
    /// History and bookmarks; `None` if the database couldn't be opened (browsing still works).
    store: Option<axion_store::Store>,
    /// Search text the overview's cards were last built for.
    overview_query: String,
    last_frame: Instant,
    tabs: Tabs,
    /// The other tab group (normal tabs while in private mode, and vice versa).
    parked: Tabs,
    private_mode: bool,
    /// The accounts file and the account this window shows.
    accounts: Accounts,
    account_id: String,
    account_menu: Menu,
    add_account: AddAccountSheet,
    /// The first-run introduction (see `crate::onboarding`), and the interface language this
    /// run started in (a different one at the end relaunches Axion).
    onboarding: Onboarding,
    start_language: axion_i18n::Language,
    /// Profile pictures as textures, and the file chooser to pick one.
    avatars: Avatars,
    photo_chooser: PhotoChooser,
    /// The picture chosen in the Add Account sheet (already cropped and scaled).
    new_account_photo: Option<image::RgbaImage>,
    /// Set when switching accounts: the window closes and Axion starts again in the other one.
    relaunch: Option<Relaunch>,
    /// Whether the account (its vault) was unlocked last time we looked; private tabs follow.
    account_unlocked: bool,
    lock: PrivateLock,
    lock_screen: LockScreen,
    /// Whether the lock screen is up, and where (to only relayout on change).
    lock_shown: Option<Rect>,
    /// A normal tab dropped into the locked private tabs: it moves there (to this index) once
    /// the account password is entered.
    pending_move: Option<(ViewId, usize)>,
    /// The plane the sidebar is drawn before (its place while nothing is locked).
    sidebar_before: Option<axion_ui::taffy::NodeId>,
    tab_drag: TabDrag,
    /// Right-click menus of tabs and bookmarks, and the tab (view, private) one was opened for.
    context_menus: ContextMenus,
    context_tab: Option<(ViewId, bool)>,
    /// What the open page menu (right click inside the page) is about.
    page_context: Option<page_context::PageContext>,
    /// The view whose "Copy Image" is waiting for its picture.
    copy_image: Option<ViewId>,
    /// The tab row the open context menu flows out of (view, private, surface).
    context_row: Option<(ViewId, bool, crate::chrome::Surface)>,
    /// The bookmarks bar's bookmarks (id, url, title), reloaded when `bookmarks_changed`.
    bar_bookmarks: Vec<(i64, String, String)>,
    bookmarks_changed: bool,
    /// What the overview lists while private tabs are locked: nothing.
    no_tabs: Tabs,
    config: Config,
    keymap: Keymap,
    paths: ConfigPaths,
    watcher: Option<ConfigWatcher>,
    dark: bool,
    /// The desktop's light/dark preference (for `theme = "auto"`) and the last value seen.
    desktop_theme: DesktopTheme,
    desktop_dark: Option<bool>,
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
    /// The window colour around the page (`appearance.tint_from_page`), springing between
    /// the pages' colours.
    window_tint: Option<[f32; 4]>,
    last_tint_step: Instant,
    /// Each page's own colour (theme colour or measured once per load).
    page_colors: PageTint,
    /// The start page (`axion://home/`).
    home: HomePage,
    /// The Playlist's media view page (`axion://media/`) and its sources.
    media_service: Arc<crate::media_view::MediaService>,
    /// Settings has asked for the start page backgrounds' thumbnails (textures are made from
    /// then on), and the backgrounds generation it last listed.
    home_thumbnails: bool,
    backgrounds_listed: Option<u64>,
    /// When to open the overview for a `--capture --overview` screenshot.
    overview_at: Option<Instant>,
    address_at: Option<(Instant, String)>,
    open_at: Option<(Instant, String)>,
    /// Where the next Settings or Passwords sheet comes out of (taken when it opens).
    sheet_origin: Option<Origin>,
    /// Search engine icons being fetched for the start page (once per run).
    engine_icons: Option<std::sync::mpsc::Receiver<crate::engine_icons::EngineIcon>>,
    /// Icons of the saved logins' sites being fetched for the Passwords list.
    password_icons: Option<std::sync::mpsc::Receiver<crate::engine_icons::EngineIcon>>,
    /// SVG site icons being rewritten for the chrome's colour scheme.
    svg_icons: crate::svg_icons::SvgIcons,
    /// The page menu row clicked last (where a sheet it opens comes out of).
    menu_click_frame: Option<Rect>,
    /// Capture aid "sweep:X1:Y1:X2:Y2:N": the pointer moves from one point to the other in N
    /// steps, one per frame (start, end, steps, steps done).
    sweep: Option<(Vec2, Vec2, u32, u32)>,
    /// Capture aid "wheel:N": the page gets a wheel event at its centre every frame for N
    /// frames, down then back up (frames left), to measure scrolling (input → page frame).
    wheel_frames: u32,
    /// While the sidebar slides: the size the shown view was given for it (its size once the
    /// slide settles), and a copy of its frame from before the slide (see `redraw_frame`).
    slide_view_size: Option<(ViewId, Vec2)>,
    slide_snapshot: Option<wgpu::Texture>,
    /// Once the page's frame at its new size took over from its wider frame from before an
    /// opening slide: that old layout, fading out over the new one.
    slide_fade: Option<(wgpu::Texture, axion_physics::Animated<f32>)>,
    /// `sidebar_slide_resize = "live"`: the width the shown page was last resized to.
    slide_live_width: Option<f32>,
    /// A live-resized page is drawn stretched over the rectangle until its frames have caught
    /// up with the end of the slide.
    live_stretch: bool,
    /// Live resizing: the width (logical) the rectangle had in the previous frame, the sizes
    /// the page was asked for (when, width in pixels), the width of the latest frame seen, and
    /// the measured delay between a resize and its frame (seconds).
    slide_prev_width: Option<f32>,
    slide_sent: Vec<(Instant, f32)>,
    slide_seen_width: Option<f32>,
    page_lag: Option<f32>,
    /// `--bench-resize`: the scripted run; `AXION_RESIZE_LOG=1`: frame timings of each real
    /// resize (logged once it has been still for `RESIZE_LOG_QUIET`), and the counters both use.
    resize_bench: Option<ResizeBench>,
    /// What the window shows (see `ui.skip_unchanged_frames`): the presented scene's
    /// fingerprint and its textures (held so no new view takes their identity), when the next
    /// frame is due after a skipped one (no present waited for the display), the display's
    /// frame interval, and the frames skipped so far.
    shown_fingerprint: Option<u64>,
    shown_textures: Vec<wgpu::TextureView>,
    /// The shown page frame's texture and a view of it, kept while the texture is the same.
    page_view: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// The shown scene's layer fingerprints (trace logging only).
    shown_layers: Vec<u64>,
    idle_redraw_at: Option<Instant>,
    frame_interval: Duration,
    /// Notices the window's monitor's refresh rate changing (see `frame_rate`).
    refresh_watch: RefreshWatch,
    frames_skipped: u64,
    frames_presented: u64,
    /// The engine's page frame count at the last pump.
    pumped_page_frames: u64,
    /// The window's newest size, applied by the next frame (see `apply_window_size`).
    pending_window_size: Option<winit::dpi::PhysicalSize<u32>>,
    /// The shown view's resizes, one at a time, and the background tabs' once the window is
    /// still (see `pace_page_resizes`).
    page_pacer: ResizePacer,
    paced_view: Option<ViewId>,
    background_resize: Debounce,
    resize_log: Option<(Recorder, Instant)>,
    resize_log_enabled: bool,
    resize_counters: ResizeCounters,
    frame_times: FrameTimes,
    /// `AXION_PERF_LOG=1`: a line per second about frames, pages, GPU and CPU (see `perf_log`).
    perf_log: Option<PerfLog>,
    /// The swapchain's present mode as configured, for the perf log.
    present_mode: String,
}

impl Browser {
    pub fn new(event_loop: &ActiveEventLoop, options: Options) -> Result<Self> {
        let bases = Bases::discover()?;
        let accounts_file = bases.config.join(axion_accounts::ACCOUNTS_FILE).exists();
        let mut accounts = Accounts::load(bases, unix_now())?;
        let account_id = accounts.select(options.account.as_deref())?;
        let account_paths = accounts.paths(&account_id)?;
        let account_name = accounts.get(&account_id).map(|a| a.name.clone()).unwrap_or_default();
        tracing::info!(account = %account_id, profile = %account_paths.profile.display(), "opening account");
        let paths = ConfigPaths::in_dir(account_paths.config.clone());
        let config_file = paths.config.exists();
        let mut config = axion_config::load_config(&paths).unwrap_or_else(|e| {
            tracing::error!("{e}; using defaults");
            Config::default()
        });
        crate::locale::apply(config.general.language);
        let start_language = axion_i18n::language();
        let keymap = load_keymap(&paths);
        let watcher = ConfigWatcher::new(&paths)
            .inspect_err(|e| tracing::warn!("live config reload unavailable: {e}"))
            .ok();

        // Wayland shows the icon of `axion.desktop` (matched by the app id below): make sure a
        // current one exists before the window does (not for capture runs: they are test runs).
        crate::desktop_integration::ensure(config.general.desktop_integration && options.capture.is_none());
        let attributes = Window::default_attributes()
            .with_title("Axion")
            .with_window_icon(crate::app_icon::window_icon())
            .with_inner_size(winit::dpi::LogicalSize::<f64>::from(
                options.size.unwrap_or(DEFAULT_WINDOW_SIZE),
            ))
            .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 360.0))
            .with_maximized(options.maximized);
        // Wayland app id / X11 class: matches `axion.desktop`, so the shell shows our icon.
        #[cfg(target_os = "linux")]
        let attributes = {
            use winit::platform::wayland::WindowAttributesExtWayland as _;
            attributes.with_name("axion", "axion")
        };
        let window = Arc::new(event_loop.create_window(attributes)?);

        match options.position {
            Some((x, y)) => window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y)),
            None => place_on_monitor(event_loop, &window),
        }
        let hardware_env = crate::hardware::Overrides::from_env();
        let adapter_choice = crate::hardware::adapter_choice(&config.hardware, &hardware_env);
        let (instance, surface, adapter, device, queue) =
            crate::gpu::init(event_loop, &window, &adapter_choice)?;
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
        // `AutoVsync` is what wgpu turns into FifoRelaxed where the surface has it, else Fifo.
        let present_mode = match surface_config.present_mode {
            wgpu::PresentMode::AutoVsync if caps.present_modes.contains(&wgpu::PresentMode::FifoRelaxed) => {
                "FifoRelaxed".to_owned()
            }
            wgpu::PresentMode::AutoVsync => "Fifo".to_owned(),
            mode => format!("{mode:?}"),
        };
        let mut perf_log = PerfLog::from_env();
        if let Some(log) = &mut perf_log {
            log.enable_gpu_timer(&device, &queue);
            tracing::info!(modes = ?caps.present_modes, chosen = %present_mode, "perf log on: present modes");
        }

        let info = adapter.get_info();
        let can_import = adapter.features().contains(axion_engine::cef::required_features());
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        // Zero-copy import is only verified on NVIDIA's driver so far. On RADV (AMD) it
        // currently triggers a GPU memory fault and context loss, so other drivers use
        // software paint until that is understood (see plan.md, Phase 0 follow-ups).
        // Under X11/XWayland Chromium has no GBM device for shared buffers at all.
        // `hardware::frame_mode` holds the rule (and the setting and AXION_OSR on top).
        let (frame_mode, frames_source) = crate::hardware::frame_mode(
            &config.hardware,
            &hardware_env,
            crate::hardware::Session { vendor: info.vendor, wayland, can_import },
        );
        let accelerated = frame_mode == crate::hardware::FrameMode::Gpu;
        let hardware_status = crate::hardware::Status {
            adapters: crate::gpu::adapters(&instance),
            in_use: crate::hardware::AdapterLine::of(&info),
            frames: frame_mode,
            frames_source,
            video_decoding: crate::hardware::video_decoding(&config),
            cef_version: axion_engine::cef::cef_version(),
            wayland,
            env: hardware_env,
            startup: crate::hardware::Startup::of(&config),
        };
        if !accelerated {
            tracing::info!(driver = %info.driver, "using software paint for pages (set AXION_OSR=gpu to force GPU sharing)");
        }
        let profile = account_paths.profile;
        let store = axion_store::Store::open(&profile.join("axion.sqlite"))
            .inspect_err(|e| tracing::error!("history unavailable: {e}"))
            .ok();
        let store_path = store.as_ref().map(|_| profile.join("axion.sqlite"));
        PrivateLock::drop_old_password(store.as_ref());
        let shields = ShieldsState::new(&profile, &config, store.as_ref());
        let downloads = Downloads::new(profile.clone());
        let vault_path = profile.join("vault.sqlite");
        let mut engine = CefEngine::new(
            device.clone(),
            queue.clone(),
            CefOptions {
                profile_dir: profile.clone(),
                accelerated,
                render_node: if std::env::var_os("AXION_NO_RENDER_NODE").is_some()
                    || !crate::hardware::pin_render_node(&config.hardware)
                {
                    None
                } else {
                    axion_engine::cef::render_node_for(info.vendor, info.device)
                },
                no_sandbox: std::env::var_os("AXION_NO_SANDBOX").is_some(),
                max_frame_rate: page_frame_rate(&config, &window),
                frame_settle: Duration::from_secs_f32(
                    std::env::var("AXION_FRAME_SETTLE_MS")
                        .ok()
                        .and_then(|v| v.parse::<f32>().ok())
                        .unwrap_or(config.tabs.gpu_frame_settle_ms)
                        .max(0.0)
                        / 1000.0,
                ),
                hardware_video_decoding: crate::hardware::video_decoding(&config),
                extra_switches: crate::hardware::chromium_switches(&config.hardware),
                waker: options.waker.clone(),
            },
        )?;
        engine.set_request_policy(Some(shields.policy()));
        engine.set_download_options(Downloads::options(&config));
        Downloads::prune(store.as_ref(), &config, unix_now());
        // What the last quit should have removed, in case it didn't get to (a crash).
        Downloads::on_quit(store.as_ref(), &config, &mut engine);
        engine.set_page_script(Some(page_script(&config)));

        let desktop_theme = DesktopTheme::start();
        let dark = is_dark(&config, &window, desktop_theme.dark());
        let scale = window.scale_factor() as f32;
        let mut ui = Ui::new(Theme::new(&config, dark), scale);
        let compositor = Compositor::new(&device, &queue, format, &mut ui.fonts());
        let queue = queue.clone();
        // Under the chrome, so menus and popovers cover the video controls.
        let player = PlayerOverlay::build(&mut ui);
        let chrome = Chrome::build(&mut ui, &config);
        let suggestions = SuggestionList::build(&mut ui, chrome.bar_node());
        let menu = Menu::build(&mut ui, chrome.bar_node(), PAGE_MENU_BASE, Placement::Below);
        // In the sidebar's plane, rising from the account button at its bottom.
        let account_menu = Menu::build(&mut ui, chrome.sidebar_node(), ACCOUNT_MENU_BASE, Placement::Above);
        let shields_popover = ShieldsPopover::build(&mut ui, chrome.bar_node());
        let downloads_popover = DownloadsPopover::build(&mut ui, chrome.bar_node());
        let permissions = permissions::PermissionsUi::build(&mut ui, chrome.bar_node(), store.as_ref());
        let toast = Toast::build(&mut ui, chrome.bar_node());
        let find = FindBar::build(&mut ui, chrome.bar_node());
        let lock_screen = LockScreen::build(&mut ui);
        let mut settings = SettingsSheet::build(&mut ui);
        settings.set_hardware(hardware_status.clone());
        // The vault's second key factor: this device's key (TPM-sealed or a private file in
        // the user's data directory, never in the profile).
        let device_key: std::sync::Arc<dyn axion_vault::DeviceKey> =
            match axion_vault::SystemDeviceKey::system() {
                Ok(key) => std::sync::Arc::new(key),
                Err(e) => {
                    tracing::error!("no device key: {e}");
                    std::sync::Arc::new(axion_vault::device::NoDeviceKey)
                }
            };
        let passwords = Passwords::build(&mut ui, chrome.bar_node(), &vault_path, device_key, &account_name);
        let lock = PrivateLock::new();
        let mut favicons = Favicons::new(device.clone(), queue.clone(), Some(profile.join("favicons")));
        favicons.set_dark(dark);
        favicons.set_contrast(config.appearance.favicon_contrast);
        let mut home = HomePage::new(&profile, favicons.dir().map(Into::into));
        // The single background image of older versions moves into the backgrounds directory.
        let migrated = home.migrate(&mut config);
        let media_service = Arc::new(crate::media_view::MediaService::default());
        media_service.set_bridge(axion_media::bridge_script(
            &bridge_secret(),
            &axion_media::BridgeOptions { own_fullscreen: false, trusted_sites: Vec::new() },
        ));
        engine.set_internal_pages(Some(Arc::new(crate::media_view::Chain(vec![
            home.service(),
            media_service.clone(),
        ]))));
        // Built after the chrome so it draws above it.
        let overview = Overview::build(&mut ui);
        let playlist = crate::playlist::Playlist::new(&profile);
        let playlist_ui =
            playlist::PlaylistUi::build(&mut ui, &config, chrome.bar_node(), device.clone(), queue.clone());
        let add_account = AddAccountSheet::build(&mut ui);
        let onboarding = Onboarding::build(&mut ui);
        let update_alert = UpdateAlert::build(&mut ui);
        let update_check_at = axion_update::auto_check_due(&config.updates, &axion_update::SystemClock)
            .then(|| Instant::now() + Duration::from_secs(u64::from(config.updates.startup_delay_seconds)));
        // The dragged tab's lens above the chrome, and the lock screen's alert above all.
        let tab_drag = TabDrag::build(&mut ui);
        let context_menus = ContextMenus::build(
            &mut ui,
            &[chrome.bar_node(), chrome.tab_strip_node(), chrome.sidebar_node(), chrome.bookmarks_bar_node()],
            chrome.bar_node(),
        );
        let tooltip_parent = ui.root();
        let tooltip = Tooltip::build(&mut ui, tooltip_parent);
        let (_, alert_plane) = lock_screen.planes();
        let root = ui.root();
        let mut planes = ui.children(root);
        planes.retain(|&n| n != alert_plane);
        planes.push(alert_plane);
        // The introduction covers everything, the lock screen's alert included.
        planes.push(onboarding.root());
        ui.set_children(root, &planes);
        // Where the sidebar goes back to when the lock screen is gone (see `order_lock_planes`).
        let sidebar_before =
            planes.iter().position(|&n| n == chrome.sidebar_node()).and_then(|i| planes.get(i + 1)).copied();

        let avatars = Avatars::new(device.clone(), queue.clone());
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
            menu,
            shields,
            shields_popover,
            downloads_popover,
            permissions,
            toast,
            updater: Updater::new(),
            update_alert,
            gpu_confirm: false,
            hardware_status,
            update_check_at,
            update_phase: Phase::Idle,
            about_icon: None,
            tooltip,
            panel_query: String::new(),
            find,
            file_drop: FileDrop::default(),
            pointer_probe: None,
            file_drag_at: None,
            clipboard_reader: ClipboardReader::default(),
            field_paste: ClipboardReader::default(),
            player,
            playlist,
            playlist_ui,
            capture_frame: 0,
            flat_audit: false,
            probe: std::env::var("AXION_TYPE_PROBE")
                .ok()
                .and_then(|v| typing_probe::Spec::parse(&v))
                .map(ProbeRun::new),
            captured_frames: Vec::new(),
            pip: None,
            instance,
            adapter: adapter.clone(),
            site_click: None,
            frame_stats: FrameStats::default(),
            now_playing: None,
            now_art: ArtStore::default(),
            mpris: if config.media.desktop_integration { Mpris::start() } else { None },
            memory: MemoryMonitor::start(Duration::from_secs_f32(
                config.tabs.check_interval_seconds.max(0.5),
            )),
            lifecycle_checked: Instant::now(),
            session_dirty: false,
            session_changed: Instant::now(),
            settings,
            passwords,
            config_dirty_since: migrated.then(Instant::now),
            last_saved_config: None,
            panel_urls: Vec::new(),
            panel: None,
            store,
            favicons,
            downloads,
            importer: Importer::new(store_path),
            overview_query: String::new(),
            last_frame: Instant::now(),
            window_tint: None,
            last_tint_step: Instant::now(),
            page_colors: PageTint::new(dark),
            home,
            media_service,
            home_thumbnails: false,
            backgrounds_listed: None,
            tabs: Tabs::default(),
            parked: Tabs::default(),
            private_mode: false,
            accounts,
            account_id,
            account_menu,
            add_account,
            onboarding,
            start_language,
            avatars,
            photo_chooser: PhotoChooser::default(),
            new_account_photo: None,
            relaunch: None,
            account_unlocked: false,
            lock,
            lock_screen,
            lock_shown: None,
            pending_move: None,
            sidebar_before,
            tab_drag,
            context_menus,
            context_tab: None,
            page_context: None,
            copy_image: None,
            context_row: None,
            bar_bookmarks: Vec::new(),
            bookmarks_changed: true,
            no_tabs: Tabs::default(),
            config,
            keymap,
            paths,
            watcher,
            dark,
            desktop_theme,
            desktop_dark: None,
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
            sweep: None,
            wheel_frames: 0,
            slide_view_size: None,
            slide_snapshot: None,
            slide_fade: None,
            slide_live_width: None,
            live_stretch: false,
            slide_prev_width: None,
            slide_sent: Vec::new(),
            slide_seen_width: None,
            page_lag: None,
            resize_bench: options.bench_resize.then(|| ResizeBench::new(options.capture_after)),
            shown_fingerprint: None,
            shown_textures: Vec::new(),
            shown_layers: Vec::new(),
            page_view: None,
            idle_redraw_at: None,
            frame_interval: frame_rate::display_interval(None),
            refresh_watch: RefreshWatch::default(),
            frames_skipped: 0,
            frames_presented: 0,
            pumped_page_frames: 0,
            pending_window_size: None,
            page_pacer: ResizePacer::default(),
            paced_view: None,
            background_resize: Debounce::default(),
            resize_log: None,
            resize_log_enabled: std::env::var_os("AXION_RESIZE_LOG").is_some(),
            resize_counters: ResizeCounters::default(),
            frame_times: FrameTimes::default(),
            perf_log,
            present_mode,
            sheet_origin: None,
            engine_icons: None,
            password_icons: None,
            svg_icons: crate::svg_icons::SvgIcons::new(),
            menu_click_frame: None,
            open_at: options.open.clone().filter(|_| options.capture.is_some()).map(|what| {
                (
                    Instant::now()
                        + options.capture_after.saturating_sub(options.lead.unwrap_or(OVERVIEW_CAPTURE_LEAD)),
                    what,
                )
            }),
            address_at: options.address.clone().filter(|_| options.capture.is_some()).map(|text| {
                (
                    Instant::now()
                        + options.capture_after.saturating_sub(options.lead.unwrap_or(OVERVIEW_CAPTURE_LEAD)),
                    text,
                )
            }),
            overview_at: (options.overview && options.capture.is_some()).then(|| {
                Instant::now()
                    + options.capture_after.saturating_sub(options.lead.unwrap_or(OVERVIEW_CAPTURE_LEAD))
            }),
        };
        browser.show_account();
        browser.start_permissions();
        browser.sync_frame_rate();
        browser.update_layout();
        browser.start_onboarding(&options, accounts_file, config_file);
        browser.playlist_start();
        browser.engine.set_color_scheme(browser.dark);
        let mut urls = options.urls;
        if urls.is_empty() && browser.config.tabs.restore_session && browser.restore_session() {
            return Ok(browser);
        }
        if urls.is_empty() {
            urls.push(browser.config.general.effective_homepage().to_owned());
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
        self.resize_counters.layouts += 1;
        let size = self.logical_size();
        self.ui.set_viewport(size, self.scale());
        self.chrome.apply_layout(&mut self.ui, &self.config, size);
        let tabs = if self.is_locked() { &self.no_tabs } else { &self.tabs };
        self.overview.resize(&mut self.ui, tabs, &self.favicons, size);
        self.update_lock();
        if self.settings.is_open() {
            self.settings.resize(&mut self.ui, size);
        }
        self.onboarding.resize(&mut self.ui, &self.config, size);
        let view_size = self.view_size();
        // While the sidebar slides, the shown tab's size is managed by the slide (see
        // `redraw_frame`); resizing it here too would make Chromium render an extra size.
        let sliding = self.slide_view_size.map(|(view, _)| view);
        let shown = self.display_view().filter(|v| Some(*v) != sliding);
        if let Some(view) = shown {
            self.resize_shown_view(view, view_size);
        }
        // The overview shows every tab: they follow at once there. Elsewhere the tabs in the
        // background follow once the window is still (see `pace_page_resizes`).
        let background: Vec<ViewId> = self
            .tabs
            .all()
            .iter()
            .map(|t| t.view)
            .filter(|v| Some(*v) != sliding && Some(*v) != shown)
            .collect();
        if self.overview.is_open() {
            for view in background {
                self.engine.resize(view, view_size);
            }
        } else if !background.is_empty() {
            let delay = Duration::from_secs_f32(self.config.layout.background_resize_delay.max(0.0));
            self.background_resize.touch(Instant::now(), delay);
        }
        self.layout_size = size;
    }

    // ---- tabs ----

    /// Opens a tab for a page's link or popup in its opener's group: a background tab of the
    /// other group never puts its tabs among the shown ones (they open in the background).
    fn open_tab_from(&mut self, opener: ViewId, url: &str, activate: bool) -> Result<()> {
        let opener_private = find_tab(&self.tabs, &self.parked, opener).map(|t| t.private);
        let private = crate::private::group_for_new_view(opener_private, self.private_mode);
        if private == self.private_mode {
            return self.open_tab(url, activate);
        }
        let view = self.engine.create_view(url, self.view_size(), private)?;
        let mut tab = Tab::new(view, url);
        tab.private = private;
        tab.needs_snapshot = true;
        self.parked.insert(tab, false);
        self.tabs_changed = true;
        Ok(())
    }

    fn open_tab(&mut self, url: &str, activate: bool) -> Result<()> {
        let view = self.engine.create_view(url, self.view_size(), self.private_mode)?;
        let previous = self.tabs.active().map(|t| t.view);
        let mut tab = Tab::new(view, url);
        tab.private = self.private_mode;
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
        if let Some(view) = self.tabs.all().get(index).map(|t| t.view) {
            self.playlist_flush_tab(view);
            self.playlist.pending.remove(&view);
        }
        let previous = self.tabs.active().map(|t| t.view);
        let Some(tab) = self.tabs.remove(index) else { return };
        self.engine.close_view(tab.view);
        self.tabs_changed = true;
        if self.tabs.is_empty() && self.private_mode {
            // Closing the last private tab leaves private browsing, like Safari.
            self.toggle_private_mode();
        } else if self.tabs.is_empty() {
            let home = self.config.general.effective_homepage().to_owned();
            if let Err(e) = self.open_tab(&home, true) {
                tracing::error!("opening a new tab failed: {e:#}");
            }
        } else if previous != self.tabs.active().map(|t| t.view) || previous == Some(tab.view) {
            self.on_active_changed(None);
        }
    }

    /// Closes the only tab without opening a replacement (the tab overview does that when it
    /// is left).
    fn close_last_tab_quietly(&mut self, index: usize) {
        if let Some(tab) = self.tabs.remove(index) {
            self.engine.close_view(tab.view);
            self.tabs_changed = true;
        }
    }

    /// Closes every private tab (leaving private browsing if it is shown).
    fn close_private_tabs(&mut self) {
        if self.private_mode {
            self.toggle_private_mode();
        }
        for tab in std::mem::take(&mut self.parked).all() {
            if self.pip.as_ref().is_some_and(|p| p.view == Some(tab.view)) {
                self.end_pip(tab.view, None, false);
            }
            if self.now_playing == Some(tab.view) {
                self.now_playing = None;
            }
            self.engine.close_view(tab.view);
        }
        self.tabs_changed = true;
    }

    fn select_tab(&mut self, index: usize) {
        let previous = self.tabs.active().map(|t| t.view);
        if self.tabs.select(index) {
            self.on_active_changed(previous);
        } else {
            // Picking the tab that is already active still leaves the Playlist page covering it.
            self.close_playlist();
        }
    }

    fn on_active_changed(&mut self, previous: Option<ViewId>) {
        self.close_playlist();
        // Find on Page belongs to the page it was opened on.
        self.close_find();
        self.passwords.hide_prompt(None, false);
        if let Some(tab) = self.tabs.active_mut() {
            tab.last_viewed = unix_now();
        }
        if let Some(view) = previous {
            self.engine.set_focus(view, false);
            self.engine.set_hidden(view, true);
            self.mark_hidden(view);
        }
        if let Some(tab) = self.tabs.active() {
            let view = tab.view;
            let delay = self.page_color_delay();
            self.page_colors.shown(view, tab.loading, delay);
            if is_home_url(&tab.url) {
                // Fresh Shields numbers and top sites.
                self.engine.run_script(view, REFRESH_SCRIPT, false);
            }
            self.wake(view);
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

    /// The view whose picture fills the page area: the active tab's, or the Playlist's
    /// playback while Axion's fullscreen player shows it.
    fn display_view(&self) -> Option<ViewId> {
        match &self.playlist.playback {
            Some(pb) if self.playlist.fullscreen => Some(pb.view),
            _ => self.active_view(),
        }
    }

    // ---- commands ----

    fn run(&mut self, action: Action) {
        let view = self.active_view();
        if self.is_locked() && !allowed_while_locked(action) {
            // A new tab in locked private browsing asks for the password first.
            if action == Action::NewTab {
                self.show_unlock_alert();
            }
            return;
        }
        if self.playlist_open() && !keeps_playlist(action) {
            self.close_playlist();
        }
        match action {
            Action::OpenPlaylist => {
                if self.playlist_open() {
                    self.close_playlist();
                } else {
                    self.open_playlist();
                }
            }
            Action::PlaylistAdd => self.playlist_add_action(),
            Action::FocusAddressBar => self.edit_address(),
            Action::NewTab => {
                let home = self.config.general.effective_homepage().to_owned();
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
                } else {
                    self.close_playlist();
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
            Action::ToggleFullscreen => self.set_presentation(!self.chrome.presentation()),
            Action::TabOverview => {
                if self.overview.is_open() {
                    self.close_overview(self.tabs.active_index());
                } else {
                    self.open_overview();
                }
            }
            Action::OpenSettings => {
                if self.settings.is_open() {
                    self.close_settings();
                } else {
                    self.menu.close(&mut self.ui, &self.config);
                    self.shields_popover.close(&mut self.ui, &self.config);
                    self.account_menu.close(&mut self.ui, &self.config);
                    self.settings.set_account(self.account_info());
                    self.sync_about();
                    self.settings.set_memory_used(self.memory.used());
                    self.settings.set_site_permissions(self.permissions.model.remembered());
                    let window = self.logical_size();
                    // Out of what it was opened from (the address field by default).
                    let fallback = self.address_origin();
                    let origin = self.sheet_origin.take().unwrap_or(fallback);
                    self.settings.open(&mut self.ui, &self.config, &self.keymap, window, origin, fallback);
                    if self.settings.category() == Category::Data {
                        self.settings_outcome(Outcome::DataPage);
                    }
                    // Typing right away searches the settings.
                    let events = self.ui.focus(self.settings.search_field());
                    self.handle_ui_events(events);
                }
            }
            Action::PinTab => {
                if let Some(tab) = self.tabs.active() {
                    let command = if tab.pinned {
                        crate::context_menu::TabCommand::Unpin
                    } else {
                        crate::context_menu::TabCommand::Pin
                    };
                    self.context_tab = Some((tab.view, self.private_mode));
                    self.tab_command(command);
                }
            }
            Action::BookmarkPage => {
                if let Some(tab) = self.tabs.active()
                    && let Some(store) = &self.store
                {
                    let (url, title) = (tab.url.clone(), tab.label());
                    let result = match store.bookmark_for(&url) {
                        Ok(Some(id)) => store.remove_bookmark(id),
                        Ok(None) => store.add_bookmark(&url, &title, unix_now()).map(|_| ()),
                        Err(e) => Err(e),
                    };
                    if let Err(e) = result {
                        tracing::error!("toggling the bookmark failed: {e}");
                    }
                    self.bookmarks_changed = true;
                }
            }
            Action::ShowBookmarks | Action::ShowHistory => {
                if !self.chrome.sidebar_open() {
                    self.chrome.toggle_sidebar();
                    self.update_layout();
                }
                let panel = if action == Action::ShowBookmarks { Panel::Bookmarks } else { Panel::History };
                self.show_panel(panel);
            }
            Action::OpenPasswords => self.open_passwords(),
            // The account menu's Lock: only accounts with a password have something to lock.
            Action::Lock => {
                if self.passwords.has_password() {
                    self.lock_account();
                }
            }
            Action::DevTools => self.toggle_devtools(DevToolsPanel::Default),
            Action::DevToolsConsole => self.toggle_devtools(DevToolsPanel::Console),
            Action::ViewSource => self.view_source(),
            Action::Find => self.open_find(),
            Action::FindNext | Action::FindPrevious => {
                if self.find.is_open() {
                    self.find.step(&mut self.engine, action == Action::FindNext);
                } else {
                    self.open_find();
                }
            }
            Action::Paste | Action::PastePlainText => {
                if let Some(view) = view {
                    let max = u64::from(self.config.clipboard.max_paste_mb) * 1024 * 1024;
                    self.clipboard_reader.read(view, action == Action::PastePlainText, max);
                }
            }
            Action::Quit => {}
            media if self.playlist_open() => self.playlist_media_key(media),
            media => {
                if let Some((view, command)) = self.player.key_command(media, &self.config) {
                    self.player.wake();
                    self.media_command(view, &command);
                }
            }
        }
    }

    /// Chromium's developer tools for the shown page, in their own window (see
    /// `Engine::open_devtools`). F12 / Ctrl+Shift+I toggle them; the Console shortcut opens them
    /// on the Console panel (or switches an open window to it).
    fn toggle_devtools(&mut self, panel: DevToolsPanel) {
        if !self.config.devtools.enabled {
            return;
        }
        let Some(view) = self.active_view() else { return };
        if self.engine.devtools_open(view) && panel == DevToolsPanel::Default {
            self.engine.close_devtools(view);
        } else {
            self.open_devtools(view, panel, None);
        }
    }

    /// Opens the developer tools of the tab showing `view`, with the element at `inspect_at`
    /// (page coordinates) selected if given: the entry point for a context menu's "Inspect"
    /// and the page menu.
    fn open_devtools(&mut self, view: ViewId, panel: DevToolsPanel, inspect_at: Option<(i32, i32)>) {
        if !self.config.devtools.enabled {
            return;
        }
        if let Err(e) = self.engine.open_devtools(view, &DevToolsOptions { panel, inspect_at }) {
            tracing::error!("opening the developer tools failed: {e}");
        }
    }

    /// The shown page's source in a new tab.
    fn view_source(&mut self) {
        if !self.config.devtools.enabled {
            return;
        }
        let Some(tab) = self.tabs.active() else { return };
        let url = tab.url.clone();
        // Pages that aren't loaded from the web have no source to show.
        let source = url.strip_prefix("view-source:").unwrap_or(&url);
        if !(source.starts_with("http://") || source.starts_with("https://") || source.starts_with("file:")) {
            return;
        }
        let url = format!("view-source:{source}");
        if let Err(e) = self.open_tab(&url, true) {
            tracing::error!("opening the page source failed: {e:#}");
        }
    }

    /// Private tabs are shown and locked: the lock screen covers them.
    fn is_locked(&self) -> bool {
        self.private_mode && self.private_locked()
    }

    /// Private tabs are locked (shown or not): the lock is on, the account has a password and
    /// the session isn't unlocked.
    fn private_locked(&self) -> bool {
        self.lock.hides_private(self.lock_enabled())
    }

    /// Private tabs have a lock: the setting is on and the account has a password.
    fn lock_enabled(&self) -> bool {
        self.config.privacy.lock_private_tabs && self.passwords.has_password()
    }

    /// The unlock alert, over the locked view or (from the normal tabs) on its own.
    fn show_unlock_alert(&mut self) {
        // The overview covers the whole window (no sidebar beside it): the card centres there,
        // above the note's Unlock button.
        let page = if self.overview.is_open() {
            Rect { origin: Vec2::ZERO, size: self.logical_size() }
        } else {
            self.page_rect()
        };
        let events = self.lock_screen.show_alert_over(&mut self.ui, &self.config, page);
        self.handle_ui_events(events);
    }

    // ---- accounts ----

    fn account(&self) -> Option<&axion_accounts::Account> {
        self.accounts.get(&self.account_id)
    }

    /// The texture of account `id`'s profile picture (decoded once; none: the initials show).
    fn picture_of(&self, id: &str) -> Option<crate::home::Image> {
        self.avatars.get(id, || self.accounts.avatar_path(id))
    }

    fn account_info(&self) -> AccountInfo {
        let account = self.account();
        AccountInfo {
            name: account.map(|a| a.name.clone()).unwrap_or_default(),
            initials: account.map(|a| a.initials()).unwrap_or_default(),
            color: account.map_or(axion_config::ACCENT_SWATCHES[0], |a| a.color),
            has_password: self.passwords.has_password(),
            picture: self.picture_of(&self.account_id),
            count: self.accounts.all().len(),
        }
    }

    /// Shows the current account wherever it appears (account button, lock screen, settings).
    fn show_account(&mut self) {
        let info = self.account_info();
        self.chrome.set_account(&mut self.ui, &info.name, &info.initials, info.color, info.picture.as_ref());
        self.lock_screen.set_account(&mut self.ui, &info.name);
        self.passwords.set_account_name(&info.name);
        self.settings.set_account(info);
        self.onboarding_account_changed();
    }

    fn toggle_account_menu(&mut self) {
        if self.account_menu.is_open() {
            self.account_menu.close(&mut self.ui, &self.config);
            return;
        }
        self.menu.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.downloads_popover.close(&mut self.ui, &self.config);
        // The actions below the list must fit in the menu.
        let shown = MAX_ITEMS - 3;
        let mut items: Vec<MenuItem> = self
            .accounts
            .all()
            .iter()
            .take(shown)
            .enumerate()
            .map(|(i, a)| {
                let current = a.id == self.account_id;
                MenuItem::account(
                    a.initials(),
                    a.color,
                    self.picture_of(&a.id),
                    &a.name,
                    current,
                    MenuAction::SwitchAccount(i),
                )
            })
            .collect();
        items.push(MenuItem::new(Icon::PersonAdd, tr("Add Account…"), MenuAction::AddAccount).separated());
        items.push(MenuItem::new(Icon::Settings, tr("Account Settings…"), MenuAction::AccountSettings));
        if self.passwords.has_password() {
            let mut lock = MenuItem::new(Icon::Lock, tr("Lock"), MenuAction::LockAccount);
            if let Some(chord) = self.keymap.chords_for(Action::Lock).first() {
                lock = lock.with_detail(chord.to_string());
            }
            items.push(lock);
        }
        let anchor = self.chrome.account_frame(&self.ui);
        let window = self.logical_size();
        // The Now Playing card above it is lifted out of its way (`sync_now_playing_lift`).
        self.account_menu.open(&mut self.ui, &self.config, anchor, window, items);
    }

    fn open_add_account(&mut self) {
        self.account_menu.close(&mut self.ui, &self.config);
        if self.settings.is_open() {
            self.close_settings();
        }
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
        self.new_account_photo = None;
        let events = self.add_account.open(&mut self.ui, &self.config);
        self.handle_ui_events(events);
    }

    fn add_account_action(&mut self, action: SheetAction) {
        match action {
            SheetAction::Cancel => {
                self.new_account_photo = None;
                self.add_account.close(&mut self.ui);
            }
            SheetAction::ChoosePhoto => self.photo_chooser.choose(PhotoFor::NewAccount),
            SheetAction::RemovePhoto => {
                self.new_account_photo = None;
                self.add_account.set_photo(&mut self.ui, None);
            }
            SheetAction::Create => {
                let Some(name) = self.add_account.name(&mut self.ui) else { return };
                match self.accounts.add(&name, unix_now()) {
                    Ok(id) => {
                        tracing::info!(account = %id, "account added");
                        if let Some(photo) = self.new_account_photo.take()
                            && let Err(e) = self.accounts.set_avatar(&id, &photo)
                        {
                            tracing::warn!("saving the new account's picture failed: {e}");
                        }
                        self.add_account.close(&mut self.ui);
                        self.switch_account(&id);
                    }
                    Err(e) => tracing::error!("adding the account failed: {e}"),
                }
            }
        }
    }

    fn open_account_settings(&mut self) {
        self.account_menu.close(&mut self.ui, &self.config);
        self.settings.set_category(Category::Account);
        if self.settings.is_open() {
            self.settings.set_account(self.account_info());
            self.refresh_settings();
        } else {
            // Out of the account button the account menu came from.
            let account = self.chrome.account_frame(&self.ui);
            self.sheet_origin =
                Some(Origin { plane: self.chrome.sidebar_node(), point: account.center(), control: account });
            self.run(Action::OpenSettings);
        }
    }

    /// Switches to account `id`. For now Axion relaunches into it (one Chromium profile per
    /// process): the window closes, the session is saved and Chromium shuts down, then the
    /// new process starts where this window was (see `crate::accounts`).
    pub fn switch_account(&mut self, id: &str) {
        if id == self.account_id || self.accounts.get(id).is_none() {
            return;
        }
        self.relaunch_into(id);
    }

    /// Closes this window and starts Axion again in account `id` (the current one too: the
    /// interface language is applied at the start).
    fn relaunch_into(&mut self, id: &str) {
        let size = self.window.inner_size().to_logical::<f64>(self.window.scale_factor());
        // Capture aid: the relaunched window takes the screenshot this one would have.
        let capture = self.capture.as_ref().map(|(path, _)| {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("capture");
            path.with_file_name(format!("{stem}-switched.png"))
        });
        self.relaunch = Some(Relaunch {
            account: id.to_owned(),
            size: (size.width, size.height),
            position: self.window.outer_position().ok().map(|p| (p.x, p.y)),
            maximized: self.window.is_maximized(),
            capture,
            exe: None,
        });
        self.capture = None;
        // The next start of a plain `axion` opens it too.
        if let Err(e) = self.accounts.select(Some(id)) {
            tracing::warn!("remembering the account failed: {e}");
        }
        tracing::info!(from = %self.account_id, to = %id, "restarting Axion");
    }

    /// Whether the window should close (an account switch is under way).
    pub fn wants_exit(&self) -> bool {
        self.relaunch.is_some()
    }

    /// The relaunch to start once this browser is dropped (Chromium shut down).
    pub fn take_relaunch(&mut self) -> Option<Relaunch> {
        self.relaunch.take()
    }

    /// Locks private tabs and Passwords now: the account menu's Lock, or the Passwords
    /// sheet's lock button. Nothing else locks during a session.
    fn lock_account(&mut self) {
        if self.passwords.is_open() {
            self.close_passwords();
        }
        let outcome = self.passwords.lock(&mut self.ui, &self.config);
        self.passwords_outcome(outcome);
        self.lock.lock_now();
        self.account_unlocked = false;
        self.pending_move = None;
        tracing::info!("account locked");
        self.private_tabs_locked();
        self.update_lock();
    }

    /// Private tabs just locked: nothing of them may stay in sight.
    fn private_tabs_locked(&mut self) {
        if !self.lock_enabled() {
            return;
        }
        if self.private_mode {
            if self.overview.is_open() {
                let events = self.overview.close_instantly(&mut self.ui);
                self.handle_ui_events(events);
            }
            self.close_popups();
            self.tab_drag.cancel(&mut self.ui);
        }
        // A private tab's Picture in Picture shows its page: it ends (the video keeps playing).
        if let Some(view) = self.pip_view()
            && find_tab(&self.tabs, &self.parked, view).is_some_and(|t| t.private)
        {
            self.end_pip(view, None, false);
        }
        self.tabs_changed = true;
    }

    /// Private tabs follow the vault: it was unlocked elsewhere (Passwords, a fill prompt).
    fn sync_account_lock(&mut self) {
        let unlocked = self.passwords.is_unlocked();
        if unlocked == self.account_unlocked {
            return;
        }
        self.account_unlocked = unlocked;
        if self.lock.follow_vault(unlocked) {
            if !unlocked {
                self.private_tabs_locked();
            }
            self.tabs_changed = true;
            self.update_lock();
        }
    }

    // ---- media ----

    fn media_command(&mut self, view: ViewId, command: &axion_media::Command) {
        tracing::debug!(?view, ?command, "media command");
        // Every volume control of a Playlist video sets the Playlist's one shared volume.
        if self.is_playback_view(view)
            && crate::playlist::remember_volume(&mut self.config.playlist, &command.action)
        {
            self.config_dirty_since.get_or_insert(Instant::now());
        }
        // Axion's fullscreen player on the Playlist's playback is the app's own state.
        if let axion_media::Action::Fullscreen(on) = command.action
            && self.is_playback_view(view)
        {
            self.playlist_set_fullscreen(on);
            return;
        }
        if self.click_site_player(view, command) {
            return;
        }
        self.engine.run_script(view, &command.script(), command.needs_gesture());
    }

    /// Sites' own players (hosters behind JW Player, video.js…) often load their video only
    /// when their play button is clicked, and iframes only play with sound after a click in
    /// them. So play/pause of such a player is a real click at the video's centre — with the
    /// site's UI hidden it lands on the video, which the site player toggles on. Returns false
    /// if this command isn't one (then the bridge carries it out).
    fn click_site_player(&mut self, view: ViewId, command: &axion_media::Command) -> bool {
        use axion_media::Action;
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return false };
        let Some(media) = tab.media.as_ref() else { return false };
        let Some(player) = media.get(&command.player).filter(|p| !p.native && !p.trusted).cloned() else {
            return false;
        };
        let click = match command.action {
            Action::Toggle => true,
            Action::Play => !player.is_playing(),
            Action::Pause => player.is_playing(),
            _ => return false,
        };
        let dpr = if media.dpr > 0.0 { media.dpr } else { self.scale() };
        let center = Vec2::new(player.x + player.w / 2.0, player.y + player.h / 2.0) * dpr / self.scale();
        let size = self.page_rect().size;
        if center.x < 0.0 || center.y < 0.0 || center.x > size.x || center.y > size.y {
            return false;
        }
        if click {
            self.click_page(view, center);
            // Hosters often swallow the first click (for an ad tab); try again if it didn't play.
            self.site_click = (!player.is_playing()).then(|| SiteClick {
                view,
                player: player.id.clone(),
                tries: 1,
                at: Instant::now(),
            });
        }
        true
    }

    fn click_page(&mut self, view: ViewId, pos: Vec2) {
        let modifiers = axion_engine::Modifiers::default();
        self.engine.send_input(view, &InputEvent::MouseMove { pos, modifiers });
        for pressed in [true, false] {
            self.engine.send_input(
                view,
                &InputEvent::MouseButton {
                    pos,
                    button: MouseButton::Left,
                    pressed,
                    click_count: 1,
                    modifiers,
                },
            );
        }
    }

    /// Clicks a site player again while it hasn't started (see `click_site_player`).
    fn retry_site_click(&mut self) {
        let Some(pending) = self.site_click.take() else { return };
        if pending.at.elapsed() < SITE_CLICK_RETRY {
            self.site_click = Some(pending);
            return;
        }
        let player = self.primary_player(pending.view).filter(|p| p.id == pending.player).cloned();
        tracing::debug!(found = player.is_some(), paused = ?player.as_ref().map(|p| p.paused), tries = pending.tries, "site click check");
        let Some(player) = player else { return };
        if !player.paused || pending.tries >= SITE_CLICK_TRIES {
            return;
        }
        let command = axion_media::Command::new(&player.id, axion_media::Action::Play);
        let before = self.site_click.is_none();
        if self.click_site_player(pending.view, &command) && before {
            if let Some(next) = &mut self.site_click {
                next.tries = pending.tries + 1;
            }
            tracing::debug!(tries = pending.tries + 1, "clicked the site player again");
        }
    }

    fn page_message(&mut self, view: ViewId, message: &str) {
        if axion_vault::autofill::is_message(message) {
            self.autofill_message(view, message);
            return;
        }
        let message = match axion_media::parse(message) {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!("{e}");
                return;
            }
        };
        match message {
            axion_media::Message::State(media) if self.playlist_playback_media(view, &media) => {}
            axion_media::Message::State(media) => {
                self.playlist_tab_media(view, &media);
                let playing = media.primary().is_some_and(axion_media::Player::is_playing);
                if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                    let audible = media.is_audible();
                    if audible != tab.playing_media {
                        self.tabs_changed = true;
                    }
                    tab.playing_media = audible;
                    tab.media = Some(media);
                }
                // The tab that started playing last is the one the desktop controls.
                let current_playing =
                    self.now_playing.and_then(|v| self.primary_player(v)).is_some_and(|p| p.is_playing());
                if playing
                    && (self.now_playing != Some(view) && !current_playing || self.now_playing.is_none())
                {
                    self.now_playing = Some(view);
                }
                self.update_now_playing();
            }
            axion_media::Message::Probe(state) => tracing::info!(%state, "media probe"),
            axion_media::Message::Failure { code, message } => {
                self.playlist_view_failed(view, &code, &message)
            }
            axion_media::Message::DoubleClick { id } => {
                let fullscreen = self.player.mode() != PlayerMode::Fullscreen;
                let command = axion_media::Command::new(&id, axion_media::Action::Fullscreen(fullscreen));
                self.media_command(view, &command);
            }
        }
    }

    /// The autofill script reported a sign-in form or a submitted password. Only the shown,
    /// unlocked tab counts, and logins are matched against the tab's own address.
    fn autofill_message(&mut self, view: ViewId, message: &str) {
        if Some(view) != self.active_view() || self.is_locked() {
            return;
        }
        let (Some(event), Some(url)) =
            (axion_vault::autofill::parse(message), self.tabs.active().map(|t| t.url.clone()))
        else {
            return;
        };
        self.passwords.page_event(&mut self.ui, &self.config, view, &url, event);
    }

    fn passwords_outcome(&mut self, outcome: PasswordsOutcome) {
        match outcome {
            PasswordsOutcome::Nothing => {}
            PasswordsOutcome::ConfigChanged => {
                self.config_dirty_since.get_or_insert(Instant::now());
            }
            PasswordsOutcome::Events(events) => self.handle_ui_events(events),
            PasswordsOutcome::Fill { view, script } => self.engine.run_script(view, &script, false),
            PasswordsOutcome::LockSession => self.lock_account(),
        }
    }

    fn open_passwords(&mut self) {
        if self.settings.is_open() {
            self.close_settings();
        }
        self.shields_popover.close(&mut self.ui, &self.config);
        let window = self.logical_size();
        let fallback = self.address_origin();
        let origin = self.sheet_origin.take().unwrap_or(fallback);
        let outcome = self.passwords.open(&mut self.ui, &self.config, window, origin, fallback);
        self.passwords_outcome(outcome);
    }

    /// The address field, where sheets come from by default (keyboard shortcuts) and drain
    /// back into when their own origin is gone.
    fn address_origin(&self) -> Origin {
        let address = self.chrome.address_frame(&self.ui);
        Origin { plane: self.chrome.bar_node(), point: address.center(), control: address }
    }

    /// Closes Settings: it drains back into where it came from.
    fn close_settings(&mut self) {
        let fallback = self.address_origin();
        self.settings.close(&mut self.ui, &self.config, fallback);
    }

    /// Closes Passwords: it drains back into where it came from.
    fn close_passwords(&mut self) {
        let fallback = self.address_origin();
        self.passwords.close(&mut self.ui, &self.config, fallback);
    }

    /// Capture aid: one log line per video Axion has state for.
    fn log_media_state(&self) {
        let tab = self.active_view().and_then(|v| self.primary_player(v));
        let playback = self.playlist.playback.as_ref().and_then(|p| p.player());
        let line = |p: Option<&axion_media::Player>| match p {
            Some(p) => format!(
                "time={:.2} duration={:.2} volume={:.2} muted={} paused={}",
                p.time, p.duration, p.volume, p.muted, p.paused
            ),
            None => "none".to_owned(),
        };
        tracing::info!(
            tab = %line(tab),
            playlist = %line(playback),
            playlist_volume = self.config.playlist.volume,
            playlist_muted = self.config.playlist.muted,
            "media state"
        );
    }

    fn primary_player(&self, view: ViewId) -> Option<&axion_media::Player> {
        find_tab(&self.tabs, &self.parked, view)?.media.as_ref()?.primary()
    }

    /// Selects the tab of `view`, switching between normal and private tabs if needed.
    fn show_view(&mut self, view: ViewId) {
        if !self.tabs.all().iter().any(|t| t.view == view) && self.parked.all().iter().any(|t| t.view == view)
        {
            self.toggle_private_mode();
        }
        if let Some(i) = self.tabs.all().iter().position(|t| t.view == view) {
            self.select_tab(i);
        }
    }

    /// Previous: the page's previous-track handler, else the track starts over.
    fn now_playing_previous(&mut self) {
        let Some(view) = self.now_playing else { return };
        let handled = find_tab(&self.tabs, &self.parked, view)
            .and_then(|t| t.media.as_ref())
            .and_then(|m| m.session.as_ref())
            .is_some_and(|s| s.prev);
        self.now_playing_command(if handled {
            axion_media::Action::PreviousTrack
        } else {
            axion_media::Action::Seek(0.0)
        });
    }

    /// Keeps the Now Playing card clear of the account menu: as the menu's glass grows upwards
    /// from the account button the card is lifted by as much.
    fn sync_now_playing_lift(&mut self) {
        let account = self.chrome.account_frame(&self.ui).origin.y;
        let lift = self.account_menu.top().map_or(0.0, |top| account - top);
        self.chrome.set_now_playing_lift(&mut self.ui, lift);
    }

    fn now_playing_command(&mut self, action: axion_media::Action) {
        let Some(view) = self.now_playing else { return };
        let Some(id) = self.primary_player(view).map(|p| p.id.clone()) else { return };
        self.media_command(view, &axion_media::Command::new(&id, action));
    }

    /// Refreshes the sidebar card and the desktop's media controls.
    fn update_now_playing(&mut self) {
        // Private tabs' media stays out of the desktop; the card shows it in private mode only.
        let tab = self.now_playing.and_then(|v| find_tab(&self.tabs, &self.parked, v));
        let tab = tab.filter(|t| t.media.as_ref().and_then(|m| m.primary()).is_some());
        if tab.is_none() {
            self.now_playing = None;
        }
        let info = tab.and_then(|t| {
            let media = t.media.as_ref()?;
            let player = media.primary()?;
            let session = media.session.clone().unwrap_or_default();
            let title = if session.title.is_empty() { t.label() } else { session.title };
            Some(NowPlayingInfo {
                private: t.private,
                player: player.clone(),
                title,
                artist: session.artist,
                album: session.album,
                artwork: session.artwork,
                image: media.image.clone(),
                url: t.url.clone(),
                host: display_host(&t.url),
                can_prev: session.prev,
                can_next: session.next,
            })
        });
        let private_shown = self.private_mode && !self.private_locked();
        let card = info.as_ref().filter(|i| !i.private || private_shown).map(|i| NowPlaying {
            title: i.title.clone(),
            subtitle: if i.artist.is_empty() { i.host.clone() } else { i.artist.clone() },
            playing: i.player.is_playing(),
            time: i.player.time,
            duration: i.player.duration,
            live: i.player.live,
            can_prev: i.can_prev,
            can_next: i.can_next,
        });
        self.chrome.set_now_playing(&mut self.ui, &self.config, card.as_ref());
        // Artwork: what the page offers (never fetched for private tabs), else the site's icon.
        let art_url = info
            .as_ref()
            .filter(|i| !i.private && self.config.media.now_playing_artwork)
            .and_then(|i| first_web_url(&[&i.artwork, &i.player.poster, &i.image]).map(str::to_owned));
        self.now_art.want(art_url.as_deref());
        if let Some(i) = info.as_ref().filter(|_| card.is_some()) {
            let art = match (self.now_art.image(), self.favicons.get(&i.url)) {
                (Some(image), _) => Art::Image(image),
                (None, Some(icon)) => Art::Favicon(icon),
                (None, None) => Art::None,
            };
            self.chrome.set_now_playing_art(&mut self.ui, art);
        }
        if let Some(mpris) = &mut self.mpris {
            let track = info.filter(|i| !i.private).map(|i| Track {
                playing: i.player.is_playing(),
                title: i.title,
                artist: i.artist,
                album: i.album,
                art_url: i.artwork,
                length: if i.player.live { 0.0 } else { i.player.duration },
                position: i.player.time,
                volume: if i.player.muted { 0.0 } else { i.player.volume },
                rate: i.player.rate,
                can_next: i.can_next,
                can_previous: i.can_prev,
            });
            mpris.update(track);
        }
    }

    fn handle_mpris(&mut self) {
        let Some(commands) = self.mpris.as_ref().map(Mpris::commands) else { return };
        for command in commands {
            use axion_media::Action as A;
            let action = match command {
                MprisCommand::Raise => {
                    self.window.focus_window();
                    if let Some(view) = self.now_playing {
                        self.show_view(view);
                    }
                    continue;
                }
                MprisCommand::Play => A::Play,
                MprisCommand::Pause => A::Pause,
                MprisCommand::PlayPause => A::Toggle,
                MprisCommand::Next => A::NextTrack,
                MprisCommand::Previous => {
                    self.now_playing_previous();
                    continue;
                }
                MprisCommand::Seek(by) => A::Skip(by),
                MprisCommand::SetPosition(to) => A::Seek(to),
                MprisCommand::Volume(v) => A::Volume(v),
                MprisCommand::Rate(r) => A::Rate(r),
            };
            self.now_playing_command(action);
        }
    }

    fn start_pip(&mut self, view: ViewId, player: axion_media::Player) {
        if let Some(old) = self.pip_view().filter(|&v| v != view) {
            self.end_pip(old, None, false);
        }
        if self.chrome.page_fullscreen() {
            let leave = axion_media::Command::new(&player.id, axion_media::Action::Fullscreen(false));
            self.media_command(view, &leave);
        }
        // Its window opens from the event loop (`sync_windows`).
        let (device, queue, config) = (&self.device, &self.queue, &self.config);
        self.pip.get_or_insert_with(|| PipWindow::new(device, queue, config)).show(view, player);
    }

    fn pip_view(&self) -> Option<ViewId> {
        self.pip.as_ref().and_then(|p| p.view)
    }

    /// Opens windows that need the event loop (the PiP window).
    pub fn sync_windows(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pip) = self.pip.as_mut().filter(|p| p.needs_window()) else { return };
        if let Err(e) = pip.open(event_loop, &self.instance, &self.adapter) {
            tracing::error!("Picture in Picture failed: {e:#}");
            if let Some(view) = pip.view {
                self.end_pip(view, None, false);
            }
        }
    }

    pub fn is_pip_window(&self, id: winit::window::WindowId) -> bool {
        self.pip.as_ref().and_then(PipWindow::id) == Some(id)
    }

    pub fn is_main_window(&self, id: winit::window::WindowId) -> bool {
        self.window.id() == id
    }

    pub fn pip_window_event(&mut self, event: WindowEvent) {
        let Some(pip) = &mut self.pip else { return };
        let outcome = pip.window_event(event, &self.config);
        self.pip_outcome(outcome);
    }

    fn pip_outcome(&mut self, outcome: Option<PipOutcome>) {
        match outcome {
            Some(PipOutcome::Command(view, command)) => self.media_command(view, &command),
            Some(PipOutcome::Commands(view, commands)) => {
                for command in commands {
                    self.media_command(view, &command);
                }
            }
            Some(PipOutcome::Close(view, id)) => self.end_pip(view, Some(&id), false),
            Some(PipOutcome::BackToTab(view)) => {
                self.end_pip(view, None, true);
                self.window.focus_window();
            }
            None => {}
        }
    }

    /// Ends PiP; `pause` with the player's id pauses the video (the × button).
    fn end_pip(&mut self, view: ViewId, pause: Option<&str>, show_tab: bool) {
        if self.is_playback_view(view) {
            // The Playlist's playback: no tab to go back to.
            if let Some(id) = pause {
                self.media_command(view, &axion_media::Command::new(id, axion_media::Action::Pause));
            }
            if let Some(pip) = &mut self.pip {
                pip.hide();
            }
            self.playlist_pip_ended(show_tab);
            return;
        }
        let takeover =
            self.pip.as_ref().filter(|p| p.takeover).and_then(|p| p.player.as_ref()).map(|p| p.id.clone());
        if let Some(id) = takeover {
            self.media_command(view, &axion_media::Command::new(&id, axion_media::Action::Takeover(false)));
        }
        if let Some(id) = pause {
            self.media_command(view, &axion_media::Command::new(id, axion_media::Action::Pause));
        }
        if let Some(pip) = &mut self.pip {
            pip.hide();
        }
        if show_tab {
            self.show_view(view);
        } else if Some(view) != self.active_view() {
            self.engine.set_hidden(view, true);
        }
    }

    /// Keeps the PiP picture current: the video's part of its tab while that tab is shown,
    /// otherwise the whole (hidden, still rendering) tab, where the video covers the page.
    fn update_pip(&mut self) {
        let Some(view) = self.pip_view() else { return };
        let id = self.pip.as_ref().and_then(|p| p.player.as_ref()).map(|p| p.id.clone()).unwrap_or_default();
        let found = self.media_of(view).map(|(media, dpr)| (media.get(&id).cloned(), dpr));
        let Some((Some(player), dpr)) = found else {
            // The tab closed or its page went away: PiP ends with it.
            if let Some(pip) = &mut self.pip {
                pip.hide();
            }
            return;
        };
        // The Playlist's playback is only the video: nothing to cover, and its view is kept
        // alive and sized by the Playlist.
        let own_view = self.is_playback_view(view);
        let shown = !own_view && Some(view) == self.active_view() && !self.overview.is_open();
        let takeover = self.pip.as_ref().is_some_and(|p| p.takeover);
        if own_view {
            self.engine.request_frame(view);
        } else if shown && takeover {
            self.media_command(view, &axion_media::Command::new(&id, axion_media::Action::Takeover(false)));
        } else if !shown && !takeover {
            self.media_command(view, &axion_media::Command::new(&id, axion_media::Action::Takeover(true)));
            self.engine.set_hidden(view, false);
            // A hidden-then-shown view only renders again after a resize.
            self.engine.resize(view, self.view_size());
        }
        if !shown && !own_view {
            self.engine.request_frame(view);
        }
        let picture = self.engine.frame(view).map(|frame| {
            let tex = Vec2::new(frame.texture.width() as f32, frame.texture.height() as f32);
            let uv = if !shown {
                letterbox(tex, player.aspect())
            } else {
                // The video itself, not its element's black bars around it.
                let (x, y, w, h) = player.picture_rect();
                let r = Rect::new(x * dpr, y * dpr, w * dpr, h * dpr);
                let min = (r.origin / tex).clamp(Vec2::ZERO, Vec2::ONE);
                let max = ((r.origin + r.size) / tex).clamp(Vec2::ZERO, Vec2::ONE);
                Rect { origin: min, size: (max - min).max(Vec2::splat(0.001)) }
            };
            let size = (frame.texture.width(), frame.texture.height());
            ((frame.texture.create_view(&Default::default()), size), uv)
        });
        if let Some(pip) = &mut self.pip {
            pip.takeover = !shown;
            pip.player = Some(player);
            pip.render(picture, &self.config);
        }
    }

    /// Whole-page fullscreen (F11): the window goes fullscreen and Axion's chrome gives way
    /// to the page; the page's own element fullscreen (video) is separate and keeps working.
    fn set_presentation(&mut self, on: bool) {
        if !self.chrome.set_presentation(on, &self.config) {
            return;
        }
        let window = on || self.chrome.page_fullscreen();
        self.window.set_fullscreen(window.then_some(Fullscreen::Borderless(None)));
        self.update_layout();
    }

    fn update_player(&mut self, dt: f32) {
        self.retry_site_click();
        self.update_pip();
        let locked = self.is_locked();
        let tab = self.tabs.active().filter(|_| !self.overview.is_open() && !locked);
        // Fullscreen from the Playlist: the player is about its playback view.
        let playback = self.playlist.playback.as_ref().filter(|_| self.playlist.fullscreen);
        let cx = PlayerContext {
            view: playback.map(|p| p.view).or(tab.map(|t| t.view)),
            media: match playback {
                Some(p) => p.media.as_ref(),
                None => tab.and_then(|t| t.media.as_ref()),
            },
            page: self.page_rect(),
            pointer: self.pointer,
            page_fullscreen: self.chrome.page_fullscreen(),
        };
        self.player.update(&mut self.ui, &self.config, &cx, dt);
        if let Some((view, command)) = self.player.takeover_command() {
            self.media_command(view, &command);
        }
    }

    // ---- session ----

    /// Reopens the last session's tabs: the active one loads, the others stay unloaded until
    /// they are shown. Returns false if there was nothing to restore.
    fn restore_session(&mut self) -> bool {
        let Some((saved, active)) = self.store.as_ref().and_then(|s| s.session().ok()) else { return false };
        let pinned: usize = self
            .store
            .as_ref()
            .and_then(|s| s.meta(SESSION_PINNED_KEY).ok().flatten())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if saved.is_empty() {
            return false;
        }
        let size = self.view_size();
        // Tabs not looked at for longer than the setting allows don't come back.
        let max_age = i64::from(self.config.tabs.close_after_days) * DAY_SECONDS;
        let now = unix_now();
        let mut kept: usize = 0;
        let mut active_kept = 0;
        for (i, saved_tab) in saved.iter().enumerate() {
            let (url, title) = (&saved_tab.url, &saved_tab.title);
            if max_age > 0
                && i != active
                && i >= pinned
                && saved_tab.last_viewed > 0
                && now - saved_tab.last_viewed > max_age
            {
                continue;
            }
            if i == active {
                active_kept = kept;
            }
            kept += 1;
            let view = if i == active {
                match self.engine.create_view(url, size, false) {
                    Ok(view) => view,
                    Err(e) => {
                        tracing::error!("restoring {url} failed: {e}");
                        self.engine.create_discarded_view(url, size, false)
                    }
                }
            } else {
                self.engine.create_discarded_view(url, size, false)
            };
            let mut tab = Tab::new(view, url);
            tab.title.clone_from(title);
            tab.pinned = i < pinned;
            if saved_tab.last_viewed > 0 {
                tab.last_viewed = saved_tab.last_viewed;
            }
            if self.engine.is_discarded(view) {
                tab.lifecycle = Lifecycle::Discarded;
                tab.loading = false;
            }
            self.tabs.insert(tab, true);
        }
        if kept == 0 {
            return false;
        }
        self.tabs.select(active_kept.min(kept - 1));
        self.on_active_changed(None);
        tracing::info!(tabs = kept, "session restored");
        true
    }

    /// Writes the open (non-private) tabs a moment after they last changed.
    fn save_session_if_due(&mut self, force: bool) {
        if !self.session_dirty || (!force && self.session_changed.elapsed() < SESSION_SAVE_DELAY) {
            return;
        }
        self.session_dirty = false;
        let normal = if self.private_mode { &self.parked } else { &self.tabs };
        let tabs: Vec<axion_store::SessionTab> = normal
            .all()
            .iter()
            .map(|t| axion_store::SessionTab {
                url: t.url.clone(),
                title: t.title.clone(),
                last_viewed: t.last_viewed,
            })
            .collect();
        let active = normal.active_index();
        let pinned = normal.pinned_count().to_string();
        if let Some(store) = &mut self.store
            && let Err(e) = store
                .save_session(&tabs, active)
                .and_then(|()| store.set_meta(SESSION_PINNED_KEY, Some(&pinned)))
        {
            tracing::warn!("saving the session failed: {e}");
        }
    }

    // ---- lifecycle ----

    /// The tab was just hidden: its background time starts now.
    fn mark_hidden(&mut self, view: ViewId) {
        if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view)
            && tab.lifecycle == Lifecycle::Active
        {
            tab.lifecycle = Lifecycle::Background;
            tab.hidden_since = Some(Instant::now());
        }
    }

    /// The tab is about to be shown: reload it if it was unloaded, resume it if frozen.
    fn wake(&mut self, view: ViewId) {
        let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) else { return };
        match tab.lifecycle {
            Lifecycle::Discarded => {
                if let Err(e) = self.engine.restore(view) {
                    tracing::error!("reloading an unloaded tab failed: {e}");
                }
                tab.loading = true;
            }
            Lifecycle::Frozen => self.engine.set_frozen(view, false),
            Lifecycle::Active | Lifecycle::Background => {}
        }
        tab.lifecycle = Lifecycle::Active;
        tab.hidden_since = None;
    }

    /// Freezes and unloads background tabs as the settings say. Runs every few seconds.
    fn apply_lifecycle(&mut self) {
        let interval = Duration::from_secs_f32(self.config.tabs.check_interval_seconds.max(0.5));
        let now = Instant::now();
        if now.duration_since(self.lifecycle_checked) < interval {
            return;
        }
        self.lifecycle_checked = now;
        let pip_view = self.pip_view();
        let infos: Vec<TabInfo<ViewId>> = self
            .tabs
            .all()
            .iter()
            .chain(self.parked.all())
            .map(|t| TabInfo {
                key: t.view,
                lifecycle: t.lifecycle,
                hidden_since: t.hidden_since,
                playing_media: t.playing_media || Some(t.view) == pip_view,
                // A background tab still waiting for its overview snapshot counts as loading.
                loading: t.loading || t.needs_snapshot,
                devtools_open: self.engine.devtools_open(t.view),
                capturing: t.capture.any(),
            })
            .collect();
        self.close_unused_tabs();
        let used = self.memory.used();
        for transition in axion_tabs::plan(&lifecycle::policy(&self.config), &infos, now, used) {
            let (view, state) = match transition {
                Transition::Freeze(view) => {
                    self.engine.set_frozen(view, true);
                    (view, Lifecycle::Frozen)
                }
                Transition::Discard(view) => {
                    self.engine.discard(view);
                    (view, Lifecycle::Discarded)
                }
            };
            tracing::info!(?view, ?state, memory_mb = used.map(|b| b / (1024 * 1024)), "tab lifecycle");
            if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                tab.lifecycle = state;
                tab.playing_media = false;
            }
        }
    }

    /// Closes background tabs not looked at for longer than `tabs.close_after_days` (Safari's
    /// "Close Tabs"), except ones playing media.
    fn close_unused_tabs(&mut self) {
        let days = self.config.tabs.close_after_days;
        if days == 0 || self.private_mode {
            return;
        }
        let cutoff = unix_now() - i64::from(days) * DAY_SECONDS;
        let active = self.tabs.active_index();
        let stale: Vec<usize> = self
            .tabs
            .all()
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                *i != active && t.last_viewed < cutoff && !t.playing_media && !t.pinned && !t.capture.any()
            })
            .map(|(i, _)| i)
            .collect();
        for i in stale.into_iter().rev() {
            tracing::info!(url = %self.tabs.all()[i].url, "closing a tab not looked at for {days} days");
            self.close_tab(i);
        }
    }

    /// Switches between the normal and the private tab group.
    fn toggle_private_mode(&mut self) {
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
            self.engine.set_hidden(view, true);
            self.mark_hidden(view);
        }
        std::mem::swap(&mut self.tabs, &mut self.parked);
        self.private_mode = !self.private_mode;
        self.menu.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.suggestions.hide(&mut self.ui, &self.config);
        if self.tabs.is_empty() && self.overview.is_open() && !self.private_mode {
            // Normal tabs all closed in the overview: it makes one when it is left.
        } else if self.tabs.is_empty() {
            let home = self.config.general.effective_homepage().to_owned();
            if let Err(e) = self.open_tab(&home, true) {
                tracing::error!("opening a tab failed: {e:#}");
            }
        } else {
            self.on_active_changed(None);
        }
        self.apply_theme();
        self.tabs_changed = true;
        self.update_lock();
    }

    /// Shows or hides the lock screen and hides private tab titles while locked.
    fn update_lock(&mut self) {
        let has_lock = self.lock_enabled();
        let hidden = self.private_locked();
        self.chrome.set_private(&mut self.ui, self.private_mode, has_lock, hidden);
        let locked = self.is_locked();
        let page = locked.then(|| self.page_rect());
        if page == self.lock_shown {
            return;
        }
        let was_locked = self.lock_shown.is_some();
        self.lock_shown = page;
        self.chrome.set_locked(&mut self.ui, &self.config, locked);
        if locked {
            if let Some(view) = self.active_view() {
                self.engine.set_focus(view, false);
            }
            self.chrome.set_editing(&mut self.ui, false);
            if !was_locked {
                // Popovers about the page (Shields, the page menu, a fill offer) go with it.
                self.close_popups();
            }
        }
        let radius = self.config.effective_layout().page_corner_radius;
        self.lock_screen.show(&mut self.ui, page, radius);
        self.order_lock_planes(locked);
        self.tabs_changed = true;
        if locked && !was_locked && self.config.privacy.prompt_unlock_on_open && !self.overview.is_open() {
            // The password card comes up by itself, focused: no extra click to start typing.
            self.show_unlock_alert();
        }
    }

    /// The lock screen's cover lies above the toolbar and its popovers (nothing of a locked
    /// page shows through them). While it is up, the sidebar comes above it, so a floating
    /// sidebar still switches to the normal tabs; otherwise the sidebar stays below the bar.
    fn order_lock_planes(&mut self, locked: bool) {
        let root = self.ui.root();
        let (cover, _) = self.lock_screen.planes();
        let sidebar = self.chrome.sidebar_node();
        let mut planes = self.ui.children(root);
        let original = self.sidebar_before;
        planes.retain(|&n| n != sidebar);
        if self.playlist_open() && !locked {
            // The Playlist page lies under the sidebar and the toolbar (whose only visible
            // part is then the sidebar's button).
            let page = self.playlist_ui.page.root();
            planes.retain(|&n| n != page);
            let at = original.and_then(|next| planes.iter().position(|&n| n == next)).unwrap_or(0);
            planes.insert(at, sidebar);
            planes.insert(at, page);
            self.ui.set_children(root, &planes);
            return;
        }
        let at = if locked {
            planes.iter().position(|&n| n == cover).map(|i| i + 1)
        } else {
            original.and_then(|next| planes.iter().position(|&n| n == next))
        };
        planes.insert(at.unwrap_or(0).min(planes.len()), sidebar);
        self.ui.set_children(root, &planes);
    }

    /// The lock screen's password: unlocks the account (private tabs and Passwords).
    fn try_unlock(&mut self) {
        let password = zeroize::Zeroizing::new(self.lock_screen.password(&self.ui));
        match self.passwords.unlock_with(&mut self.ui, &self.config, &password) {
            Ok(()) => {
                self.lock.unlock();
                self.account_unlocked = true;
                // The alert may be up on its own (a tab dropped into the private ones).
                self.lock_screen.close_alert(&mut self.ui);
                self.tabs_changed = true;
                self.update_lock();
                if self.overview.is_open() {
                    // Unlocked inside the overview: the private tabs' cards replace the note.
                    self.sync_overview_mode();
                    self.rebuild_overview();
                }
                if let Some((view, index)) = self.pending_move.take()
                    && let Some(from) = self.group(false).index_of(view)
                {
                    self.move_to_other_group(false, from, index);
                }
                if let Some(view) = self.active_view() {
                    self.engine.set_focus(view, true);
                }
            }
            Err(axion_vault::VaultError::WrongPassword) => self.lock_screen.wrong_password(&mut self.ui),
            Err(e) => {
                tracing::error!("unlocking the account failed: {e}");
                self.lock_screen.show_error(&mut self.ui, &crate::passwords::describe(&e));
            }
        }
    }

    /// Rebuilds the overview's cards. It lists none while private tabs are shown and locked
    /// (it still switches to the normal tabs).
    fn rebuild_overview(&mut self) {
        let tabs = if self.is_locked() { &self.no_tabs } else { &self.tabs };
        self.overview.rebuild(&mut self.ui, tabs, &self.favicons);
    }

    fn open_overview(&mut self) {
        self.close_playlist();
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
        self.chrome.set_editing(&mut self.ui, false);
        let window = self.logical_size();
        let page = self.page_rect();
        let tabs = if self.is_locked() { &self.no_tabs } else { &self.tabs };
        let events = self.overview.open(&mut self.ui, tabs, &self.favicons, &self.config, window, page);
        self.overview_query.clear();
        self.handle_ui_events(events);
        self.sync_overview_mode();
    }

    /// Closes the overview into tab `index`, which becomes the active tab.
    fn close_overview(&mut self, index: usize) {
        if self.is_locked() {
            // No page zooms out of it: the lock screen is there.
            let events = self.overview.close_instantly(&mut self.ui);
            self.handle_ui_events(events);
            return;
        }
        let index = if self.tabs.is_empty() {
            // The last tab was closed in the overview; leaving it asks for a new one.
            let home = self.config.general.effective_homepage().to_owned();
            if let Err(e) = self.open_tab(&home, true) {
                tracing::error!("opening a new tab failed: {e:#}");
            }
            self.tabs.active_index()
        } else {
            index
        };
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
            OverviewAction::Close(i) if !self.is_locked() => {
                if self.tabs.len() == 1 && !self.private_mode {
                    // The only tab: no new one until the overview is left.
                    self.close_last_tab_quietly(i);
                } else {
                    self.close_tab(i);
                }
                // Closing the last private tab went back to the normal ones.
                self.sync_overview_mode();
                self.rebuild_overview();
            }
            OverviewAction::Close(_) => {}
            OverviewAction::NewTab | OverviewAction::Unlock if self.is_locked() => self.show_unlock_alert(),
            OverviewAction::Unlock => {}
            OverviewAction::NewTab => {
                let home = self.config.general.effective_homepage().to_owned();
                if let Err(e) = self.open_tab(&home, true) {
                    tracing::error!("opening a new tab failed: {e:#}");
                }
                self.rebuild_overview();
                self.close_overview(self.tabs.active_index());
            }
            OverviewAction::ShowPrivate | OverviewAction::ShowNormal => {
                let want_private = action == OverviewAction::ShowPrivate;
                if want_private == self.private_mode {
                    self.sync_overview_mode();
                    return;
                }
                if want_private && self.private_locked() {
                    // Locked private tabs must never be seen: leave the overview at once (no
                    // zoom of a private page), then the lock screen takes over.
                    let events = self.overview.close_instantly(&mut self.ui);
                    self.handle_ui_events(events);
                    self.toggle_private_mode();
                    return;
                }
                self.toggle_private_mode();
                // The other group's pages stay hidden behind the overview until one is picked.
                if let Some(view) = self.active_view() {
                    self.engine.set_focus(view, false);
                }
                self.sync_overview_mode();
                self.rebuild_overview();
            }
            OverviewAction::ToggleSearch => {
                let events = self.overview.toggle_search(&mut self.ui);
                self.handle_ui_events(events);
                self.rebuild_overview();
            }
        }
    }

    fn sync_overview_mode(&mut self) {
        let normal = if self.private_mode { self.parked.len() } else { self.tabs.len() };
        self.overview.set_mode(&mut self.ui, self.private_mode, normal);
        let note = self.is_locked().then_some(tr("“Private Browsing” is locked"));
        self.overview.set_note(&mut self.ui, note);
    }

    fn show_panel(&mut self, panel: Panel) {
        // A freshly opened list starts unfiltered.
        self.ui.set_field_text(self.chrome.panel_search_field(), "");
        self.chrome.reset_panel_groups();
        self.show_panel_with(panel, false);
    }

    /// Re-filters the open sidebar list when its search field changed. Call every frame.
    fn poll_panel_search(&mut self) {
        let Some((panel, _)) = self.panel else { return };
        if panel == Panel::Downloads {
            return;
        }
        let text = self.ui.field_text(self.chrome.panel_search_field()).unwrap_or_default();
        if text.trim() != self.panel_query {
            self.show_panel_with(panel, false);
        }
    }

    /// Shows a sidebar panel; `keep_scroll` after a change in it (an entry deleted).
    fn show_panel_with(&mut self, panel: Panel, keep_scroll: bool) {
        let Some(store) = &self.store else { return };
        let query = if panel == Panel::Downloads {
            String::new()
        } else {
            self.ui.field_text(self.chrome.panel_search_field()).unwrap_or_default().trim().to_owned()
        };
        let bookmarks = if query.is_empty() {
            store.bookmarks()
        } else {
            store.search_bookmarks(&query, PANEL_SEARCH_LIMIT)
        };
        let items: Vec<PanelItem> = match panel {
            Panel::Bookmarks => bookmarks
                .unwrap_or_default()
                .into_iter()
                .map(|b| PanelItem {
                    // Imported bookmarks keep their old folder ("Dev / Rust · docs.rs").
                    detail: if b.folder_path.is_empty() {
                        display_host(&b.url)
                    } else {
                        format!("{} · {}", b.folder_path, display_host(&b.url))
                    },
                    title: b.title,
                    url: b.url,
                    section: None,
                    deletable: false,
                    ..PanelItem::default()
                })
                .collect(),
            Panel::History => {
                let now = unix_now();
                let mut entries = if query.is_empty() {
                    store.recent_history(PANEL_HISTORY_LIMIT)
                } else {
                    store.search_history(&query, PANEL_HISTORY_LIMIT, now)
                }
                .unwrap_or_default();
                // The list is grouped by day, newest first (search ranks by relevance).
                entries.sort_by_key(|e| std::cmp::Reverse(e.last_visit));
                crate::history::panel_items(entries, now)
            }
            // Reading list entries arrive with Phase 6.
            Panel::ReadingList => Vec::new(),
            Panel::Downloads => {
                self.downloads.items(List::Panel, Some(store), &self.config, unix_now(), usize::MAX)
            }
        };
        let title = match panel {
            Panel::Bookmarks => tr("Bookmarks"),
            Panel::History => tr("History"),
            Panel::ReadingList => tr("Reading List"),
            Panel::Downloads => tr("Downloads"),
        };
        self.panel_urls = items.iter().map(|i| i.url.clone()).collect();
        let clear = match panel {
            Panel::History => (!items.is_empty() && query.is_empty()).then_some(tr(CLEAR_HISTORY)),
            Panel::Downloads => self.downloads.has_finished().then_some(tr("Clear List")),
            _ => None,
        };
        self.panel = Some((panel, false));
        self.panel_query = query.clone();
        self.chrome.set_panel_searchable(&mut self.ui, panel != Panel::Downloads);
        self.chrome.set_panel_empty_text(if query.is_empty() {
            tr("Nothing here yet").into()
        } else {
            tr_fmt!("No results for “{query}”", query = query)
        });
        // History days fold away, except while a search lists its matches.
        let foldable = (panel == Panel::History && query.is_empty())
            .then_some(self.config.sidebar.history_collapse_older);
        self.chrome.set_panel_groups(foldable);
        self.chrome.show_panel(&mut self.ui, title, &items, &self.favicons, clear, keep_scroll);
    }

    /// The History panel's clear button: the first click asks, the second clears.
    fn clear_panel(&mut self) {
        if let Some((Panel::Downloads, _)) = self.panel {
            Downloads::clear_list(self.store.as_ref());
            self.show_panel_with(Panel::Downloads, true);
            return;
        }
        let Some((Panel::History, confirming)) = self.panel else { return };
        if !confirming {
            self.panel = Some((Panel::History, true));
            self.chrome.set_panel_clear_label(&mut self.ui, tr(CLEAR_HISTORY_CONFIRM));
            return;
        }
        if let Some(store) = &self.store
            && let Err(e) = store.clear_history_since(0)
        {
            tracing::error!("clearing history failed: {e}");
        }
        self.show_panel(Panel::History);
    }

    fn delete_panel_item(&mut self, index: usize) {
        let Some((panel, _)) = self.panel else { return };
        if panel == Panel::Downloads {
            self.downloads.remove_row(index, self.store.as_ref());
            self.show_panel_with(panel, true);
            return;
        }
        let (Some(store), Some(url)) = (&self.store, self.panel_urls.get(index)) else { return };
        if let Err(e) = store.delete_history_url(url) {
            tracing::error!("deleting a history entry failed: {e}");
        }
        self.show_panel_with(panel, true);
    }

    fn open_page_menu(&mut self) {
        if self.menu.is_open() {
            self.menu.close(&mut self.ui, &self.config);
            return;
        }
        let Some(tab) = self.tabs.active() else { return };
        let bookmarked = self.store.as_ref().and_then(|s| s.bookmark_for(&tab.url).ok().flatten()).is_some();
        let zoom = zoom_label(tab.zoom);
        let items = vec![
            if bookmarked {
                MenuItem::new(Icon::Bookmark, tr("Remove Bookmark"), MenuAction::RemoveBookmark)
            } else {
                MenuItem::new(Icon::Bookmark, tr("Add Bookmark"), MenuAction::AddBookmark)
            },
            MenuItem::new(Icon::Share, tr("Copy Link"), MenuAction::CopyLink),
            MenuItem::new(Icon::Search, tr("Find on Page"), MenuAction::FindOnPage),
            MenuItem::new(Icon::ZoomIn, tr("Zoom"), MenuAction::Zoom).with_detail(zoom.clone()).with_submenu(
                vec![
                    MenuItem::controls(vec![
                        MenuControl::icon(Icon::Minus, MenuAction::ZoomOut),
                        MenuControl::text(zoom, MenuAction::ZoomReset),
                        MenuControl::icon(Icon::Plus, MenuAction::ZoomIn),
                    ])
                    .with_caption(tr("Zoom")),
                ],
            ),
            MenuItem::new(Icon::Reload, tr("Reload Without Cache"), MenuAction::HardReload),
            MenuItem::new(Icon::Settings, tr("Settings"), MenuAction::Settings),
        ];
        let mut items = items;
        if !self.playlist_ui.candidates.is_empty() {
            let add = MenuItem::new(
                Icon::PlaylistAdd,
                tr("Add to Playlist"),
                MenuAction::Playlist(crate::playlist::Command::Add),
            );
            items.insert(2, add);
        }
        items.insert(
            items.len() - 1,
            MenuItem::new(
                Icon::Playlist,
                tr("Show Playlist"),
                MenuAction::Playlist(crate::playlist::Command::Show),
            ),
        );
        if self.config.devtools.enabled {
            let mut tools = MenuItem::new(Icon::Code, tr("Developer Tools"), MenuAction::DevTools);
            if let Some(keys) = self.keymap.chords_for(Action::DevTools).first() {
                tools = tools.with_detail(keys.to_string());
            }
            items.insert(items.len() - 1, tools);
        }
        items.insert(
            items.len() - 1,
            MenuItem::new(Icon::Info, tr("Check for Updates…"), MenuAction::CheckForUpdates),
        );
        self.account_menu.close(&mut self.ui, &self.config);
        let anchor = self.chrome.page_menu_frame(&self.ui);
        let window = self.logical_size();
        self.menu.open(&mut self.ui, &self.config, anchor, window, items);
    }

    // ---- find on page ----

    /// Opens the find bar out of the address field with the field focused, or focuses it
    /// again (selecting the text) if it is open.
    fn open_find(&mut self) {
        let Some(view) = self.active_view().filter(|_| !self.is_locked()) else { return };
        if self.overview.is_open() {
            return;
        }
        self.menu.close(&mut self.ui, &self.config);
        let anchor = self.chrome.address_frame(&self.ui);
        let width = self.logical_size().x;
        let top = self.page_rect().origin.y;
        self.find.open(&mut self.ui, &self.config, anchor, width, top, view);
        let events = self.ui.focus(self.find.field());
        self.handle_ui_events(events);
    }

    fn close_find(&mut self) {
        if !self.find.is_open() {
            return;
        }
        self.find.close(&mut self.ui, &self.config, &mut self.engine);
        if self.ui.focused() == Some(self.find.field()) {
            let events = self.ui.blur();
            self.handle_ui_events(events);
        }
        // Typing goes back to the page.
        if self.ui.focused().is_none()
            && let Some(view) = self.active_view()
        {
            self.engine.set_focus(view, true);
        }
    }

    fn find_action(&mut self, action: FindAction) {
        match action {
            FindAction::Previous | FindAction::Next => {
                self.find.step(&mut self.engine, action == FindAction::Next);
            }
            FindAction::ToggleMatchCase => {
                self.find.toggle_match_case(&mut self.ui, &mut self.config);
                self.config_dirty_since.get_or_insert_with(Instant::now);
            }
            FindAction::Close => return self.close_find(),
        }
        // The buttons take the focus away from the field; typing goes on there.
        self.refocus_find_field();
    }

    /// Puts the focus back into the find field, the caret at the end.
    fn refocus_find_field(&mut self) {
        let field = self.find.field();
        if self.ui.focused() == Some(field) {
            return;
        }
        let text = self.ui.field_text(field).unwrap_or_default().to_owned();
        let events = self.ui.focus(field);
        self.handle_ui_events(events);
        let end = text.len();
        self.ui.set_field_text_selecting(field, &text, end..end);
    }

    // ---- files dragged in ----

    /// Moves a file drag from the desktop along with the pointer (every frame while one is
    /// over the window) and finishes its drop.
    fn update_file_drop(&mut self) {
        if !self.file_drop.is_active() {
            return;
        }
        if self.pointer_probe.is_none() && self.file_drag_at.is_none() {
            self.pointer_probe = PointerProbe::connect(&self.window);
        }
        let pointer = self
            .file_drag_at
            .or_else(|| self.pointer_probe.as_ref().and_then(|p| p.position(&self.window)))
            .or(self.pointer);
        let page = self.page_rect();
        let target = pointer
            .filter(|&p| page.contains(p) && !self.ui.covers(p) && !self.is_locked())
            .and_then(|p| Some((self.active_view()?, p - page.origin)));
        let modifiers = input::engine_modifiers(self.modifiers);
        let outcome = self.file_drop.update(&mut self.engine, target, modifiers);
        if !self.file_drop.is_active() {
            self.file_drag_at = None;
        }
        if let Some(DropOutcome::OpenTabs(files)) = outcome
            && !self.is_locked()
        {
            // Dropped on the bar or the sidebar: each file opens in a tab of its own.
            for (i, path) in files.iter().enumerate() {
                let url = format!("file://{}", path.display());
                if let Err(e) = self.open_tab(&url, i == 0) {
                    tracing::error!("opening a dropped file failed: {e:#}");
                }
            }
        }
    }

    fn menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::SwitchAccount(i) => {
                if let Some(id) = self.accounts.all().get(i).map(|a| a.id.clone()) {
                    self.switch_account(&id);
                }
                return;
            }
            MenuAction::AddAccount => return self.open_add_account(),
            MenuAction::AccountSettings => return self.open_account_settings(),
            MenuAction::LockAccount => return self.lock_account(),
            MenuAction::Tab(command) => return self.tab_command(command),
            MenuAction::Bookmark(command) => return self.bookmark_command(command),
            MenuAction::Playlist(command) => return self.playlist_command(command),
            MenuAction::Page(command) => return self.page_command(command),
            _ => {}
        }
        let Some(tab) = self.tabs.active() else { return };
        let (url, title) = (tab.url.clone(), tab.label());
        match action {
            MenuAction::AddBookmark => {
                if let Some(store) = &self.store
                    && let Err(e) = store.add_bookmark(&url, &title, unix_now())
                {
                    tracing::error!("adding a bookmark failed: {e}");
                }
                self.bookmarks_changed = true;
            }
            MenuAction::RemoveBookmark => {
                if let Some(store) = &self.store
                    && let Ok(Some(id)) = store.bookmark_for(&url)
                    && let Err(e) = store.remove_bookmark(id)
                {
                    tracing::error!("removing a bookmark failed: {e}");
                }
                self.bookmarks_changed = true;
            }
            MenuAction::CopyLink => self.copy_link(),
            MenuAction::ZoomIn | MenuAction::ZoomOut | MenuAction::ZoomReset => {
                self.run(match action {
                    MenuAction::ZoomIn => Action::ZoomIn,
                    MenuAction::ZoomOut => Action::ZoomOut,
                    _ => Action::ZoomReset,
                });
                // The menu stays open: show the new level.
                if let Some(zoom) = self.tabs.active().map(|t| zoom_label(t.zoom)) {
                    self.menu.set_text(&mut self.ui, MenuAction::Zoom, &zoom);
                    self.menu.set_text(&mut self.ui, MenuAction::ZoomReset, &zoom);
                }
            }
            MenuAction::Zoom => {}
            MenuAction::HardReload => self.run(Action::HardReload),
            MenuAction::DevTools => {
                self.menu.close(&mut self.ui, &self.config);
                self.run(Action::DevTools);
            }
            MenuAction::FindOnPage => {
                self.menu.close(&mut self.ui, &self.config);
                self.run(Action::Find);
            }
            MenuAction::Settings => {
                // Out of the menu's Settings row (the menu's glass lies in the bar's plane).
                // The menu is gone by the time Settings closes, so it drains into the address
                // field, the basin the menu came from.
                let row = self.menu_click_frame.unwrap_or_else(|| self.menu.anchor());
                self.sheet_origin =
                    Some(Origin { plane: self.chrome.bar_node(), point: row.center(), control: row });
                self.run(Action::OpenSettings);
            }
            MenuAction::CheckForUpdates => {
                // Out of the menu's row, like Settings.
                let row = self.menu_click_frame.unwrap_or_else(|| self.menu.anchor());
                self.sheet_origin =
                    Some(Origin { plane: self.chrome.bar_node(), point: row.center(), control: row });
                self.menu.close(&mut self.ui, &self.config);
                self.open_about(true);
            }
            MenuAction::SwitchAccount(_)
            | MenuAction::AddAccount
            | MenuAction::AccountSettings
            | MenuAction::LockAccount
            | MenuAction::Tab(_)
            | MenuAction::Playlist(_)
            | MenuAction::Page(_)
            | MenuAction::Bookmark(_) => {}
        }
    }

    // ---- shields ----

    fn shields_info(&self) -> Option<ShieldsInfo> {
        let tab = self.tabs.active()?;
        Some(ShieldsInfo {
            host: display_host(&tab.url),
            level: self.shields.level(&tab.url),
            blocked_here: tab.blocked,
            blocked_total: self.shields.total(),
            lists_loading: !self.shields.is_ready(),
        })
    }

    /// Capture aid: closes every menu, popover and sheet at once (with `--lead` or
    /// `AXION_CAPTURE_FRAMES` it shows them on their way out).
    fn close_popups(&mut self) {
        self.menu.close(&mut self.ui, &self.config);
        self.context_menus.close(&mut self.ui, &self.config);
        self.account_menu.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.downloads_popover.close(&mut self.ui, &self.config);
        self.permissions.capture.close(&mut self.ui, &self.config);
        self.suggestions.hide(&mut self.ui, &self.config);
        self.passwords.hide_prompt(None, false);
        if self.settings.is_open() {
            self.close_settings();
        }
        if self.passwords.is_open() {
            self.close_passwords();
        }
    }

    // ---- moving tabs ----

    /// The normal (`false`) or private tab group.
    fn group(&self, private: bool) -> &Tabs {
        if private == self.private_mode { &self.tabs } else { &self.parked }
    }

    fn group_mut(&mut self, private: bool) -> &mut Tabs {
        if private == self.private_mode { &mut self.tabs } else { &mut self.parked }
    }

    /// A dragged tab was let go.
    fn tab_dropped(&mut self, item: TabItem, target: Option<Target>, lens: Option<Vec2>) {
        // Rows are rebuilt from the tabs either way (the drag only moved them for show).
        self.tabs_changed = true;
        let Some(target) = target else { return };
        let Some(from) = self.group(item.private).index_of(item.view) else { return };
        match crate::private::drop_into(item.private, target.private, self.private_locked()) {
            TabDrop::Reorder => {
                if self.group_mut(item.private).move_tab(from, target.index) {
                    tracing::info!(from, to = target.index, private = item.private, "tab moved");
                }
            }
            TabDrop::Reopen => {
                if let Some(view) = self.move_to_other_group(item.private, from, target.index)
                    && let Some(at) = lens
                {
                    self.tab_drag.land(MoveKey::Tab(view, Surface::Sidebar), at);
                }
            }
            TabDrop::NeedsUnlock => {
                // Into locked private tabs: once the password is entered.
                self.pending_move = Some((item.view, target.index));
                self.show_unlock_alert();
            }
        }
    }

    /// Moves tab `from` of one group to `index` of the other: the same address opens there
    /// (the private tabs' own cookies and logins, or the normal ones) and the original closes.
    /// A tab moved into private tabs leaves nothing behind (no closed-tab entry, and private
    /// tabs keep no history). The moved tab is shown. Returns its new view.
    fn move_to_other_group(&mut self, from_private: bool, from: usize, index: usize) -> Option<ViewId> {
        let to_private = !from_private;
        let tab = self.group_mut(from_private).take(from)?;
        if self.pip_view() == Some(tab.view) {
            self.end_pip(tab.view, None, false);
        }
        if self.now_playing == Some(tab.view) {
            self.now_playing = None;
        }
        self.engine.close_view(tab.view);
        let view = match self.engine.create_view(&tab.url, self.view_size(), to_private) {
            Ok(view) => view,
            Err(e) => {
                tracing::error!("reopening the tab in the other group failed: {e:#}");
                self.tabs_changed = true;
                return None;
            }
        };
        let mut moved = Tab::new(view, &tab.url);
        moved.title.clone_from(&tab.title);
        moved.private = to_private;
        let previous = self.active_view();
        let index = self.group_mut(to_private).insert_at(index, moved, true);
        tracing::info!(to_private, index, "tab moved to the other group");
        if to_private == self.private_mode {
            self.on_active_changed(previous);
        } else {
            self.toggle_private_mode();
        }
        self.tabs_changed = true;
        Some(view)
    }

    /// The drag's per-frame step before layout (the slot follows the pointer).
    fn update_tab_drag(&mut self, dt: f32) {
        self.tab_drag.update(&mut self.ui, &mut self.chrome, &self.config, dt);
    }

    fn toggle_shields_popover(&mut self) {
        if self.shields_popover.is_open() {
            self.shields_popover.close(&mut self.ui, &self.config);
            return;
        }
        self.menu.close(&mut self.ui, &self.config);
        let Some(info) = self.shields_info() else { return };
        let anchor = self.chrome.shield_frame(&self.ui);
        let width = self.logical_size().x;
        self.shields_popover.open(&mut self.ui, &self.config, anchor, width, &info);
    }

    /// Sets the active page's site level and reloads it, so the change shows at once.
    fn set_shields_level(&mut self, level: Level) {
        let Some(tab) = self.tabs.active() else { return };
        let (url, view) = (tab.url.clone(), tab.view);
        self.shields.set_level(self.store.as_ref(), &url, level, &self.config);
        self.engine.reload(view, false);
        self.tabs_changed = true;
        self.refresh_shields_popover();
    }

    /// Counts a blocked request once per page (pages often retry the same URL).
    fn count_blocked(&mut self, view: ViewId, url: String) {
        let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) else { return };
        if !tab.blocked_urls.insert(url) {
            return;
        }
        tab.blocked += 1;
        self.shields.count_blocked(self.store.as_ref());
        if Some(view) == self.active_view() {
            self.refresh_shields_popover();
        }
    }

    fn refresh_shields_popover(&mut self) {
        if !self.shields_popover.is_open() {
            return;
        }
        if let Some(info) = self.shields_info() {
            let anchor = self.chrome.shield_frame(&self.ui);
            let width = self.logical_size().x;
            self.shields_popover.update(&mut self.ui, &self.config, anchor, width, &info);
        }
    }

    fn apply_shields_config(&mut self) {
        self.shields.apply_config(&self.config);
        // Pages loaded from now on get the media bridge with the current setting.
        self.engine.set_page_script(Some(page_script(&self.config)));
    }

    /// Fetches the icons of the search engines the start page offers that aren't cached yet,
    /// so its engine menu shows them without the user having visited each engine.
    fn fetch_engine_icons(&mut self) {
        if self.engine_icons.is_some() {
            return;
        }
        let Some(dir) = self.favicons.dir().map(std::path::Path::to_path_buf) else { return };
        let hosts: Vec<String> = crate::settings::SEARCH_ENGINES
            .iter()
            .filter_map(|(_, url)| url.strip_prefix("https://")?.split('/').next())
            .filter(|host| {
                ![false, true].iter().any(|&dark| crate::favicons::cache_file(&dir, host, dark).exists())
            })
            .map(str::to_owned)
            .collect();
        if !hosts.is_empty() {
            self.engine_icons = Some(crate::engine_icons::fetch(hosts));
        }
    }

    /// Search engine icons that arrived: cached like any site's, shown on the start page.
    fn take_engine_icons(&mut self) {
        let Some(rx) = &self.engine_icons else { return };
        let icons: Vec<_> = rx.try_iter().collect();
        for icon in &icons {
            self.favicons.insert(&icon.site, &icon.url, icon.width, icon.height, &icon.rgba, true);
        }
        if !icons.is_empty() {
            self.home.icons_changed();
        }
    }

    /// While the Passwords sheet is open: shows the icons of its logins' sites, and fetches the
    /// ones that aren't cached (a few at a time, in the background).
    fn sync_password_icons(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let mut finished = false;
        if let Some(rx) = &self.password_icons {
            loop {
                match rx.try_recv() {
                    Ok(icon) => {
                        self.favicons.insert(
                            &icon.site,
                            &icon.url,
                            icon.width,
                            icon.height,
                            &icon.rgba,
                            true,
                        );
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.password_icons = None;
        }
        if !self.passwords.is_open() {
            return;
        }
        let missing = self.passwords.sync_icons(&mut self.ui, &self.config, &self.favicons);
        if !missing.is_empty() && self.password_icons.is_none() {
            self.password_icons = Some(crate::engine_icons::fetch(missing));
        }
    }

    fn edit_address(&mut self) {
        // Already editing: the click places the caret or selects inside the field (it must not
        // start over and select everything again).
        if self.ui.focused() == Some(self.chrome.address_field) {
            return;
        }
        if let Some(view) = self.active_view() {
            self.engine.set_focus(view, false);
        }
        // Editing starts from the shown tab's address: the field keeps its text while focused,
        // so a tab opened just now (new tab) would otherwise show the previous tab's.
        if let Some(url) = self.tabs.active().map(|t| t.url.clone()) {
            // On the start page the field starts empty, ready for typing.
            let text = if is_home_url(&url) { "" } else { url.as_str() };
            self.ui.set_field_text(self.chrome.address_field, text);
        }
        let events = self.chrome.set_editing(&mut self.ui, true);
        self.handle_ui_events(events);
    }

    fn navigate_from_address(&mut self, text: &str) {
        let Some(url) = resolve_input(text, &self.config.general.search_url) else { return };
        self.navigate_to(url);
    }

    fn navigate_to(&mut self, url: String) {
        self.close_playlist();
        self.suggestions.hide(&mut self.ui, &self.config);
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
        let _op = crate::stall::op("chrome events");
        for event in events {
            match event {
                UiEvent::Clicked { action, .. } if crate::eye::owns(action) => {
                    // The eye of a password field: keep typing where the focus was.
                    if let Some(field) = crate::eye::toggle(&mut self.ui, action) {
                        let events = self.ui.focus(field);
                        self.handle_ui_events(events);
                    }
                }
                event if self.onboarding.is_open() && self.onboarding.wants(&event) => {
                    self.onboarding_event(event);
                }
                event if self.passwords.wants(&event) => {
                    let (ui, config, clipboard) = (&mut self.ui, &mut self.config, &mut self.clipboard);
                    let outcome = self.passwords.event(ui, config, clipboard, self.pointer, event);
                    self.passwords_outcome(outcome);
                }
                UiEvent::Clicked { action, .. }
                    if self.playlist_open()
                        && crate::playlist_page::PageAction::from_id(action).is_some() =>
                {
                    if let Some(action) = crate::playlist_page::PageAction::from_id(action) {
                        self.playlist_ui.page.menu().close(&mut self.ui, &self.config);
                        self.playlist_action(action);
                    }
                }
                UiEvent::Clicked { action, .. } if self.playlist_ui.page.menu().owns(action) => {
                    let click = self.playlist_ui.page.menu().click(&mut self.ui, &self.config, action);
                    if let Some(MenuClick { action: MenuAction::Playlist(command), .. }) = click {
                        self.playlist_ui.page.menu().close(&mut self.ui, &self.config);
                        self.playlist_command(command);
                    }
                }
                UiEvent::Clicked { action, .. } if self.playlist_ui.popover.action(action).is_some() => {
                    if let Some(action) = self.playlist_ui.popover.action(action) {
                        self.playlist_popover_action(action);
                    }
                }
                UiEvent::Cancelled { node } if node == self.playlist_ui.page.search_field() => {
                    let events = self.playlist_ui.page.set_search(&mut self.ui, false);
                    self.handle_ui_events(events);
                }
                UiEvent::ValueChanged { node, value } if node == self.playlist_ui.page.scrubber() => {
                    self.playlist_seek(value);
                }
                UiEvent::ValueChanged { node, value } if node == self.playlist_ui.page.volume() => {
                    self.playlist_set_volume(value);
                }
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
                UiEvent::Clicked { action, .. } if FindBar::action(action).is_some() => {
                    if let Some(action) = FindBar::action(action) {
                        self.find_action(action);
                    }
                }
                UiEvent::Submitted { node, .. } if node == self.find.field() => {
                    // Enter: the next match; Shift+Enter: the previous one. The field keeps
                    // the focus.
                    self.find.step(&mut self.engine, !self.modifiers.shift);
                    self.refocus_find_field();
                }
                UiEvent::Cancelled { node } if node == self.find.field() => self.close_find(),
                UiEvent::Cancelled { node } if node == self.chrome.panel_search_field() => {
                    // Escape clears the list's search.
                    self.ui.set_field_text(node, "");
                }
                UiEvent::Submitted { node, .. } if node == self.chrome.panel_search_field() => {
                    // Enter opens the first match.
                    if self.panel.is_some_and(|(p, _)| p != Panel::Downloads)
                        && let Some(url) = self.panel_urls.first().cloned()
                    {
                        self.navigate_to(url);
                    }
                }
                UiEvent::Cancelled { node } if node == self.settings.search_field() => {
                    // Escape clears the settings search (and leaves the field).
                    self.ui.set_field_text(node, "");
                }
                UiEvent::Cancelled { node } if node == self.overview.search_field() => {
                    // Escape in the search field closes the search, not the overview.
                    self.overview_action(OverviewAction::ToggleSearch);
                }
                UiEvent::ValueChanged { node, value }
                    if self.overview.segment_action(node, value).is_some() =>
                {
                    if let Some(action) = self.overview.segment_action(node, value) {
                        self.overview_action(action);
                    }
                }
                UiEvent::Clicked { action, .. } if SettingsAction::from_id(action).is_some() => {
                    let Some(action) = SettingsAction::from_id(action) else { continue };
                    // The backdrop's "close" also fires for clicks on the sheet's background.
                    let on_sheet = self.pointer.is_some_and(|p| self.settings.contains(&self.ui, p));
                    if action == SettingsAction::Close && on_sheet {
                        continue;
                    }
                    let outcome = self.settings.action(&mut self.ui, action, &mut self.config, &self.keymap);
                    self.settings_outcome(outcome);
                }
                UiEvent::ValueChanged { node, value } if node == self.chrome.now_playing_scrubber() => {
                    if let Some(to) = self.chrome.now_playing_seek(value) {
                        self.now_playing_command(axion_media::Action::Seek(to));
                    }
                }
                UiEvent::ValueChanged { node, value } if node == self.player.scrubber() => {
                    if let Some((view, command)) = self.player.seek_command(value) {
                        self.media_command(view, &command);
                    }
                }
                UiEvent::Clicked { action, .. } if PlayerControl::from_id(action).is_some() => {
                    let Some(control) = PlayerControl::from_id(action) else { continue };
                    match self.player.click(&mut self.ui, control, &self.config) {
                        PlayerOutcome::Command(view, command) => self.media_command(view, &command),
                        PlayerOutcome::PictureInPicture(view, player) => self.start_pip(view, player),
                        PlayerOutcome::AddToPlaylist(view, player) => self.playlist_add_player(view, &player),
                        PlayerOutcome::Nothing => {}
                    }
                }
                UiEvent::ValueChanged { node, value } if node == self.player.volume() => {
                    if let Some((view, commands)) = self.player.volume_command(value) {
                        for command in commands {
                            self.media_command(view, &command);
                        }
                    }
                }
                UiEvent::ValueChanged { node, value } => {
                    let outcome = self.settings.value_changed(&mut self.ui, node, value, &mut self.config);
                    self.settings_outcome(outcome);
                }
                UiEvent::Submitted { node, .. } if Some(node) == self.settings.password_field() => {
                    let action = SettingsAction::SetPassword;
                    let outcome = self.settings.action(&mut self.ui, action, &mut self.config, &self.keymap);
                    self.settings_outcome(outcome);
                }
                UiEvent::Submitted { node, .. } if Some(node) == self.settings.trusted_site_field() => {
                    let action = SettingsAction::AddTrustedSite;
                    let outcome = self.settings.action(&mut self.ui, action, &mut self.config, &self.keymap);
                    self.settings_outcome(outcome);
                }
                UiEvent::Submitted { node, text } if Some(node) == self.settings.homepage_field() => {
                    self.config.general.set_homepage(&text);
                    self.settings_outcome(Outcome::Changed);
                }
                UiEvent::Clicked { action, .. } if LockScreen::is_unlock(action) => self.try_unlock(),
                UiEvent::Clicked { action, .. } if LockScreen::is_lock_action(action) => {
                    if LockScreen::is_cancel(action) {
                        // A tab waiting to move into the private ones stays where it is.
                        self.pending_move = None;
                    }
                    let events = self.lock_screen.action(&mut self.ui, &self.config, action);
                    self.handle_ui_events(events);
                }
                UiEvent::Submitted { node, .. } if node == self.lock_screen.field() => self.try_unlock(),
                UiEvent::Submitted { node, .. } if Some(node) == self.settings.account_name_field() => {
                    let action = SettingsAction::SaveAccountName;
                    let outcome = self.settings.action(&mut self.ui, action, &mut self.config, &self.keymap);
                    self.settings_outcome(outcome);
                }
                UiEvent::Submitted { node, .. } if Some(node) == self.settings.repository_field() => {
                    let action = SettingsAction::SaveRepository;
                    let outcome = self.settings.action(&mut self.ui, action, &mut self.config, &self.keymap);
                    self.settings_outcome(outcome);
                }
                UiEvent::Submitted { node, .. } if node == self.add_account.field() => {
                    self.add_account_action(SheetAction::Create);
                }
                UiEvent::Clicked { action, .. } if self.add_account.action(action).is_some() => {
                    let Some(action) = self.add_account.action(action) else { continue };
                    let on_sheet = self.pointer.is_some_and(|p| self.add_account.contains(&self.ui, p));
                    if action == SheetAction::Cancel && on_sheet {
                        continue;
                    }
                    self.add_account_action(action);
                }
                UiEvent::Clicked { action, .. } if self.update_alert.action(action).is_some() => {
                    let Some(action) = self.update_alert.action(action) else { continue };
                    let on_alert = self.pointer.is_some_and(|p| self.update_alert.contains(&self.ui, p));
                    if action == AlertAction::Backdrop && (on_alert || !self.update_alert.is_dismissable()) {
                        continue;
                    }
                    self.update_alert_action(action);
                }
                UiEvent::Clicked { action, .. } if self.toast.owns(action) => {
                    self.toast.dismiss(&mut self.ui, &self.config);
                    self.open_about(false);
                }
                UiEvent::Clicked { action, .. } if self.account_menu.owns(action) => {
                    if let Some(click) = self.account_menu.click(&mut self.ui, &self.config, action) {
                        if !click.keep_open {
                            self.account_menu.close(&mut self.ui, &self.config);
                        }
                        self.menu_action(click.action);
                    }
                }
                UiEvent::Clicked { action, .. } if self.context_menus.owns(action) => {
                    if let Some(click) = self.context_menus.click(&mut self.ui, &self.config, action) {
                        if !click.keep_open {
                            self.context_menus.close(&mut self.ui, &self.config);
                        }
                        self.menu_action(click.action);
                    }
                }
                UiEvent::Clicked { action, node } if self.menu.owns(action) => {
                    self.menu_click_frame = Some(self.ui.frame(node));
                    if let Some(MenuClick { action, keep_open }) =
                        self.menu.click(&mut self.ui, &self.config, action)
                    {
                        if !keep_open {
                            self.menu.close(&mut self.ui, &self.config);
                        }
                        self.menu_action(action);
                    }
                }
                UiEvent::Clicked { action, .. } if self.downloads_popover.action(action).is_some() => {
                    match self.downloads_popover.action(action) {
                        Some(DownloadsAction::Open(i)) => self.downloads.open_row(List::Popover, i),
                        Some(DownloadsAction::Button(i, k)) => {
                            self.downloads.row_button(List::Popover, i, k, &mut self.engine);
                        }
                        Some(DownloadsAction::ShowAll) => {
                            self.downloads_popover.close(&mut self.ui, &self.config);
                            self.show_all_downloads();
                        }
                        None => {}
                    }
                }
                UiEvent::Clicked { action, .. } if self.permissions.prompt.action(action).is_some() => {
                    if let Some(choice) = self.permissions.prompt.action(action) {
                        self.answer_permission_prompt(choice);
                    }
                }
                UiEvent::Clicked { action, .. } if self.permissions.capture.action(action).is_some() => {
                    if let Some(action) = self.permissions.capture.action(action) {
                        self.capture_action(action);
                    }
                }
                UiEvent::Clicked { action, .. } if self.shields_popover.action(action).is_some() => {
                    match self.shields_popover.action(action) {
                        Some(PopoverAction::SetLevel(level)) => self.set_shields_level(level),
                        Some(PopoverAction::OpenSettings) => {
                            self.shields_popover.close(&mut self.ui, &self.config);
                            self.settings.set_category(Category::Shields);
                            if !self.settings.is_open() {
                                let shield = self.chrome.shield_frame(&self.ui);
                                self.sheet_origin = Some(Origin {
                                    plane: self.chrome.bar_node(),
                                    point: shield.center(),
                                    control: shield,
                                });
                                self.run(Action::OpenSettings);
                            }
                        }
                        None => {}
                    }
                }
                UiEvent::Clicked { action, .. } if SuggestionList::row_action(action).is_some() => {
                    let url = SuggestionList::row_action(action)
                        .and_then(|i| self.suggestions.get(i))
                        .map(|s| s.url.clone());
                    if let Some(url) = url {
                        self.navigate_to(url);
                    }
                }
                UiEvent::Clicked { action, node } => match ChromeAction::from_id(action) {
                    Some(ChromeAction::ToggleSidebar) => self.run(Action::ToggleSidebar),
                    Some(ChromeAction::Back) => self.run(Action::Back),
                    Some(ChromeAction::Forward) => self.run(Action::Forward),
                    Some(ChromeAction::Home) => {
                        let home = self.config.general.effective_homepage().to_owned();
                        self.navigate_to(home);
                    }
                    Some(ChromeAction::ReloadOrStop) => {
                        if let Some(tab) = self.tabs.active() {
                            let (view, loading) = (tab.view, tab.loading);
                            if loading { self.engine.stop(view) } else { self.engine.reload(view, false) }
                        }
                    }
                    // Locked private browsing: nothing about the page, nothing new in it.
                    Some(ChromeAction::NewTab) if self.is_locked() => self.show_unlock_alert(),
                    Some(action) if self.is_locked() && !chrome_allowed_while_locked(action) => {}
                    Some(ChromeAction::EditAddress) => self.edit_address(),
                    Some(ChromeAction::NewTab) => self.run(Action::NewTab),
                    Some(ChromeAction::Overview) => self.run(Action::TabOverview),
                    Some(ChromeAction::Shield) => self.toggle_shields_popover(),
                    Some(ChromeAction::Share) => {
                        // Until the share sheet exists: copy the address.
                        self.copy_link();
                    }
                    // In private browsing, the normal tabs' row switches back to them.
                    Some(ChromeAction::ToggleTabsSection) if self.private_mode => self.toggle_private_mode(),
                    Some(ChromeAction::ToggleTabsSection) | Some(ChromeAction::CollapseTabs) => {
                        self.chrome.toggle_section(&mut self.ui, Section::Tabs)
                    }
                    Some(ChromeAction::CollapsePrivate) => {
                        self.chrome.toggle_section(&mut self.ui, Section::Private)
                    }
                    Some(ChromeAction::ToggleGroup(i)) => self.chrome.toggle_group(i),
                    Some(ChromeAction::ToggleSavedSection) => {
                        self.chrome.toggle_section(&mut self.ui, Section::Saved)
                    }
                    Some(ChromeAction::PageMenu) => self.open_page_menu(),
                    Some(ChromeAction::Bookmarks) => self.show_panel(Panel::Bookmarks),
                    Some(ChromeAction::History) => self.show_panel(Panel::History),
                    Some(ChromeAction::Passwords) => {
                        // The sheet comes out of the row (the sidebar is too big an origin).
                        let row = self.ui.frame(node);
                        self.sheet_origin = Some(Origin {
                            plane: self.chrome.sidebar_node(),
                            point: row.center(),
                            control: row,
                        });
                        self.open_passwords();
                    }
                    Some(ChromeAction::Account) => self.toggle_account_menu(),
                    Some(ChromeAction::ReadingList) => self.show_panel(Panel::ReadingList),
                    Some(ChromeAction::ClosePanel) => {
                        self.panel_urls.clear();
                        self.panel = None;
                        self.panel_query.clear();
                        self.chrome.set_panel_searchable(&mut self.ui, false);
                        self.chrome.close_panel(&mut self.ui);
                    }
                    Some(ChromeAction::ClearPanel) => self.clear_panel(),
                    Some(ChromeAction::DeletePanelItem(i)) => self.delete_panel_item(i),
                    Some(ChromeAction::Downloads) => self.show_panel(Panel::Downloads),
                    Some(ChromeAction::Playlist) => {
                        if self.playlist_open() {
                            self.close_playlist();
                        } else {
                            self.open_playlist();
                        }
                    }
                    Some(ChromeAction::PlaylistButton) => self.toggle_playlist_popover(),
                    Some(ChromeAction::DownloadsButton) => self.toggle_downloads_popover(),
                    Some(ChromeAction::PanelButton(i, k)) => {
                        self.downloads.row_button(List::Panel, i, k, &mut self.engine);
                        self.show_panel_with(Panel::Downloads, true);
                    }
                    Some(ChromeAction::OpenPanelItem(i))
                        if self.panel.is_some_and(|(p, _)| p == Panel::Downloads) =>
                    {
                        self.downloads.open_row(List::Panel, i);
                    }
                    Some(ChromeAction::OpenPanelItem(i)) => {
                        if let Some(url) = self.panel_urls.get(i).cloned() {
                            self.navigate_to(url);
                        }
                    }
                    // "Private" switches to the private tabs; there, it folds their list (or,
                    // locked, asks for the password).
                    Some(ChromeAction::Private) if !self.private_mode => self.toggle_private_mode(),
                    Some(ChromeAction::Private) if self.is_locked() => self.show_unlock_alert(),
                    Some(ChromeAction::Private) => self.chrome.toggle_section(&mut self.ui, Section::Private),
                    Some(ChromeAction::Capture) => self.toggle_capture_popover(),
                    Some(ChromeAction::MuteTab) => {
                        if let Some(tab) = self.tabs.active_mut() {
                            tab.muted = !tab.muted;
                            let (view, muted) = (tab.view, tab.muted);
                            self.engine.set_audio_muted(view, muted);
                            self.tabs_changed = true;
                        }
                    }
                    Some(ChromeAction::NowPlaying) => {
                        if let Some(view) = self.now_playing {
                            self.show_view(view);
                        }
                    }
                    Some(ChromeAction::NowPlayingToggle) => {
                        self.now_playing_command(axion_media::Action::Toggle)
                    }
                    Some(ChromeAction::NowPlayingPrevious) => self.now_playing_previous(),
                    Some(ChromeAction::NowPlayingNext) => {
                        self.now_playing_command(axion_media::Action::NextTrack)
                    }
                    Some(ChromeAction::SelectTab(i)) => self.select_tab(i),
                    Some(ChromeAction::SelectOtherTab(i)) => {
                        self.toggle_private_mode();
                        self.select_tab(i);
                    }
                    Some(ChromeAction::CloseTab(i)) => self.close_tab(i),
                    Some(ChromeAction::OpenBookmark(i)) => {
                        self.bookmark_command(crate::context_menu::BookmarkCommand::Open(i))
                    }
                    Some(ChromeAction::BookmarksOverflow) => self.toggle_bookmarks_overflow(),
                    None => {}
                },
                UiEvent::Submitted { node, text } if node == self.chrome.address_field => {
                    match self.suggestions.selected() {
                        Some(s) => self.navigate_to(s.url.clone()),
                        None => self.navigate_from_address(&text),
                    }
                }
                UiEvent::Cancelled { .. } => {
                    self.suggestions.hide(&mut self.ui, &self.config);
                    self.chrome.set_editing(&mut self.ui, false);
                    if let Some(view) = self.active_view() {
                        self.engine.set_focus(view, true);
                    }
                }
                UiEvent::FocusChanged { node: None } => {
                    self.suggestions.hide(&mut self.ui, &self.config);
                    self.chrome.set_editing(&mut self.ui, false);
                    self.tabs_changed = true;
                }
                UiEvent::Copy(text) => {
                    if let Some(clipboard) = &mut self.clipboard {
                        let _ = clipboard.set_text(text);
                    }
                }
                UiEvent::PasteRequested { node } => {
                    // The text arrives a moment later (see `take_field_pastes`).
                    let max = u64::from(self.config.clipboard.max_paste_mb) * 1024 * 1024;
                    self.field_paste.read(node, true, max);
                }
                _ => {}
            }
        }
    }

    // ---- engine ----

    /// Runs the engine and applies what happened. Call often (every loop iteration).
    pub fn pump(&mut self) {
        let _op = crate::stall::op("pump");
        let started = Instant::now();
        self.pump_engine();
        let page_frames = self.engine.counters().page_frames;
        if page_frames != self.pumped_page_frames {
            // A page frame came: shown at once, not one refresh after a skipped frame.
            self.pumped_page_frames = page_frames;
            self.idle_redraw_at = None;
        }
        let took = started.elapsed();
        if let Some(log) = &mut self.perf_log {
            log.wakeup(took);
        }
        if self.perf_log.as_ref().is_some_and(PerfLog::due) {
            let snapshot = self.perf_snapshot();
            if let Some(log) = &mut self.perf_log {
                log.log(&snapshot);
            }
        }
        let recorders = [
            self.resize_bench.as_mut().and_then(|b| b.recorder.as_mut()),
            self.resize_log.as_mut().map(|(r, _)| r),
        ];
        for recorder in recorders.into_iter().flatten() {
            recorder.wakeup(took);
        }
    }

    /// What the perf log reports about the window and pages (`AXION_PERF_LOG`).
    fn perf_snapshot(&self) -> crate::perf_log::Snapshot {
        let monitor = self.window.current_monitor();
        crate::perf_log::Snapshot {
            surface: (self.surface_config.width, self.surface_config.height),
            scale: self.window.scale_factor(),
            present_mode: self.present_mode.clone(),
            monitor: monitor.as_ref().and_then(|m| m.name()),
            refresh_mhz: monitor.as_ref().and_then(|m| m.refresh_rate_millihertz()),
            page_rate: self.engine.frame_rate(),
            window_fullscreen: self.window.fullscreen().is_some(),
            presentation: self.chrome.presentation(),
            page_fullscreen: self.chrome.page_fullscreen(),
            page_frame: self
                .display_view()
                .and_then(|v| self.engine.frame(v))
                .map(|f| (f.texture.width(), f.texture.height())),
            gpu_frames: self.engine.is_accelerated(),
            engine: self.engine.counters(),
            memory: self.engine.frame_memory(),
            skip_unchanged: self.config.ui.skip_unchanged_frames,
        }
    }

    /// The browser's and the engine's resize/frame counters (benchmarks).
    fn bench_counters(&self) -> ResizeCounters {
        let engine = self.engine.counters();
        ResizeCounters {
            view_resizes: engine.view_resizes,
            frame_textures: engine.frame_textures,
            check_buffers: engine.check_buffers,
            skipped_frames: self.frames_skipped,
            page_frames: engine.page_frames,
            ..self.resize_counters
        }
    }

    /// `--bench-resize` and `AXION_RESIZE_LOG`: advances the script, records the frame.
    fn bench_frame(&mut self) -> Result<()> {
        let counters = self.bench_counters();
        let times = std::mem::take(&mut self.frame_times);
        if let Some((recorder, last)) = &mut self.resize_log {
            recorder.frame(times);
            if last.elapsed() >= RESIZE_LOG_QUIET {
                recorder.log("window resize", counters);
                self.resize_log = None;
            }
        }
        let Some(bench) = &mut self.resize_bench else { return Ok(()) };
        if let Some(recorder) = &mut bench.recorder {
            recorder.frame(times);
        }
        match bench.tick(Instant::now(), counters) {
            resize_bench::Step::Finish => bail!(CaptureDone),
            resize_bench::Step::Resize(sizes) => {
                for size in sizes {
                    let _ = self.window.request_inner_size(winit::dpi::LogicalSize::new(size.x, size.y));
                }
            }
            resize_bench::Step::Nothing => {}
        }
        let scrolls = bench.phase().scrolls();
        if scrolls && self.wheel_frames == 0 {
            self.wheel_frames = u32::MAX / 2;
        } else if !scrolls && self.wheel_frames > 0 {
            self.wheel_frames = 0;
        }
        Ok(())
    }

    /// Clipboard text read for a chrome field: pasted if that field still has the focus.
    fn take_field_pastes(&mut self) {
        while let Some((node, data)) = self.field_paste.poll() {
            if let Some(text) = data.text.filter(|_| self.ui.focused() == Some(node)) {
                self.ui.paste(&text);
            }
        }
    }

    fn pump_engine(&mut self) {
        self.watch_refresh_rate();
        // The desktop switched light/dark (winit doesn't report it on most Linux desktops).
        let desktop_dark = self.desktop_theme.dark();
        if desktop_dark != self.desktop_dark {
            self.desktop_dark = desktop_dark;
            if self.config.appearance.theme == ThemeMode::Auto {
                self.apply_theme();
            }
        }
        self.take_field_pastes();
        while let Some((view, data)) = self.clipboard_reader.poll() {
            // Only into the page that is still shown (and not locked away meanwhile).
            if Some(view) == self.active_view() && !self.is_locked() {
                tracing::debug!(
                    text = data.text.is_some(),
                    files = data.files.len(),
                    "pasting into the page"
                );
                self.engine.paste(view, &data);
            }
        }
        self.update_file_drop();
        let mut downloads_changed = false;
        let events = {
            let _op = crate::stall::op("chromium pump");
            self.engine.pump()
        };
        let _op = crate::stall::op("engine events");
        let started = Instant::now();
        let count = events.len() as u32;
        if count > 0 {
            // Something happened in a page: the next frame runs at once.
            self.idle_redraw_at = None;
        }
        for event in events {
            match &event {
                // Autofill messages may hold a submitted password: never log their content.
                EngineEvent::PageMessage { view, message } if axion_vault::autofill::is_message(message) => {
                    tracing::debug!(?view, "engine: autofill message");
                }
                event => tracing::debug!(?event, "engine"),
            }
            match event {
                EngineEvent::AddressChanged { view, url } => {
                    // The fill offer was for the page that was there (a save offer stays).
                    self.passwords.hide_prompt(Some(view), true);
                    self.permissions_navigated(view, &url);
                    if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                        tab.url = url;
                        self.tabs_changed = true;
                    }
                }
                EngineEvent::TitleChanged { view, title } => {
                    if let (Some(store), Some(tab)) = (
                        &self.store,
                        find_tab(&self.tabs, &self.parked, view).filter(|t| t.records_history()),
                    ) && let Err(e) = store.set_title(&tab.url, &title)
                    {
                        tracing::warn!("saving a title failed: {e}");
                    }
                    if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                        tab.title = title;
                        self.tabs_changed = true;
                    }
                }
                EngineEvent::LoadingStateChanged { view, loading, can_go_back, can_go_forward } => {
                    if loading {
                        self.playlist_forget_sniffed(view);
                    }
                    if !loading
                        && let (Some(store), Some(tab)) = (
                            &self.store,
                            find_tab(&self.tabs, &self.parked, view).filter(|t| t.records_history()),
                        )
                        && let Err(e) = store.record_visit(&tab.url, &tab.title, unix_now())
                    {
                        tracing::warn!("saving history failed: {e}");
                    }
                    // Pages often add their icon links late: look once more when loading ends.
                    if !loading
                        && let Some(page) = find_tab(&self.tabs, &self.parked, view).map(|t| t.url.clone())
                        && let Some(icon) = self.favicons.page_loaded(&page)
                    {
                        tracing::debug!("fetching icon {icon}");
                        self.fetch_icon(view, &page, &icon);
                    }
                    if loading
                        && self.find.view() == Some(view)
                        && find_tab(&self.tabs, &self.parked, view).is_some_and(|t| !t.loading)
                    {
                        // Another page: Find on Page closes, as in Safari.
                        self.close_find();
                    }
                    if let Some(tab) = find_tab(&self.tabs, &self.parked, view) {
                        if loading && !tab.loading {
                            self.page_colors.page_started(view);
                        } else if !loading && tab.loading {
                            let delay = self.page_color_delay();
                            self.page_colors.page_loaded(view, delay);
                            if is_home_url(&tab.url) && !tab.private {
                                self.fetch_engine_icons();
                            }
                        }
                    }
                    if loading {
                        // Leaving a saved video's page keeps the position it reached.
                        self.playlist_flush_tab(view);
                    }
                    if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                        if loading && !tab.loading {
                            // A new page: its count starts over.
                            tab.blocked = 0;
                            tab.blocked_urls.clear();
                            tab.media = None;
                            tab.playing_media = false;
                        }
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
                    if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                        tab.progress = progress;
                        if Some(view) == self.active_view() && !self.is_locked() {
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
                        self.release_pointer_holds();
                        self.chrome.set_page_fullscreen(fullscreen);
                        // In F11 fullscreen the window stays fullscreen after the video.
                        let window = fullscreen || self.chrome.presentation();
                        self.window.set_fullscreen(window.then_some(Fullscreen::Borderless(None)));
                        self.update_layout();
                    }
                }
                // The Playlist's own views open no tabs.
                EngineEvent::NewViewRequested { opener, .. } if self.playlist_owns_view(opener) => {}
                EngineEvent::NewViewRequested { opener, url, kind, user_gesture, opener_frame } => {
                    let opener_url = find_tab(&self.tabs, &self.parked, opener).map(|t| t.url.clone());
                    // Axion's own pages open only from the browser or from each other.
                    if url.starts_with("axion:")
                        && !opener_url.as_deref().is_some_and(|u| u.starts_with("axion:"))
                    {
                        tracing::warn!(%url, "refused a page's new tab with an axion:// page");
                        continue;
                    }
                    if !self.shields.allow_popup(
                        opener_url.as_deref().unwrap_or_default(),
                        opener_frame.as_deref(),
                        &url,
                        user_gesture,
                    ) {
                        tracing::info!(%url, user_gesture, ?opener_frame, "blocked a popup");
                        self.count_blocked(opener, url);
                        continue;
                    }
                    // Ctrl/middle-click opens in the background unless the setting says otherwise.
                    let activate =
                        kind != NewViewKind::BackgroundTab || !self.config.tabs.open_links_in_background;
                    if let Err(e) = self.open_tab_from(opener, &url, activate) {
                        tracing::error!("opening {url} failed: {e:#}");
                    }
                }
                EngineEvent::FaviconUrlsChanged { view, urls } => {
                    if let Some(page) = find_tab(&self.tabs, &self.parked, view).map(|t| t.url.clone())
                        && let Some(icon) = self.favicons.icon_urls(&page, &urls)
                    {
                        tracing::debug!("fetching icon {icon}");
                        self.fetch_icon(view, &page, &icon);
                    }
                }
                EngineEvent::PageMetaChanged { view, meta } => {
                    let delay = self.page_color_delay();
                    self.page_colors.theme_colors(view, &meta.theme_colors, delay);
                    if let Some(page) = find_tab(&self.tabs, &self.parked, view).map(|t| t.url.clone())
                        && let Some(icon) = self.favicons.icon_links(&page, &meta.icons)
                    {
                        tracing::debug!("fetching icon {icon}");
                        self.fetch_icon(view, &page, &icon);
                    }
                }
                EngineEvent::InternalMessage { view, host, message } => {
                    self.internal_message(view, &host, &message);
                }
                EngineEvent::ContextMenu { view, params } => self.show_page_menu(view, params),
                EngineEvent::ImageReady { view, width, height, rgba, .. } => {
                    self.image_fetched(view, width, height, rgba);
                }
                EngineEvent::ImageFailed { view, .. } => self.image_failed(view),
                EngineEvent::FaviconFailed { view, url } if self.playlist_thumbnail_failed(view, &url) => {}
                EngineEvent::FaviconReady { view, url, width, height, rgba }
                    if self.playlist_thumbnail_ready(view, &url, (width, height), &rgba) => {}
                EngineEvent::FaviconFailed { view, url } => {
                    tracing::debug!("no usable icon at {url}");
                    if let Some(page) = find_tab(&self.tabs, &self.parked, view).map(|t| t.url.clone())
                        && let Some(icon) = self.favicons.failed(&page, &url)
                    {
                        tracing::debug!("fetching icon {icon}");
                        self.fetch_icon(view, &page, &icon);
                    }
                }
                EngineEvent::FaviconReady { view, url, width, height, rgba } => {
                    if let Some((page, private)) =
                        find_tab(&self.tabs, &self.parked, view).map(|t| (t.url.clone(), t.private))
                    {
                        // Private tabs leave no icon on disk.
                        self.favicons.insert(&page, &url, width, height, &rgba, !private);
                        if !private {
                            self.home.icons_changed();
                        }
                        self.tabs_changed = true;
                        if self.overview.is_open() {
                            self.rebuild_overview();
                        }
                    }
                }
                EngineEvent::PageMessage { view, message } => self.page_message(view, &message),
                EngineEvent::RequestBlocked { view, url } | EngineEvent::NavigationBlocked { view, url } => {
                    self.count_blocked(view, url);
                }
                EngineEvent::FindResult { view, active, count, final_update } => {
                    self.find.set_result(&mut self.ui, &mut self.engine, view, (active, count, final_update));
                }
                EngineEvent::MediaSniffed { view, url, mime, referer } => {
                    self.playlist_sniffed(view, url, mime, referer);
                }
                EngineEvent::PermissionRequest { view, id, origin, kinds } => {
                    self.permission_requested(view, id, origin, kinds);
                }
                EngineEvent::PermissionCancelled { view, id } => self.permission_cancelled(view, id),
                EngineEvent::CaptureChanged { view, audio, video } => {
                    self.capture_changed(view, audio, video)
                }
                EngineEvent::Closed { view } => {
                    self.permissions_closed(view);
                    self.playlist.sniffed.remove(&view);
                    self.page_colors.closed(view);
                }
                EngineEvent::DownloadUpdated(info) => {
                    let playlist_download = self.playlist_download_update(&info);
                    self.downloads.update(
                        info,
                        self.store.as_ref(),
                        &self.config,
                        unix_now(),
                        !playlist_download,
                    );
                    downloads_changed = true;
                }
                EngineEvent::SiteData(sites) => {
                    self.settings.set_site_data(Some(sites));
                    self.refresh_settings();
                }
            }
        }
        self.downloads_pump(downloads_changed);
        self.check_capture_stops();
        self.apply_permission_defaults();
        self.import_pump();
        self.playlist_housekeeping();
        self.update_home();
        self.update_photos();
        if count > 0 {
            let took = started.elapsed();
            let stats = &mut self.frame_stats;
            stats.events += count;
            stats.events_total += took;
            stats.events_worst = stats.events_worst.max(took);
        }
        let outcome = self.passwords.tick(&mut self.ui, &self.config, &mut self.clipboard);
        self.passwords_outcome(outcome);
        self.reload_config_if_changed();
        self.apply_lifecycle();
        // An unlock lasts for the session; only Lock locks again.
        self.sync_account_lock();
        self.save_session_if_due(false);
        self.handle_mpris();
    }

    fn settings_outcome(&mut self, outcome: Outcome) {
        let _op = crate::stall::op("settings apply");
        match outcome {
            Outcome::Nothing => {}
            Outcome::Changed => {
                // Apply at once; write to disk a moment later (sliders send many changes).
                self.config_dirty_since.get_or_insert(Instant::now());
                if self.settings.take_rebuild() {
                    self.refresh_settings();
                }
                self.engine.set_download_options(Downloads::options(&self.config));
                self.apply_shields_config();
                self.apply_theme();
                self.update_layout();
                self.tabs_changed = true;
                if self.playlist_open() {
                    self.playlist.reload(self.store.as_ref(), &self.config);
                }
            }
            Outcome::Close => self.close_settings(),
            Outcome::ShowWelcomeTour => self.open_onboarding(0),
            Outcome::CheckForUpdates => {
                self.updater.check(&self.config, true);
                self.refresh_about();
            }
            Outcome::InstallUpdate => self.request_update_install(),
            Outcome::OpenReleasePage => self.open_release_page(),
            Outcome::RelaunchNow => self.relaunch_for_update(),
            Outcome::CopyDiagnostics => self.copy_gpu_diagnostics(),
            Outcome::RestartAxion => self.restart_to_apply_settings(),
            Outcome::ConfirmGpuFrames => self.confirm_gpu_frames(),
            Outcome::SkipUpdate => {
                if let Some(release) = self.updater.available() {
                    axion_update::skip(&mut self.config.updates, &release.version);
                    self.config_dirty_since.get_or_insert(Instant::now());
                }
                self.refresh_about();
            }
            Outcome::RemindLater => {
                axion_update::remind_later(&mut self.config.updates, &axion_update::SystemClock);
                self.config_dirty_since.get_or_insert(Instant::now());
                self.refresh_about();
            }
            Outcome::SourceChanged => {
                self.config_dirty_since.get_or_insert(Instant::now());
                self.updater.reset();
                self.updater.check(&self.config, true);
                self.refresh_about();
            }
            Outcome::SetAccountPassword { current, new } => {
                let (ui, config) = (&mut self.ui, &self.config);
                let (result, done) = match &current {
                    Some(old) => {
                        (self.passwords.change_password(ui, config, old, &new), tr("Password changed."))
                    }
                    None => (self.passwords.set_up_password(ui, config, &new), tr("Password set.")),
                };
                let status = match result {
                    Ok(()) => {
                        if current.is_some() {
                            // A new password ends the private session (its tabs close).
                            self.close_private_tabs();
                        }
                        self.lock.unlock();
                        self.account_unlocked = true;
                        done.to_owned()
                    }
                    Err(axion_vault::VaultError::WrongPassword) => {
                        tr("The current password is wrong.").to_owned()
                    }
                    Err(e) => {
                        tracing::error!("setting the account password failed: {e}");
                        crate::passwords::describe(&e)
                    }
                };
                self.settings.set_account_status(Some(status));
                self.settings.set_account(self.account_info());
                self.update_lock();
                self.refresh_settings();
            }
            Outcome::RenameAccount(name) => {
                match self.accounts.rename(&self.account_id, &name) {
                    Ok(_) => self.show_account(),
                    Err(e) => tracing::error!("renaming the account failed: {e}"),
                }
                self.refresh_settings();
            }
            Outcome::AccountColor(color) => {
                if let Err(e) = self.accounts.set_color(&self.account_id, color) {
                    tracing::error!("saving the account colour failed: {e}");
                }
                self.show_account();
                self.refresh_settings();
            }
            Outcome::DeleteAccount => {
                let next = self.accounts.all().iter().find(|a| a.id != self.account_id).map(|a| a.id.clone());
                let Some(next) = next else { return };
                let id = self.account_id.clone();
                self.close_settings();
                self.switch_account(&next);
                match self.accounts.remove(&id) {
                    Ok(()) => {
                        tracing::info!(account = %id, "account deleted; its data goes at the next start")
                    }
                    Err(e) => tracing::error!("deleting the account failed: {e}"),
                }
            }
            Outcome::UpdateFilterLists => self.shields.update_lists(),
            Outcome::OpenPasswords => self.open_passwords(),
            Outcome::ImportPasswords(path) => {
                let status = self.passwords.import_csv(&path);
                self.settings.set_passwords_status(status);
                self.settings.show(&mut self.ui, &self.config, &self.keymap);
            }
            Outcome::ExportPasswords { path, password } => {
                let status = self.passwords.export(&path, &password);
                self.settings.set_passwords_status(status);
                self.settings.show(&mut self.ui, &self.config, &self.keymap);
            }
            Outcome::RestorePasswords { path, password } => {
                let (status, now_has_password) =
                    self.passwords.restore(&mut self.ui, &self.config, &path, &password);
                if now_has_password {
                    // The export's password became the account password, entered just now.
                    self.lock.unlock();
                    self.account_unlocked = true;
                    self.settings.set_account(self.account_info());
                    self.update_lock();
                }
                self.settings.set_passwords_status(status);
                self.settings.show(&mut self.ui, &self.config, &self.keymap);
            }
            Outcome::ClearHistory => {
                if let Some(store) = &self.store
                    && let Err(e) = store.clear_history_since(0)
                {
                    tracing::error!("clearing history failed: {e}");
                }
            }
            Outcome::DataPage => {
                self.downloads.measure_cache();
                self.engine.request_site_data();
                self.import_page_opened();
            }
            Outcome::Import(request) => self.import_request(request),
            Outcome::ChooseDownloadFolder => self.downloads.choose_folder(),
            Outcome::ChoosePlaylistFolder => self.downloads.choose_playlist_folder(),
            Outcome::ChooseBackground => self.home.choose_background(),
            Outcome::ChooseAccountPhoto => {
                self.settings.set_photo_status(None);
                self.photo_chooser.choose(PhotoFor::Current);
            }
            Outcome::RemoveAccountPhoto => {
                if let Err(e) = self.accounts.remove_avatar(&self.account_id) {
                    tracing::error!("removing the profile picture failed: {e}");
                }
                self.avatars.forget(&self.account_id);
                self.settings.set_photo_status(None);
                self.show_account();
                self.refresh_settings();
            }
            Outcome::AddBackground(path) => {
                if self.home.add_background(&path, &mut self.config) {
                    self.settings.set_background_status(None);
                    if let Some(field) = self.settings.background_path_field() {
                        self.ui.set_field_text(field, "");
                    }
                    self.settings_outcome(Outcome::Changed);
                } else {
                    let status =
                        self.home.add_error().unwrap_or(tr("That file can't be used as a background."));
                    self.settings.set_background_status(Some(status.to_owned()));
                }
                self.sync_backgrounds();
            }
            Outcome::RemoveBackground(name) => {
                if self.home.remove_background(&name, &mut self.config) {
                    self.settings_outcome(Outcome::Changed);
                }
                self.sync_backgrounds();
            }
            Outcome::ClearBrowsingData => {
                downloads::clear_browsing_data(
                    &self.config,
                    self.store.as_ref(),
                    &mut self.engine,
                    unix_now(),
                );
                self.settings_outcome(Outcome::DataPage);
            }
            Outcome::RemoveSiteData(site) => {
                let sites = site.map(|s| vec![s]).unwrap_or_default();
                let all = sites.is_empty();
                self.engine.clear_data(&axion_engine::ClearData { cache: false, all_site_data: all, sites });
                self.engine.request_site_data();
            }
            Outcome::ForgetPermissions(which) => self.forget_site_permissions(which),
        }
    }

    /// Downloads: the toolbar indicator, the open list, and background results (cache size,
    /// chosen folder).
    fn downloads_pump(&mut self, changed: bool) {
        let (shown, progress) = self.downloads.indicator();
        self.chrome.set_download_indicator(&mut self.ui, shown, progress);
        if changed && self.panel.is_some_and(|(p, _)| p == Panel::Downloads) {
            self.show_panel_with(Panel::Downloads, true);
        }
        if changed && self.downloads_popover.is_open() {
            self.open_downloads_popover();
        }
        for event in self.downloads.poll() {
            match event {
                downloads::Event::CacheSize(size) => self.settings.set_cache_size(Some(size)),
                downloads::Event::FolderChosen(folder) => {
                    self.config.downloads.directory = downloads::short_path(&folder);
                    self.settings_outcome(Outcome::Changed);
                }
                downloads::Event::PlaylistFolderChosen(folder) => {
                    self.config.playlist.directory = downloads::short_path(&folder);
                    self.settings_outcome(Outcome::Changed);
                }
            }
            self.refresh_settings();
        }
    }

    /// Copies the active page's address and says so in a toast out of the share button.
    fn copy_link(&mut self) {
        let url = self.tabs.active().map(|t| t.url.clone());
        let (Some(url), Some(clipboard)) = (url, &mut self.clipboard) else { return };
        if clipboard.set_text(url).is_ok() {
            tracing::info!("address copied");
            self.show_toast(Icon::Check, tr("Link copied"));
        }
    }

    /// A short glass notification out of the share button (the address field when the
    /// button is hidden, e.g. in a narrow window), over the page or the Home page alike.
    fn show_toast(&mut self, icon: Icon, text: &str) {
        let mut anchor = self.chrome.share_frame(&self.ui);
        if anchor.size.x <= 0.0 {
            anchor = self.chrome.address_frame(&self.ui);
        }
        let width = self.logical_size().x;
        self.toast.show(&mut self.ui, &self.config, anchor, width, icon, text);
    }

    /// A toast with a button, like [`Self::show_toast`].
    fn show_toast_with_action(&mut self, icon: Icon, text: &str, action: &str) {
        let mut anchor = self.chrome.share_frame(&self.ui);
        if anchor.size.x <= 0.0 {
            anchor = self.chrome.address_frame(&self.ui);
        }
        let width = self.logical_size().x;
        self.toast.show_with_action(&mut self.ui, &self.config, anchor, width, icon, text, Some(action));
    }

    /// Tooltips stay away while a menu, popover, sheet or drag is active.
    fn tooltips_blocked(&self) -> bool {
        self.tab_drag.is_dragging()
            || self.menu.is_open()
            || self.account_menu.is_open()
            || self.context_menus.is_open()
            || self.shields_popover.is_open()
            || self.downloads_popover.is_open()
            || self.permissions.prompt.is_open()
            || self.permissions.capture.is_open()
            || self.suggestions.is_visible()
            || self.find.is_open()
            || self.overview.is_open()
            || self.settings.is_open()
            || self.passwords.is_open()
            || self.add_account.is_open()
            || self.onboarding.is_open()
            || self.update_alert.is_open()
            || self.lock_screen.is_alert_open()
    }

    fn toggle_downloads_popover(&mut self) {
        if self.downloads_popover.is_open() {
            self.downloads_popover.close(&mut self.ui, &self.config);
        } else {
            self.shields_popover.close(&mut self.ui, &self.config);
            self.menu.close(&mut self.ui, &self.config);
            self.open_downloads_popover();
        }
    }

    /// Opens the downloads popover, or refreshes it.
    fn open_downloads_popover(&mut self) {
        let limit = crate::downloads_popover::ROWS;
        let items = self.downloads.items(List::Popover, self.store.as_ref(), &self.config, unix_now(), limit);
        let anchor = self.chrome.downloads_frame(&self.ui);
        let width = self.logical_size().x;
        self.downloads_popover.open(&mut self.ui, &self.config, anchor, width, &items);
    }

    /// The full Downloads list in the sidebar (opening the sidebar if needed).
    fn show_all_downloads(&mut self) {
        if !self.chrome.sidebar_open() {
            self.run(Action::ToggleSidebar);
        }
        self.show_panel(Panel::Downloads);
    }

    // ---- updates ----

    /// Gives Settings the current update state (and the badge on the account button).
    fn sync_about(&mut self) {
        if self.about_icon.is_none() {
            self.about_icon = crate::app_icon::image().map(|icon| self.avatars.upload(&icon));
        }
        let info = self.updater.about(&self.config, self.about_icon.clone());
        self.settings.set_about(&mut self.ui, info);
        self.update_phase = self.updater.phase();
        let badge = self.updater.should_badge(&self.config);
        self.chrome.set_update_badge(&mut self.ui, badge);
    }

    /// Like [`Self::sync_about`], and redraws the About page if it is showing.
    fn refresh_about(&mut self) {
        self.sync_about();
        if self.settings.is_open() && self.settings.category() == Category::About {
            self.settings.show(&mut self.ui, &self.config, &self.keymap);
        }
    }

    /// Opens Settings › About & Updates (checking for updates first, for the menu's row).
    fn open_about(&mut self, check: bool) {
        self.settings.set_category(Category::About);
        if check {
            self.updater.check(&self.config, true);
        }
        if self.settings.is_open() {
            self.refresh_about();
        } else {
            self.run(Action::OpenSettings);
        }
    }

    /// Runs the startup check when it is due, and takes what the update threads reported.
    fn poll_updates(&mut self) {
        if self.update_check_at.is_some_and(|at| Instant::now() >= at) {
            self.update_check_at = None;
            self.updater.check(&self.config, false);
            self.refresh_about();
        }
        let notices = self.updater.poll();
        if notices.is_empty() {
            return;
        }
        let mut checked = false;
        for notice in notices {
            match notice {
                Notice::Checked { manual, ok, update } => {
                    checked = true;
                    if ok {
                        axion_update::record_check(&mut self.config.updates, &axion_update::SystemClock);
                        self.config_dirty_since.get_or_insert(Instant::now());
                    }
                    // Once per version, and never for a skipped one or while reminded later.
                    if let Some(release) = update
                        && !manual
                        && axion_update::may_notify(
                            &self.config.updates,
                            &release,
                            &axion_update::SystemClock,
                        )
                        && self.config.updates.notified_version != release.version.to_string()
                    {
                        self.config.updates.notified_version = release.version.to_string();
                        self.config_dirty_since.get_or_insert(Instant::now());
                        let text =
                            tr_fmt!("Axion {version} is available", version = release.version.to_string());
                        self.show_toast_with_action(Icon::Download, &text, tr("Update"));
                    }
                }
                Notice::Changed => self.sync_update_alert(),
            }
        }
        // The page redraws when the install starts or ends, not for every progress line.
        if checked || self.updater.phase() != self.update_phase {
            self.refresh_about();
        }
    }

    /// Shows the alert for the install's current state (or closes it).
    fn sync_update_alert(&mut self) {
        match self.updater.current_alert() {
            Some(view) => self.update_alert.show(&mut self.ui, &self.config, view),
            None => self.update_alert.close(),
        }
    }

    /// "Update": asks first (glass alert); a package-managed install just opens the page.
    fn request_update_install(&mut self) {
        if self.updater.mode() == InstallMode::Other {
            self.open_release_page();
            return;
        }
        if let Some(view) = self.updater.prepare_install() {
            self.update_alert.show(&mut self.ui, &self.config, view);
        }
    }

    /// The available release's page in a new tab.
    fn open_release_page(&mut self) {
        let Some(url) = self.updater.release_page() else { return };
        if self.settings.is_open() {
            self.close_settings();
        }
        if let Err(e) = self.open_tab(&url, true) {
            tracing::error!("opening the release page failed: {e:#}");
        }
    }

    /// Settings › Hardware: copies the GPU summary (adapter, driver, frame mode, VA-API, CEF).
    fn copy_gpu_diagnostics(&mut self) {
        let text = self.hardware_status.diagnostics();
        let Some(clipboard) = &mut self.clipboard else { return };
        if clipboard.set_text(text).is_ok() {
            self.show_toast(Icon::Check, tr("Diagnostics copied"));
        }
    }

    /// Writes the settings now and starts Axion again (the Hardware settings are read at start).
    fn restart_to_apply_settings(&mut self) {
        self.config_dirty_since = None;
        match axion_config::save_config(&self.paths, &self.config) {
            Ok(()) => self.last_saved_config = std::fs::read_to_string(&self.paths.config).ok(),
            Err(e) => {
                tracing::error!("saving settings failed: {e}");
                return;
            }
        }
        let account = self.account_id.clone();
        self.relaunch_into(&account);
    }

    /// "Shared GPU memory" on AMD graphics asks first: it has crashed the driver there.
    fn confirm_gpu_frames(&mut self) {
        self.gpu_confirm = true;
        self.update_alert.show(
            &mut self.ui,
            &self.config,
            AlertView {
                icon: Some(Icon::Info),
                title: tr("Use shared GPU memory on AMD graphics?").to_owned(),
                lines: vec![
                    tr("On AMD graphics (RADV) this can crash the graphics driver and freeze the screen \
                        until it recovers. Pages may also turn black.")
                    .to_owned(),
                    tr("Only continue if you know that it works on this computer. It applies after \
                        restarting Axion.")
                    .to_owned(),
                ],
                primary: Some((tr("Use Anyway").to_owned(), Tone::Plain)),
                secondary: Some(tr("Cancel").to_owned()),
                dismissable: true,
                ..Default::default()
            },
        );
    }

    fn update_alert_action(&mut self, action: AlertAction) {
        if self.gpu_confirm {
            self.gpu_confirm = false;
            self.update_alert.close();
            if action == AlertAction::Primary {
                self.config.hardware.page_frames = axion_config::PageFrames::Gpu;
                self.settings_outcome(Outcome::Changed);
                self.refresh_settings();
            }
            return;
        }
        match self.updater.phase() {
            Phase::Idle => self.update_alert.close(),
            Phase::Confirm => {
                if action == AlertAction::Primary {
                    self.updater.start_install(&self.config);
                    self.sync_update_alert();
                    self.refresh_about();
                } else {
                    self.updater.dismiss();
                    self.update_alert.close();
                }
            }
            Phase::Running => {
                if action == AlertAction::Secondary {
                    self.updater.cancel();
                }
            }
            Phase::Done => {
                if action == AlertAction::Primary {
                    self.relaunch_for_update();
                } else {
                    // The page keeps offering "Relaunch Now".
                    self.update_alert.close();
                }
            }
            Phase::Failed => {
                self.updater.dismiss();
                self.update_alert.close();
                self.refresh_about();
            }
        }
    }

    /// Starts the updated Axion in this one's place (the window closes, the session is saved
    /// and Chromium shuts down first, as for an account switch).
    fn relaunch_for_update(&mut self) {
        let Some(exe) = self.updater.relaunch_exe() else { return };
        let size = self.window.inner_size().to_logical::<f64>(self.window.scale_factor());
        self.relaunch = Some(Relaunch {
            account: self.account_id.clone(),
            size: (size.width, size.height),
            position: self.window.outer_position().ok().map(|p| (p.x, p.y)),
            maximized: self.window.is_maximized(),
            capture: None,
            exe: Some(exe),
        });
        tracing::info!("relaunching into the updated Axion");
    }

    /// A point inside the release notes box (capture aid), if the page has one.
    fn settings_notes_point(&self) -> Option<Vec2> {
        self.settings.notes_point(&self.ui)
    }

    /// Rebuilds the open settings page (after data it shows arrived).
    fn refresh_settings(&mut self) {
        if self.settings.is_open() {
            self.settings.show(&mut self.ui, &self.config, &self.keymap);
        }
    }

    fn save_config_if_due(&mut self) {
        if self.config_dirty_since.is_some_and(|t| t.elapsed() >= CONFIG_SAVE_DELAY) {
            self.config_dirty_since = None;
            match axion_config::save_config(&self.paths, &self.config) {
                Ok(()) => self.last_saved_config = std::fs::read_to_string(&self.paths.config).ok(),
                Err(e) => tracing::error!("saving settings failed: {e}"),
            }
        }
    }

    fn reload_config_if_changed(&mut self) {
        self.save_config_if_due();
        let Some(watcher) = &self.watcher else { return };
        let changed = watcher.poll();
        // Only read the file when it changed: this runs on every loop iteration.
        let own_write = changed.config
            && self.last_saved_config.is_some()
            && std::fs::read_to_string(&self.paths.config).ok() == self.last_saved_config;
        if changed.config && !own_write && self.config_dirty_since.is_none() {
            match axion_config::load_config(&self.paths) {
                Ok(config) => {
                    tracing::info!("settings reloaded");
                    self.config = config;
                    self.engine.set_download_options(Downloads::options(&self.config));
                    self.apply_shields_config();
                    self.apply_theme();
                    self.update_layout();
                    self.sync_frame_rate();
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

    /// How long after loading a page's colour is measured.
    fn page_color_delay(&self) -> Duration {
        Duration::from_secs_f32(self.config.appearance.page_color_delay_ms.max(0.0) / 1000.0)
    }

    /// Tints the window around the page with the page's colour (like Safari) and flips the
    /// chrome light/dark to stay legible on it. The colour is one per page (see `page_color`),
    /// measured on the frame once after loading, so scrolling never changes it.
    fn adapt_appearance(&mut self, page: Option<&wgpu::TextureView>, dt: f32) {
        let params = TintParams::from_config(&self.config.appearance);
        if let Some(cells) = self.compositor.take_palette() {
            self.page_colors.measured(&cells, &params);
        }
        let appearance = &self.config.appearance;
        let adapt = appearance.adapt_to_page && appearance.theme == ThemeMode::Auto;
        let tints = appearance.tint_from_page && appearance.tint_mode != axion_config::TintMode::Neutral;
        let view = self.active_view().filter(|_| !self.is_locked());
        if let (Some(view), Some(page)) = (view, page)
            && (tints || adapt)
            && self.page_colors.wants_measure(view)
            && self.compositor.sample_palette(page)
        {
            self.page_colors.measuring(view);
        }
        let color = view.and_then(|v| self.page_colors.color(v, &params));
        let spring = crate::drip::spring(&self.config, self.config.motion.smooth);
        let tint = color.filter(|_| tints);
        self.window_tint = self.page_colors.step(tint, spring, dt);
        let Some(color) = color.filter(|_| adapt) else { return };
        let dark = lightness(color) < DARK_BELOW;
        if dark != self.dark {
            self.dark = dark;
            self.ui.set_theme(Theme::new(&self.config, dark || self.private_mode));
        }
    }

    // ---- profile pictures ----

    /// Takes a picture chosen in the file chooser: copies it into the account's directory
    /// (current account) or keeps it for the account the sheet creates.
    fn update_photos(&mut self) {
        let Some((target, path)) = self.photo_chooser.poll() else { return };
        self.apply_photo(target, &path);
    }

    fn apply_photo(&mut self, target: PhotoFor, path: &std::path::Path) {
        let (size, max_mb) = (self.config.accounts.avatar_size_px, self.config.accounts.avatar_max_mb);
        let picture = match axion_accounts::load_avatar(path, size, max_mb) {
            Ok(picture) => picture,
            Err(e) => {
                tracing::warn!(path = %path.display(), "profile picture refused: {e}");
                let text = match e {
                    axion_accounts::AccountsError::AvatarTooLarge { .. }
                    | axion_accounts::AccountsError::AvatarUnreadable => e.to_string(),
                    _ => tr("That picture couldn't be read.").to_owned(),
                };
                match target {
                    PhotoFor::NewAccount => self.add_account.show_error(&mut self.ui, &text),
                    PhotoFor::Current => {
                        self.settings.set_photo_status(Some(text.clone()));
                        self.refresh_settings();
                        if self.onboarding.is_open() {
                            self.onboarding.show_error(&mut self.ui, &self.config, &text);
                        }
                    }
                }
                self.show_toast(Icon::Close, &text);
                return;
            }
        };
        match target {
            PhotoFor::NewAccount => {
                let image = self.avatars.upload(&picture);
                self.add_account.set_photo(&mut self.ui, Some(&image));
                self.new_account_photo = Some(picture);
            }
            PhotoFor::Current => {
                match self.accounts.set_avatar(&self.account_id, &picture) {
                    Ok(()) => {
                        self.settings.set_photo_status(None);
                    }
                    Err(e) => {
                        tracing::error!("saving the profile picture failed: {e}");
                        self.settings.set_photo_status(Some(tr("The picture couldn't be saved.").to_owned()));
                    }
                }
                self.avatars.forget(&self.account_id);
                self.show_account();
                self.refresh_settings();
            }
        }
    }

    // ---- the start page ----

    /// Keeps the start page's data current; open start pages pull it again when it changed.
    fn update_home(&mut self) {
        if self.home.poll_chooser(&mut self.config) {
            self.config_dirty_since.get_or_insert(Instant::now());
            self.refresh_settings();
        }
        // A file chooser pick (or the start page's path field) can fail too: Settings says why.
        let add_error = self.home.add_error().map(str::to_owned);
        self.settings.sync_background_status(add_error);
        self.update_background_picker();
        let name = self.account().map(|a| a.name.clone()).unwrap_or_default();
        let dark = is_dark(&self.config, &self.window, self.desktop_theme.dark());
        if self.home.update(&self.config, self.store.as_ref(), self.shields.total(), &name, dark) {
            self.refresh_home_pages();
        }
    }

    /// Keeps Settings' background picker current: its cards (once their thumbnails arrive,
    /// or when backgrounds were added or removed) and the selected one's blurred preview.
    fn update_background_picker(&mut self) {
        let arrived = self.home.poll_thumbnails();
        if !self.settings.is_open() {
            return;
        }
        if !self.home_thumbnails {
            // Textures only once Settings shows them.
            self.home_thumbnails = true;
            self.home.set_gpu(crate::home::Textures::new(self.device.clone(), self.queue.clone()));
        }
        let generation = self.home.backgrounds_generation();
        if arrived.thumbs || self.backgrounds_listed != Some(generation) {
            self.sync_backgrounds();
            return;
        }
        // Asks for a new preview when the blur or the selection changed.
        let preview = self.home.background_preview(&self.config, self.logical_size().x);
        if arrived.preview {
            self.settings.set_background_preview(&mut self.ui, preview);
        }
    }

    /// Lists the backgrounds in Settings again and rebuilds its page.
    fn sync_backgrounds(&mut self) {
        self.backgrounds_listed = Some(self.home.backgrounds_generation());
        self.settings.set_backgrounds(self.home.background_cards());
        let preview = self.home.background_preview(&self.config, self.logical_size().x);
        self.settings.set_background_preview(&mut self.ui, preview);
        self.refresh_settings();
    }

    fn refresh_home_pages(&mut self) {
        let views: Vec<ViewId> = self
            .tabs
            .all()
            .iter()
            .chain(self.parked.all())
            .filter(|t| is_home_url(&t.url))
            .map(|t| t.view)
            .collect();
        for view in views {
            if !self.engine.is_discarded(view) {
                self.engine.run_script(view, REFRESH_SCRIPT, false);
            }
        }
    }

    /// A message from one of Axion's own pages (the engine made sure it came from one).
    fn internal_message(&mut self, view: ViewId, host: &str, message: &str) {
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return };
        if host != "home" || !is_home_url(&tab.url) {
            tracing::debug!(host, "message from an axion:// page that isn't shown");
            return;
        }
        let private = tab.private;
        // Private start pages may search, nothing else: no settings, no history.
        let mut scratch = self.config.clone();
        let (config, store) =
            if private { (&mut scratch, None) } else { (&mut self.config, self.store.as_ref()) };
        let action = self.home.handle(message, config, store);
        match action {
            HomeAction::Nothing => {}
            HomeAction::Go(text) => {
                if let Some(url) = resolve_input(&text, &self.config.general.search_url)
                    && let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view)
                {
                    tab.url.clone_from(&url);
                    self.engine.navigate(view, &url);
                    self.engine.set_focus(view, true);
                    self.tabs_changed = true;
                }
            }
            HomeAction::ConfigChanged if !private => {
                self.config_dirty_since.get_or_insert(Instant::now());
                self.refresh_settings();
            }
            HomeAction::Refresh if !private => self.update_home(),
            HomeAction::OpenSettings if !private => {
                self.settings.set_category(Category::General);
                if self.settings.is_open() {
                    self.refresh_settings();
                } else {
                    self.run(Action::OpenSettings);
                }
            }
            HomeAction::ConfigChanged | HomeAction::Refresh | HomeAction::OpenSettings => {}
        }
    }

    /// Has Chromium download a site icon. SVGs go through `svg_icons` first: their
    /// `prefers-color-scheme` rules are decided for the chrome.
    fn fetch_icon(&mut self, view: ViewId, page: &str, url: &str) {
        if self.favicons.is_svg(page, url) {
            self.svg_icons.request(view, url, self.favicons.is_dark());
        } else {
            self.engine.fetch_favicon(view, url, FETCH_SIZE);
        }
    }

    /// SVG icons that were rewritten: Chromium rasterises them now.
    fn poll_svg_icons(&mut self) {
        for icon in self.svg_icons.poll() {
            if icon.fetch != icon.url {
                self.favicons.alias(&icon.fetch, &icon.url);
            }
            self.engine.fetch_favicon(icon.view, &icon.fetch, FETCH_SIZE);
        }
    }

    fn apply_theme(&mut self) {
        if let Some(pip) = &mut self.pip {
            pip.set_theme(&self.config);
        }
        self.dark = is_dark(&self.config, &self.window, self.desktop_theme.dark());
        // Pages follow the chosen scheme live (not the page-adaptive chrome flip below).
        self.engine.set_color_scheme(self.dark);
        let delay = self.page_color_delay();
        self.page_colors.set_dark(self.dark, self.active_view(), delay);
        let contrast_changed = self.favicons.set_contrast(self.config.appearance.favicon_contrast);
        if self.favicons.set_dark(self.dark) {
            // Sites with their own icon for the scheme show it at once.
            let pages: Vec<(ViewId, String)> = self
                .tabs
                .all()
                .iter()
                .chain(self.parked.all())
                .filter(|t| !self.engine.is_discarded(t.view))
                .map(|t| (t.view, t.url.clone()))
                .collect();
            for (view, page) in pages {
                if let Some(icon) = self.favicons.resolve(&page) {
                    self.fetch_icon(view, &page, &icon);
                }
            }
            self.tabs_changed = true;
            if self.overview.is_open() {
                self.rebuild_overview();
            }
        } else if contrast_changed {
            self.tabs_changed = true;
            if self.overview.is_open() {
                self.rebuild_overview();
            }
        }
        self.update_lock();
        // Private browsing is always dark, like Safari's.
        self.ui.set_theme(Theme::new(&self.config, self.dark || self.private_mode));
    }

    // ---- window events ----

    /// Returns `false` when the window should close.
    pub fn window_event(&mut self, event: WindowEvent) -> Result<bool> {
        let _op = crate::stall::op(match &event {
            WindowEvent::RedrawRequested => "frame",
            WindowEvent::MouseInput { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseWheel { .. } => "input: pointer",
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => "input: keyboard",
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => "window resize",
            WindowEvent::Focused(_) => "window focus",
            _ => "window event",
        });
        if !matches!(event, WindowEvent::RedrawRequested) {
            // Input: the next frame runs at once.
            self.idle_redraw_at = None;
        }
        match event {
            WindowEvent::CloseRequested => return Ok(false),
            WindowEvent::Resized(size) => {
                self.resize_counters.events += 1;
                if self.resize_log_enabled {
                    let counters = self.bench_counters();
                    let log =
                        self.resize_log.get_or_insert_with(|| (Recorder::new(counters), Instant::now()));
                    log.1 = Instant::now();
                }
                // Applied once, by the next frame: a resize sends a stream of these (dozens
                // between two frames when the window manager catches up), and reconfiguring
                // the swapchain and laying everything out for each one froze the window.
                if size.width > 0 && size.height > 0 {
                    self.pending_window_size = Some(size);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.update_layout();
                self.sync_frame_rate();
            }
            // Maybe onto another monitor, with another refresh rate.
            WindowEvent::Moved(_) => self.sync_frame_rate(),
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
                // A held slider keeps following the pointer outside the window (the compositor
                // grabs it, the button-up still arrives), so the pointer position stays.
                if self.ui.dragging().is_none() {
                    self.pointer = None;
                    self.ui.handle(UiInput::PointerLeave);
                    self.leave_page();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.tooltip.cancel(&mut self.ui);
                self.mouse_button(state, button)
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.tooltip.cancel(&mut self.ui);
                if self.context_menus.page_open() {
                    // Scrolling the page puts its menu away.
                    self.context_menus.close(&mut self.ui, &self.config);
                }
                let delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x, y) * LINE_SCROLL,
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32) / self.scale(),
                };
                if self.overview.is_open() {
                    self.overview.scroll_by(&mut self.ui, delta.y);
                } else if self.passwords.is_open() {
                    self.passwords.scroll_by(&mut self.ui, delta.y);
                } else if self.settings.is_open() {
                    if !self.settings.scroll_notes(&mut self.ui, delta.y, self.pointer) {
                        self.settings.scroll_by(&mut self.ui, delta.y);
                    }
                } else if self.playlist_open()
                    && self.pointer.is_some_and(|p| self.playlist_ui.page.list_contains(&self.ui, p))
                {
                    self.playlist_ui.page.scroll_by(&mut self.ui, delta.y);
                } else if self.playlist_open()
                    && self.pointer.is_some_and(|p| self.playlist_ui.page.rect().contains(p))
                {
                    // The page below the Playlist rests.
                } else if self.pointer.is_some_and(|p| self.chrome.panel_contains(&self.ui, p)) {
                    self.chrome.scroll_panel(&mut self.ui, delta.y);
                } else if self.pointer.is_some_and(|p| self.chrome.sections_contains(&self.ui, p)) {
                    self.chrome.scroll_sections(&mut self.ui, delta.y);
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
                if !focused {
                    // The button-up may go to another window: nothing stays held.
                    self.release_pointer_holds();
                }
                if let Some(view) = self.active_view()
                    && self.ui.focused().is_none()
                    && !self.is_locked()
                {
                    self.engine.set_focus(view, focused);
                }
            }
            WindowEvent::RedrawRequested => self.redraw()?,
            WindowEvent::HoveredFile(path) => self.file_drop.hovered(path),
            WindowEvent::HoveredFileCancelled => self.file_drop.cancelled(),
            WindowEvent::DroppedFile(path) => self.file_drop.dropped(path),
            _ => {}
        }
        Ok(true)
    }

    fn pointer_moved(&mut self, p: Vec2) {
        self.pointer = Some(p);
        if self.tab_drag.is_dragging() {
            // The dragged tab follows (see `update_tab_drag`); nothing else reacts meanwhile.
            let (chrome, favicons) = (&self.chrome, &self.favicons);
            self.tab_drag.pointer_moved(&mut self.ui, chrome, &self.config, None, favicons, p);
            return;
        }
        let pressed_tab = self.tab_drag_tab();
        let tab = pressed_tab.and_then(|view| find_tab(&self.tabs, &self.parked, view).cloned());
        let (chrome, favicons) = (&self.chrome, &self.favicons);
        if self.tab_drag.pointer_moved(&mut self.ui, chrome, &self.config, tab.as_ref(), favicons, p) {
            // The press became a drag: no click when it ends.
            self.ui.cancel_press();
            self.leave_page();
            return;
        }
        // Fullscreen controls that faded out come back before the pointer reaches them, so a
        // press right after the move already hits them.
        self.player.reveal(&mut self.ui);
        let (over_ui, events) = self.ui.handle(UiInput::PointerMove(p));
        self.handle_ui_events(events);
        if self.playlist_open() {
            // The Playlist's video area is a page of its own; the one below rests.
            self.playlist_pointer(p, None);
            self.update_cursor();
            return;
        }
        if over_ui || self.pointer_captured_by_ui || self.is_locked() {
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
            // Fullscreen video: the pointer hides with the controls.
            None if self.player.mode() == PlayerMode::Fullscreen && !self.player.is_shown() => None,
            None => input::cursor_icon(self.page_cursor),
        };
        self.window.set_cursor_visible(icon.is_some());
        if let Some(icon) = icon {
            self.window.set_cursor(icon);
        }
    }

    /// Lets go of whatever the pointer holds in the UI (a slider drag, a held glass button)
    /// without a click: the button-up got lost (focus lost, Esc, view closed, fullscreen left).
    fn release_pointer_holds(&mut self) {
        if !self.ui.is_holding() || self.tab_drag.is_dragging() {
            return;
        }
        let events = self.ui.end_interaction();
        self.pointer_captured_by_ui = false;
        self.tab_drag.release_press();
        self.handle_ui_events(events);
    }

    fn mouse_button(&mut self, state: ElementState, button: winit::event::MouseButton) {
        let pressed = state.is_pressed();
        let Some(p) = self.pointer else {
            // A button-up without a known pointer (it left the window while held).
            if !pressed && button == winit::event::MouseButton::Left {
                self.release_pointer_holds();
            }
            return;
        };
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

        if pressed && self.lock_screen.is_alert_open() && !self.lock_screen.alert_contains(&self.ui, p) {
            // A click outside the password card only dismisses it.
            self.lock_screen.close_alert(&mut self.ui);
            self.pending_move = None;
            return;
        }
        if pressed && self.menu.is_open() && !self.menu.contains(&self.ui, p) {
            // A click outside only dismisses the menu.
            self.menu.close(&mut self.ui, &self.config);
            return;
        }
        if pressed && self.account_menu.is_open() && !self.account_menu.contains(&self.ui, p) {
            self.account_menu.close(&mut self.ui, &self.config);
            return;
        }
        if pressed && self.context_menus.is_open() && !self.context_menus.contains(&self.ui, p) {
            self.context_menus.close(&mut self.ui, &self.config);
            // A right click elsewhere opens that one's menu instead.
            if button != winit::event::MouseButton::Right {
                return;
            }
        }
        // Right and middle clicks on tabs and bookmarks (never a tab drag: that is the left
        // button's).
        if pressed && button != winit::event::MouseButton::Left && self.secondary_click(button, p) {
            return;
        }
        if pressed
            && self.downloads_popover.is_open()
            && !self.downloads_popover.contains(&self.ui, p)
            && !self.chrome.downloads_frame(&self.ui).contains(p)
        {
            self.downloads_popover.close(&mut self.ui, &self.config);
            return;
        }
        if pressed && self.permissions_press(p) {
            return;
        }
        if pressed && self.shields_popover.is_open() && !self.shields_popover.contains(&self.ui, p) {
            self.shields_popover.close(&mut self.ui, &self.config);
            return;
        }
        if pressed
            && self.playlist_ui.popover.is_open()
            && !self.playlist_ui.popover.contains(&self.ui, p)
            && !self.chrome.playlist_button_frame(&self.ui).contains(p)
        {
            self.playlist_ui.popover.close(&mut self.ui, &self.config);
            return;
        }
        if pressed && self.playlist_open() && self.playlist_ui.page.menu().is_open() {
            // A click outside a Playlist menu only dismisses it.
            if !self.playlist_ui.page.menu().contains(&self.ui, p) {
                self.playlist_ui.page.menu().close(&mut self.ui, &self.config);
                return;
            }
        }
        // A left button-up always reaches the UI: a slider/button held in the fullscreen player
        // is let go wherever the pointer is.
        let left_up = button == winit::event::MouseButton::Left && !pressed;
        if self.playlist_open() && !left_up && self.playlist_pointer(p, Some((button, pressed))) {
            return;
        }
        if button == winit::event::MouseButton::Left
            && pressed
            && self.overview.is_searching(&self.ui)
            && !self.overview.search_contains(&self.ui, p)
            && self.ui.hit_action(p).is_none()
        {
            // A click elsewhere in the overview puts the search away.
            self.overview_action(OverviewAction::ToggleSearch);
        }
        if button == winit::event::MouseButton::Left {
            if pressed {
                let (consumed, events) = self.ui.handle(UiInput::PointerDown);
                self.pointer_captured_by_ui = consumed;
                self.handle_ui_events(events);
                // A press on a tab may become a drag of it (not while editing the address).
                let item = self.ui.pressed_action().and_then(|n| self.chrome.tab_item(n));
                if let Some(item) = item
                    && self.ui.focused() != Some(self.chrome.address_field)
                    && !self.is_locked()
                {
                    self.tab_drag.press(item, p);
                }
                if consumed {
                    return;
                }
            } else if self.tab_drag.is_dragging() {
                self.pointer_captured_by_ui = false;
                let lens = self.tab_drag.lens_origin();
                let dropped = self.tab_drag.finish(&mut self.ui);
                // The UI's press was cancelled when the drag began: no click comes of this.
                let _ = self.ui.handle(UiInput::PointerUp);
                if let Some(dropped) = dropped {
                    self.tab_dropped(dropped.item, dropped.target, lens);
                }
                return;
            } else {
                self.tab_drag.release_press();
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
        if !self.page_rect().contains(p) && pressed || self.is_locked() {
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
        if pressed {
            self.tooltip.cancel(&mut self.ui);
            if input::key(event) == Some(Key::Named("Escape".into())) {
                // Esc ends a held drag (slider, button) before it does anything else.
                self.release_pointer_holds();
            }
        }
        if self.onboarding.is_open() {
            return self.onboarding_key(event);
        }
        if pressed && self.passwords.is_open() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.close_passwords();
            let _ = self.ui.blur();
            return false;
        }
        // Escape closes Find on Page wherever the focus is (the page after a click on it too).
        if pressed && self.find.is_open() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.close_find();
            return false;
        }
        if pressed
            && self.settings.is_open()
            && self.ui.focused().is_none()
            && input::key(event) == Some(Key::Named("Escape".into()))
        {
            self.close_settings();
            return false;
        }
        if pressed
            && self.playlist_open()
            && !self.settings.is_open()
            && self.ui.focused().is_none()
            && input::key(event) == Some(Key::Named("Escape".into()))
            && self.playlist_escape()
        {
            return false;
        }
        if pressed && self.tab_drag.is_dragging() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.tab_drag.cancel(&mut self.ui);
            self.tabs_changed = true;
            return false;
        }
        if pressed
            && self.lock_screen.is_alert_open()
            && input::key(event) == Some(Key::Named("Escape".into()))
        {
            self.lock_screen.close_alert(&mut self.ui);
            self.pending_move = None;
            return false;
        }
        if pressed && self.context_menus.is_open() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.context_menus.close(&mut self.ui, &self.config);
            return false;
        }
        if pressed && self.menu.is_open() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.menu.close(&mut self.ui, &self.config);
            return false;
        }
        if pressed && self.account_menu.is_open() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.account_menu.close(&mut self.ui, &self.config);
            return false;
        }
        if pressed
            && self.update_alert.is_dismissable()
            && input::key(event) == Some(Key::Named("Escape".into()))
        {
            self.update_alert_action(AlertAction::Backdrop);
            return false;
        }
        if pressed && self.add_account.is_open() && input::key(event) == Some(Key::Named("Escape".into())) {
            self.add_account.close(&mut self.ui);
            return false;
        }
        if pressed
            && self.downloads_popover.is_open()
            && input::key(event) == Some(Key::Named("Escape".into()))
        {
            self.downloads_popover.close(&mut self.ui, &self.config);
            return false;
        }
        if pressed && input::key(event) == Some(Key::Named("Escape".into())) && self.permissions_escape() {
            return false;
        }
        if pressed && self.shields_popover.is_open() && input::key(event) == Some(Key::Named("Escape".into()))
        {
            self.shields_popover.close(&mut self.ui, &self.config);
            return false;
        }
        if pressed
            && self.chrome.presentation()
            && !self.chrome.page_fullscreen()
            && self.ui.focused().is_none()
            && input::key(event) == Some(Key::Named("Escape".into()))
        {
            self.set_presentation(false);
            return false;
        }
        if pressed && self.playlist.fullscreen && input::key(event) == Some(Key::Named("Escape".into())) {
            self.playlist_set_fullscreen(false);
            return false;
        }
        if pressed
            && self.chrome.page_fullscreen()
            && self.player.mode() != PlayerMode::Fullscreen
            && input::key(event) == Some(Key::Named("Escape".into()))
            && let Some(view) = self.active_view()
        {
            // Off-screen pages can't leave fullscreen on Escape by themselves.
            self.engine.run_script(view, "document.fullscreenElement && document.exitFullscreen();", false);
            return false;
        }
        if pressed
            && let Some(chord) = input::chord(event, self.modifiers)
            && let Some(action) = self.keymap.action_for(&chord)
            // Media keys (Space, arrows…) are ours only while a video is fullscreen.
            && (!action.is_media()
                || (self.player.mode() == PlayerMode::Fullscreen || self.playlist_open())
                    && self.ui.focused().is_none())
            // Pasting into the page is ours only where its engine can't reach the clipboard;
            // fields of the chrome paste by themselves.
            && (!matches!(action, Action::Paste | Action::PastePlainText)
                || self.ui.focused().is_none() && !self.engine.has_system_clipboard())
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
            && self.ui.focused() == Some(self.find.field())
            && let Some(Key::Named(name)) = input::key(event)
            && (name == "Up" || name == "Down")
        {
            self.find.step(&mut self.engine, name == "Down");
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
                    if self.ui.focused() == Some(self.chrome.address_field) {
                        self.autocomplete_address();
                    }
                }
            }
            return false;
        }

        // A locked page gets no keys.
        if let Some(view) = self.active_view().filter(|_| !self.is_locked()) {
            for key in input::page_key_events(event, self.modifiers) {
                self.engine.send_input(view, &key);
            }
        }
        false
    }

    /// The tab whose row or segment is pressed (a drag candidate).
    fn tab_drag_tab(&self) -> Option<ViewId> {
        self.ui.pressed_action().and_then(|n| self.chrome.tab_item(n)).map(|i| i.view)
    }

    // ---- drawing ----

    fn redraw(&mut self) -> Result<()> {
        let started = Instant::now();
        let (presented, skipped) = (self.frames_presented, self.frames_skipped);
        let result = self.redraw_frame();
        if let Some(log) = &mut self.perf_log {
            let mut times = self.frame_times;
            times.total = ms_since(started);
            let outcome = if self.frames_presented != presented {
                FrameOutcome::Presented
            } else if self.frames_skipped != skipped {
                FrameOutcome::Skipped
            } else {
                FrameOutcome::NoImage
            };
            log.frame(times, outcome);
        }
        let serial = self.active_view().and_then(|v| self.engine.frame(v)).map_or(0, |f| f.serial);
        self.frame_stats.record(started.elapsed(), serial);
        // Resize events are applied inside the frame (`apply_window_size`).
        self.frame_times.total = ms_since(started);
        result?;
        self.bench_frame()
    }

    /// The window's newest size from its `Resized` events since the last frame.
    fn apply_window_size(&mut self) {
        let Some(size) = self.pending_window_size.take() else { return };
        let started = Instant::now();
        if (size.width, size.height) != (self.surface_config.width, self.surface_config.height) {
            self.resize_counters.configures += 1;
            self.surface_config.width = size.width;
            self.surface_config.height = size.height;
            self.surface.configure(&self.device, &self.surface_config);
        }
        self.frame_times.configure += ms_since(started);
        let laid = Instant::now();
        self.update_layout();
        self.frame_times.layout += ms_since(laid);
        self.sync_frame_rate();
        self.frame_times.resize_events += ms_since(started);
    }

    /// The shown page gets the window's sizes one at a time (see `window_resize`), the
    /// background tabs once the window has been still for `layout.background_resize_delay`.
    fn pace_page_resizes(&mut self, now: Instant) {
        let wait = Duration::from_secs_f32(self.config.layout.window_resize_wait.max(0.0));
        if let Some(view) = self.paced_view {
            if let Some(frame) = self.engine.frame(view) {
                self.page_pacer.frame(frame.texture.width(), frame.texture.height());
            }
            if !self.chrome.is_sliding()
                && Some(view) == self.display_view()
                && let Some(size) = self.page_pacer.poll(now, wait)
            {
                self.engine.resize(view, size);
            }
        }
        if self.background_resize.take_due(now) {
            let size = self.view_size();
            let sliding = self.slide_view_size.map(|(view, _)| view);
            let views: Vec<ViewId> = self.tabs.all().iter().map(|t| t.view).collect();
            for view in views {
                if Some(view) != sliding && Some(view) != self.paced_view {
                    self.engine.resize(view, size);
                }
            }
        }
    }

    /// Gives the shown view `size`, paced (see `pace_page_resizes`).
    fn resize_shown_view(&mut self, view: ViewId, size: ViewSize) {
        if self.paced_view != Some(view) {
            self.paced_view = Some(view);
            self.page_pacer.reset();
        }
        let wait = Duration::from_secs_f32(self.config.layout.window_resize_wait.max(0.0));
        if let Some(size) = self.page_pacer.request(size, Instant::now(), wait) {
            self.engine.resize(view, size);
        }
    }

    fn redraw_frame(&mut self) -> Result<()> {
        self.apply_window_size();
        if self.logical_size() != self.layout_size {
            let _op = crate::stall::op("frame: layout");
            let started = Instant::now();
            self.update_layout();
            self.frame_times.layout += ms_since(started);
        }
        let tabs_changed = self.tabs_changed;
        if std::mem::take(&mut self.tabs_changed) {
            self.update_now_playing();
            self.session_dirty = true;
            self.session_changed = Instant::now();
            let normal_count = if self.private_mode { self.parked.len() } else { self.tabs.len() };
            self.chrome.set_normal_count(normal_count);
            self.chrome.sync_tabs(&mut self.ui, &self.tabs, &self.parked, &self.favicons);
            let audio = self.tabs.active().map_or((false, false), |t| (t.playing_media, t.muted));
            self.chrome.set_audio(&mut self.ui, audio.0, audio.1);
            let capture = self.tabs.active().map(|t| t.capture).unwrap_or_default();
            self.chrome.set_capture(&mut self.ui, capture);
            let level = self.tabs.active().map(|t| self.shields.level(&t.url));
            self.chrome.set_shield_active(&mut self.ui, level.is_some_and(|l| l != Level::Off));
            // A locked private tab's title stays out of the window title too.
            let title =
                self.tabs.active().filter(|_| !self.is_locked()).map(|t| t.label()).unwrap_or_default();
            self.window.set_title(
                if title.is_empty() { "Axion".to_owned() } else { format!("Axion - {title}") }.as_str(),
            );
        }
        self.sync_bookmarks_bar(tabs_changed);
        let now = Instant::now();
        if self.overview_at.is_some_and(|at| now >= at) {
            self.overview_at = None;
            self.open_overview();
        }
        if let Some((_, list)) = self.open_at.take_if(|(at, _)| now >= *at) {
            // Several steps ("a,b") run one after another, a moment apart.
            let (what, rest) = list.split_once(',').unwrap_or((list.as_str(), ""));
            if !rest.is_empty() {
                self.open_at = Some((now + OPEN_STEP_INTERVAL, rest.to_owned()));
            }
            match what {
                "menu" => self.open_page_menu(),
                // Capture aids: the developer tools (again: back out of them), on the Console,
                // the page source, and closing the shown tab.
                "devtools" => self.run(Action::DevTools),
                "devtools-console" => self.run(Action::DevToolsConsole),
                "view-source" => self.run(Action::ViewSource),
                "close-tab" => self.run(Action::CloseTab),
                // Capture aids: the page menu with its zoom submenu open, a zoom step in it,
                // and closing every popup at once (mid-animation with --lead / frame sequences).
                "zoom-menu" => self.menu.open_first_submenu(&mut self.ui, &self.config),
                "zoom-in" => self.menu_action(MenuAction::ZoomIn),
                "close-popups" => self.close_popups(),
                "shields" => self.toggle_shields_popover(),
                // Test aids: answer the permission prompt, open the recording indicator's
                // popover and stop everything the tab records.
                answer if answer.starts_with("permission-") => self.answer_permission_for_test(answer),
                "capture-popover" => self.toggle_capture_popover(),
                "capture-stop" => self.capture_action(crate::permission_prompt::CaptureAction::StopAll),
                // Test aid: load "navigate:URL" in the shown tab (after "private": a private one).
                url if url.starts_with("navigate:") => self.navigate_to(url["navigate:".len()..].to_owned()),
                "history" => self.show_panel(Panel::History),
                "downloads" => self.show_panel(Panel::Downloads),
                "downloads-popover" => self.toggle_downloads_popover(),
                // Test aids: the Share button's toast, and the sidebar list's search text
                // ("panel-search:TEXT", after "history" / "bookmarks").
                "share" => self.copy_link(),
                query if query.starts_with("panel-search:") => {
                    let field = self.chrome.panel_search_field();
                    self.ui.set_field_text(field, &query["panel-search:".len()..]);
                }
                // Capture aid: "Clear Browsing Data" with the configured choices.
                "clear-data" => self.settings_outcome(Outcome::ClearBrowsingData),
                "data-settings" => {
                    self.settings.set_category(Category::Data);
                    self.run(Action::OpenSettings);
                }
                // Capture aid: toggle the sidebar.
                "sidebar" => self.run(Action::ToggleSidebar),
                // Capture aid: type "type:TEXT" into the address bar like the keyboard does.
                typed if typed.starts_with("type:") => {
                    self.edit_address();
                    let (_, events) = self.ui.handle(UiInput::Text(typed["type:".len()..].to_owned()));
                    self.handle_ui_events(events);
                    self.autocomplete_address();
                }
                "bookmarks" => self.show_panel(Panel::Bookmarks),
                "settings" => self.run(Action::OpenSettings),
                // Capture aids for the updater: Settings › About (and checking), the install
                // asking for confirmation, and answering it.
                "about" => self.open_about(false),
                "about-check" => self.open_about(true),
                "update-install" => self.request_update_install(),
                "update-confirm" => self.update_alert_action(AlertAction::Primary),
                "update-cancel" => self.update_alert_action(AlertAction::Secondary),
                // Capture aid: a real pointer click on the toast's Update button.
                "toast-click" => {
                    let at = self.toast.button_center(&self.ui);
                    let _ = self.ui.handle(UiInput::PointerMove(at));
                    let (_, events) = self.ui.handle(UiInput::PointerDown);
                    self.handle_ui_events(events);
                    let (_, events) = self.ui.handle(UiInput::PointerUp);
                    self.handle_ui_events(events);
                }
                "about-notes-scroll" => {
                    let point = self.settings_notes_point();
                    let scrolled = self.settings.scroll_notes(&mut self.ui, -60.0, point);
                    tracing::info!(scrolled, "release notes scrolled");
                }
                // Capture aids for the Playlist: its page, the Add popover, adding the page's
                // video, playing row N, a search, a row's menu, edit mode, the player's menus.
                "playlist" => self.open_playlist(),
                "playlist-popover" => self.toggle_playlist_popover(),
                "playlist-add" => self.run(Action::PlaylistAdd),
                "playlist-edit" => self.playlist_action(crate::playlist_page::PageAction::Edit),
                "playlist-speed" => self.playlist_action(crate::playlist_page::PageAction::Speed),
                "playlist-sort-menu" => self.playlist_action(crate::playlist_page::PageAction::Sort),
                sort if sort.starts_with("playlist-sort:") => {
                    let sort = match &sort["playlist-sort:".len()..] {
                        "oldest" => axion_config::PlaylistSort::Oldest,
                        "manual" => axion_config::PlaylistSort::Manual,
                        _ => axion_config::PlaylistSort::Newest,
                    };
                    self.playlist_command(crate::playlist::Command::Sort(sort));
                }
                "playlist-sleep" => self.playlist_action(crate::playlist_page::PageAction::Sleep),
                "playlist-popout" => self.playlist_action(crate::playlist_page::PageAction::Fullscreen),
                "playlist-pip" => self.playlist_action(crate::playlist_page::PageAction::PictureInPicture),
                "playlist-open-page" => self.playlist_action(crate::playlist_page::PageAction::OpenPage),
                // Items saved before sources were captured: forget every captured source.
                "playlist-forget-source" => {
                    let ids: Vec<i64> = self.playlist.items.iter().map(|i| i.id).collect();
                    if let Some(store) = &self.store {
                        for id in ids {
                            let _ = store.set_playlist_source(id, "");
                        }
                    }
                    self.playlist.reload(self.store.as_ref(), &self.config);
                }
                play if play.starts_with("playlist-play:") => {
                    if let Some(id) = play["playlist-play:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| self.playlist_ui.shown.get(i).copied())
                    {
                        self.playlist_play(id);
                    }
                }
                menu if menu.starts_with("playlist-menu:") => {
                    if let Ok(i) = menu["playlist-menu:".len()..].parse::<usize>() {
                        self.playlist_action(crate::playlist_page::PageAction::RowMenu(i));
                    }
                }
                query if query.starts_with("playlist-search:") => {
                    let events = self.playlist_ui.page.set_search(&mut self.ui, true);
                    self.handle_ui_events(events);
                    let field = self.playlist_ui.page.search_field();
                    self.ui.set_field_text(field, &query["playlist-search:".len()..]);
                }
                "playlist-download" => {
                    let ids: Vec<i64> = self.playlist.items.iter().map(|i| i.id).collect();
                    for id in ids {
                        self.playlist_command(crate::playlist::Command::Download(id));
                    }
                }
                // Capture aid: what the start page's path field sends ("home-background:PATH"),
                // and picking a background in its gear panel ("home-select:N", 0: none).
                path if path.starts_with("home-background:") => {
                    let message = serde_json::json!({ "type": "backgroundPath", "path": &path["home-background:".len()..] });
                    if let Some(view) = self.active_view() {
                        self.internal_message(view, "home", &message.to_string());
                    }
                }
                pick if pick.starts_with("home-select:") => {
                    let index = pick["home-select:".len()..].parse::<usize>().unwrap_or(0);
                    let name = index
                        .checked_sub(1)
                        .and_then(|i| self.home.background_cards().get(i).map(|c| c.name.clone()));
                    let message =
                        serde_json::json!({ "type": "selectBackground", "name": name.unwrap_or_default() });
                    if let Some(view) = self.active_view() {
                        self.internal_message(view, "home", &message.to_string());
                    }
                }
                // Capture aids: switch the theme while running.
                "theme-dark" | "theme-light" => {
                    self.config.appearance.theme =
                        if what == "theme-dark" { ThemeMode::Dark } else { ThemeMode::Light };
                    self.apply_theme();
                }
                "next-tab" => self.run(Action::NextTab),
                // Debug aid: the main player logs its loading state from its frame (PageMessage log).
                "probe-media" => {
                    let target = self
                        .active_view()
                        .zip(self.active_view().and_then(|v| self.primary_player(v)).map(|p| p.id.clone()));
                    if let Some((view, id)) = target {
                        self.media_command(view, &axion_media::Command::new(&id, axion_media::Action::Probe));
                    }
                }
                // Capture aid: press at window position X1:Y1 and drag to X2:Y2 without releasing
                // ("drag:X1:Y1:X2:Y2", logical px) — shows held controls.
                drag if drag.starts_with("drag:") => {
                    let n: Vec<f32> = drag.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    if let [x1, y1, x2, y2] = n[..] {
                        let (_, _) = self.ui.handle(UiInput::PointerMove(Vec2::new(x1, y1)));
                        let (_, events) = self.ui.handle(UiInput::PointerDown);
                        self.handle_ui_events(events);
                        let (_, events) = self.ui.handle(UiInput::PointerMove(Vec2::new(x2, y2)));
                        self.handle_ui_events(events);
                    }
                }
                // Capture aid: a real click at page position "click:X:Y" (logical px).
                click if click.starts_with("click:") => {
                    let mut parts = click.split(':').skip(1).filter_map(|n| n.parse::<f32>().ok());
                    if let (Some(x), Some(y), Some(view)) = (parts.next(), parts.next(), self.active_view()) {
                        self.click_page(view, Vec2::new(x, y));
                    }
                }
                // Capture aid: scroll the sidebar by "scroll-sidebar:DY" (negative: down).
                scroll if scroll.starts_with("scroll-sidebar:") => {
                    if let Ok(dy) = scroll["scroll-sidebar:".len()..].parse::<f32>() {
                        self.chrome.scroll_sections(&mut self.ui, dy);
                    }
                }
                // Capture aid: search the open settings for "settings-search:TEXT".
                query if query.starts_with("settings-search:") => {
                    let field = self.settings.search_field();
                    self.ui.set_field_text(field, &query["settings-search:".len()..]);
                }
                // Capture aid: pick tab N of the other group in the sidebar.
                other if other.starts_with("other-tab:") => {
                    if let Some(i) = other.split(':').nth(1).and_then(|n| n.parse::<usize>().ok()) {
                        self.handle_ui_events(vec![UiEvent::Clicked {
                            action: ChromeAction::SelectOtherTab(i).id(),
                            node: self.ui.root(),
                        }]);
                    }
                }
                // Capture aids: a real mouse press at window position "press:X:Y" (logical px),
                // and letting go ("release") — with "sweep:" in between they drag (tabs).
                press if press.starts_with("press:") => {
                    let n: Vec<f32> = press.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    if let [x, y] = n[..] {
                        self.pointer_moved(Vec2::new(x, y));
                        self.mouse_button(ElementState::Pressed, winit::event::MouseButton::Left);
                    }
                }
                "release" => self.mouse_button(ElementState::Released, winit::event::MouseButton::Left),
                // Capture aid: a quick click at "tap:X:Y" (press and release in one step).
                tap if tap.starts_with("tap:") => {
                    let n: Vec<f32> = tap.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    if let [x, y] = n[..] {
                        self.pointer_moved(Vec2::new(x, y));
                        self.mouse_button(ElementState::Pressed, winit::event::MouseButton::Left);
                        self.mouse_button(ElementState::Released, winit::event::MouseButton::Left);
                    }
                }
                // Capture aids: a right ("rtap:X:Y") or middle ("mtap:X:Y") click; pinning tab N
                // ("pin-tab:N") and bookmarking all tabs, as their context menu rows do.
                tap if tap.starts_with("rtap:") || tap.starts_with("mtap:") => {
                    let n: Vec<f32> = tap.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    let button = if tap.starts_with('r') {
                        winit::event::MouseButton::Right
                    } else {
                        winit::event::MouseButton::Middle
                    };
                    if let [x, y] = n[..] {
                        self.pointer_moved(Vec2::new(x, y));
                        self.mouse_button(ElementState::Pressed, button);
                        self.mouse_button(ElementState::Released, button);
                    }
                }
                pin if pin.starts_with("pin-tab:") => {
                    if let Some(view) = pin["pin-tab:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| self.tabs.all().get(i))
                        .map(|t| t.view)
                    {
                        self.context_tab = Some((view, self.private_mode));
                        self.tab_command(crate::context_menu::TabCommand::Pin);
                    }
                }
                "bookmark-all" => {
                    self.context_tab = self.active_view().map(|v| (v, self.private_mode));
                    self.tab_command(crate::context_menu::TabCommand::BookmarkAll);
                }
                // Capture aid: click a sidebar or toolbar control by its action ("chrome:N").
                chrome if chrome.starts_with("chrome:") => {
                    if let Ok(id) = chrome["chrome:".len()..].parse::<u32>() {
                        let node = self.ui.root();
                        self.handle_ui_events(vec![UiEvent::Clicked {
                            action: axion_ui::ActionId(id),
                            node,
                        }]);
                    }
                }
                // Capture aid: the window manager closes the PiP window (Alt+F4, its close button).
                "pip-close" => self.pip_window_event(WindowEvent::CloseRequested),
                // Capture aids: the PiP window's × and "back to the tab" buttons.
                "pip-x" | "pip-back" => {
                    let control = if what == "pip-x" { PipControl::Close } else { PipControl::BackToTab };
                    let outcome = self.pip.as_mut().and_then(|p| p.click(control, &self.config));
                    self.pip_outcome(outcome);
                }
                // Capture aid: resize the PiP window to "pip-size:W:H" (logical px), like the
                // user dragging its edge.
                size if size.starts_with("pip-size:") => {
                    let n: Vec<f64> = size.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    if let (&[w, h], Some(pip)) = (&n[..], &mut self.pip) {
                        pip.request_size(winit::dpi::LogicalSize::new(w, h), &self.config);
                    }
                }
                // Capture aids: the pointer in the PiP window (logical px) as a user drags: move
                // to "pip-move:X:Y", press "pip-press" there, let go with "pip-release". No
                // window-manager grab is started.
                pointer if pointer.starts_with("pip-move:") => {
                    let n: Vec<f32> = pointer.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    if let ([x, y], Some(pip)) = (&n[..], &mut self.pip) {
                        pip.synthetic = true;
                        let outcome = pip.pointer_moved(Vec2::new(*x, *y), &self.config);
                        self.pip_outcome(outcome);
                    }
                }
                "pip-press" | "pip-release" => {
                    if let Some(pip) = &mut self.pip {
                        pip.synthetic = true;
                        let outcome = pip.button(what == "pip-press", &self.config);
                        self.pip_outcome(outcome);
                    }
                }
                // Capture aid: start Picture in Picture for the page's main video.
                "player-pip" => {
                    let target = self
                        .active_view()
                        .zip(self.active_view().and_then(|v| self.primary_player(v)).cloned());
                    if let Some((view, player)) = target {
                        self.start_pip(view, player);
                    }
                }
                // Capture aid: the page's main video goes fullscreen (as its own fullscreen
                // button would; Axion's player takes over), again to leave it.
                "video-fullscreen" => {
                    let target = self
                        .active_view()
                        .zip(self.active_view().and_then(|v| self.primary_player(v)).cloned());
                    if let Some((view, player)) = target {
                        let on = !self.chrome.page_fullscreen();
                        self.media_command(
                            view,
                            &axion_media::Command::new(&player.id, axion_media::Action::Fullscreen(on)),
                        );
                    }
                }
                // Capture aid: open the player's speed menu.
                // Capture aid: scroll the page down by 600 px.
                "toggle-sidebar" => self.run(Action::ToggleSidebar),
                // Capture aid: F11, whole-page fullscreen (again to leave it).
                "presentation" => self.run(Action::ToggleFullscreen),
                // Capture aid: move the pointer to window position "hover:X:Y" (logical px).
                hover if hover.starts_with("hover:") => {
                    let mut parts = hover.split(':').skip(1).filter_map(|n| n.parse::<f32>().ok());
                    if let (Some(x), Some(y)) = (parts.next(), parts.next()) {
                        self.pointer_moved(Vec2::new(x, y));
                    }
                }
                sweep if sweep.starts_with("sweep:") => {
                    let n: Vec<f32> = sweep.split(':').skip(1).filter_map(|v| v.parse().ok()).collect();
                    if let [x1, y1, x2, y2, steps] = n[..] {
                        self.sweep = Some((Vec2::new(x1, y1), Vec2::new(x2, y2), steps.max(1.0) as u32, 0));
                    }
                }
                // Test aid: "keys:TEXT" types TEXT into the page as the keyboard does ("\\b" is
                // Backspace).
                keys if keys.starts_with("keys:") => {
                    if let Some(view) = self.active_view() {
                        for key in input::typed_keys(&keys["keys:".len()..].replace("\\b", "\x08")) {
                            self.engine.send_input(view, &key);
                        }
                    }
                }
                wheel if wheel.starts_with("wheel:") => {
                    self.wheel_frames = wheel["wheel:".len()..].parse().unwrap_or(0);
                }
                // Test aids: Find on Page for "find:TEXT" (typed like the keyboard does), its
                // next / previous match, the case toggle and closing it.
                find if find.starts_with("find:") => {
                    self.open_find();
                    let (_, events) = self.ui.handle(UiInput::Text(find["find:".len()..].to_owned()));
                    self.handle_ui_events(events);
                }
                "find-next" => self.find_action(FindAction::Next),
                "find-previous" => self.find_action(FindAction::Previous),
                "find-case" => self.find_action(FindAction::ToggleMatchCase),
                "find-close" => self.find_action(FindAction::Close),
                // Test aids: a file from the desktop dragged to window position
                // "drag-file:PATH:X:Y" (logical px; it stays there until the next step), then
                // dropped ("drop-file") or taken away ("drag-cancel").
                drag if drag.starts_with("drag-file:") => {
                    let mut parts = drag["drag-file:".len()..].rsplitn(3, ':');
                    let (y, x, path) = (parts.next(), parts.next(), parts.next());
                    if let (Some(Ok(y)), Some(Ok(x)), Some(path)) =
                        (y.map(str::parse::<f32>), x.map(str::parse::<f32>), path)
                    {
                        self.file_drag_at = Some(Vec2::new(x, y));
                        self.file_drop.hovered(PathBuf::from(path));
                    }
                }
                "drop-file" => {
                    if let Some(path) = self.file_drop.files().first().cloned() {
                        self.file_drop.dropped(path);
                    }
                }
                "drag-cancel" => self.file_drop.cancelled(),
                // Test aid: a file dragged in and dropped at once at "drag-drop-file:PATH:X:Y".
                drag if drag.starts_with("drag-drop-file:") => {
                    let mut parts = drag["drag-drop-file:".len()..].rsplitn(3, ':');
                    let (y, x, path) = (parts.next(), parts.next(), parts.next());
                    if let (Some(Ok(y)), Some(Ok(x)), Some(path)) =
                        (y.map(str::parse::<f32>), x.map(str::parse::<f32>), path)
                    {
                        self.file_drag_at = Some(Vec2::new(x, y));
                        self.file_drop.hovered(PathBuf::from(path));
                        self.file_drop.dropped(PathBuf::from(path));
                    }
                }
                // Test aids: Ctrl+<letter> on the page as the keyboard sends it ("ctrl:v"), and
                // the paste Axion makes itself where the engine has no clipboard ("paste").
                ctrl if ctrl.starts_with("ctrl:") => {
                    if let (Some(view), Some(letter)) =
                        (self.active_view(), ctrl["ctrl:".len()..].chars().next())
                    {
                        for key in input::ctrl_keys(letter) {
                            self.engine.send_input(view, &key);
                        }
                    }
                }
                // Capture aid: type "ui-text:TEXT" into whatever chrome field has the focus.
                typed if typed.starts_with("ui-text:") => {
                    let (_, events) = self.ui.handle(UiInput::Text(typed["ui-text:".len()..].to_owned()));
                    self.handle_ui_events(events);
                }
                // Capture aid: a keyboard shortcut through the keymap, as pressed ("chord:Alt+T").
                chord if chord.starts_with("chord:") => {
                    match chord["chord:".len()..].parse::<axion_config::Chord>() {
                        Ok(c) => match self.keymap.action_for(&c) {
                            Some(action) => {
                                tracing::info!("chord {c:?} runs {action}");
                                self.run(action);
                            }
                            None => tracing::warn!("chord {c:?} is bound to nothing"),
                        },
                        Err(e) => tracing::warn!("bad chord: {e}"),
                    }
                }
                "paste" => self.run(Action::Paste),
                // Capture aid: Ctrl+V in the focused chrome field.
                "ui-paste" => {
                    if let Some(node) = self.ui.focused() {
                        self.handle_ui_events(vec![UiEvent::PasteRequested { node }]);
                    }
                }
                // Capture aid: audits the next frame of a flat-mode window for glass or anything
                // that imitates it (a glass pass, a rim, a knocked-out or heavy shadow) and logs
                // every finding ("flat-audit: ...").
                "flat-audit" => self.flat_audit = true,
                // Capture aid: blocks the main thread for MS milliseconds ("stall:MS"), to check
                // the stall watchdog (`AXION_STALL_LOG`).
                ms if ms.starts_with("stall:") => {
                    if let Ok(ms) = ms["stall:".len()..].parse::<u64>() {
                        let _op = crate::stall::op("capture aid: stall");
                        std::thread::sleep(Duration::from_millis(ms));
                    }
                }
                // Capture aid: switches the tab bar live, the way Settings > Appearance does
                // ("tabbar:separate" / "tabbar:compact").
                style if style.starts_with("tabbar:") => {
                    let style = match &style["tabbar:".len()..] {
                        "separate" => Some(TabBarStyle::Separate),
                        "compact" => Some(TabBarStyle::Compact),
                        _ => None,
                    };
                    if let Some(style) = style {
                        self.config.layout.tab_bar = style;
                        self.settings_outcome(Outcome::Changed);
                    }
                }
                // Capture aid: a click into the address field, then "TEXT" typed into it, as
                // keystrokes ("edit-address:TEXT"; unlike `--address`, which sets the text).
                typed if typed.starts_with("edit-address:") => {
                    self.edit_address();
                    let text = typed["edit-address:".len()..].to_owned();
                    let (_, events) = self.ui.handle(UiInput::Text(text));
                    self.handle_ui_events(events);
                }
                // Capture aid: logs every text of the visible chrome that does not fit its box
                // (a translation too long for its layout).
                "i18n-overflow" => {
                    let clipped = self.ui.clipped_texts();
                    tracing::info!("i18n-overflow: {} clipped texts", clipped.len());
                    for (text, width, wanted) in clipped {
                        tracing::warn!("i18n-overflow: {text:?} needs {wanted:.0} px, box is {width:.0} px");
                    }
                }
                // Capture aid: open Settings on page N (0 = Account … 10 = Passwords).
                page if page.starts_with("settings-page:") => {
                    if let Some(category) = page["settings-page:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| Category::ALL.get(i).copied())
                    {
                        self.settings.set_category(category);
                        self.run(Action::OpenSettings);
                    }
                }
                "settings-scroll" => self.settings.scroll_by(&mut self.ui, -400.0),
                // Capture aids for the import group: "import:source:N" (toggle a browser),
                // "import:bookmarks" / "import:history" (toggle), "import:run", "import:file:PATH".
                import if import.starts_with("import:") => self.import_capture(&import["import:".len()..]),
                // Capture aids for Settings › Hardware: a change that needs a restart, and the
                // AMD confirmation.
                "hardware-change" => {
                    self.config.hardware.gpu_rasterization = false;
                    self.settings_outcome(Outcome::Changed);
                    self.refresh_settings();
                }
                "hardware-confirm" => self.confirm_gpu_frames(),
                "player-speed" => {
                    let _ = self.player.click(&mut self.ui, PlayerControl::Speed, &self.config);
                }
                // Capture aid: scroll the page's main player into view.
                "scroll-player" => {
                    let y = self.tabs.active().and_then(|t| t.media.as_ref()?.primary().map(|p| p.y));
                    if let (Some(view), Some(y)) = (self.active_view(), y) {
                        self.engine.run_script(view, &format!("window.scrollBy(0, {} - 90);", y), false);
                    }
                }
                // Capture aid: what a site's own fullscreen button does (its player's frame).
                "page-fullscreen" => {
                    if let Some(view) = self.active_view() {
                        let script = "(document.querySelector('iframe') || document.querySelector('video')).requestFullscreen();";
                        self.engine.run_script(view, script, true);
                    }
                }
                // Capture aids for the media controls: play the page's video (and fullscreen it).
                "media-play" | "media-fullscreen" => {
                    let target = self
                        .active_view()
                        .zip(self.active_view().and_then(|v| self.primary_player(v)).cloned());
                    if let Some((view, player)) = target {
                        let mut actions = vec![axion_media::Action::Play];
                        if what == "media-fullscreen" {
                            actions.push(axion_media::Action::Fullscreen(true));
                        }
                        for action in actions {
                            self.media_command(view, &axion_media::Command::new(&player.id, action));
                        }
                    }
                }
                // Capture aid: logs the media state of the active tab, the Playlist playback and
                // the PiP video ("media-log"), to check what seeks and volume changes did.
                "media-log" => self.log_media_state(),
                "shields-settings" => {
                    self.settings.set_category(Category::Shields);
                    self.run(Action::OpenSettings);
                }
                "appearance-settings" => {
                    self.settings.set_category(Category::Appearance);
                    self.run(Action::OpenSettings);
                }
                "keyboard-settings" => {
                    self.settings.set_category(Category::Keyboard);
                    self.run(Action::OpenSettings);
                }
                // Capture aid: filter the Keyboard page's list by "keys-search:TEXT".
                query if query.starts_with("keys-search:") => {
                    if let Some(field) = self.settings.key_search_field() {
                        self.ui.set_field_text(field, &query["keys-search:".len()..]);
                    }
                }
                "tabs-settings" => {
                    self.settings.set_category(Category::Tabs);
                    self.run(Action::OpenSettings);
                }
                "media-settings" => {
                    self.settings.set_category(Category::Media);
                    self.run(Action::OpenSettings);
                }
                "private" => self.toggle_private_mode(),
                // Capture aid: switch the open overview to the private or normal tabs.
                "overview-private" => self.overview_action(OverviewAction::ShowPrivate),
                "overview-normal" => self.overview_action(OverviewAction::ShowNormal),
                "passwords" => self.open_passwords(),
                "passwords-settings" => {
                    self.settings.set_category(Category::Passwords);
                    self.run(Action::OpenSettings);
                }
                // Capture aids: fake entries in a throwaway profile's vault, then views of them.
                "passwords-test" if std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                    self.passwords.seed_test_data();
                    self.open_passwords();
                }
                step if step.starts_with("passwords-") && self.passwords.is_open() => {
                    let step = &step["passwords-".len()..];
                    let outcome = self.passwords.capture_step(&mut self.ui, &mut self.config, step);
                    self.passwords_outcome(outcome);
                }
                "prompt-unlock" => {
                    let outcome = self.passwords.capture_step(&mut self.ui, &mut self.config, "unlock");
                    self.passwords_outcome(outcome);
                }
                "prompt-fill" => {
                    let outcome = self.passwords.capture_step(&mut self.ui, &mut self.config, "fill");
                    self.passwords_outcome(outcome);
                }
                "save-prompt" | "fill-prompt" => {
                    self.close_passwords();
                    let target = self.tabs.active().map(|t| (t.view, t.url.clone()));
                    if let Some((view, url)) = target {
                        if what == "save-prompt" {
                            self.passwords.show_save_prompt_for_test(&mut self.ui, &self.config, view, &url);
                        } else {
                            let event = axion_vault::autofill::PageEvent::Form;
                            self.passwords.page_event(&mut self.ui, &self.config, view, &url, event);
                        }
                    }
                }
                // Capture aid (throwaway profiles only): give the account a test password, then
                // show the locked private tabs.
                "private-locked" if std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                    self.set_test_password();
                    self.toggle_private_mode();
                    self.lock_account();
                }
                "account-password" if std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                    self.set_test_password()
                }
                // Capture aids: the Now Playing card's buttons, and "now-seek:FRACTION" (its slider).
                "now-prev" => self.now_playing_previous(),
                "now-next" => self.now_playing_command(axion_media::Action::NextTrack),
                seek if seek.starts_with("now-seek:") => {
                    if let Ok(value) = seek["now-seek:".len()..].parse::<f32>()
                        && let Some(to) = self.chrome.now_playing_seek(value)
                    {
                        self.now_playing_command(axion_media::Action::Seek(to));
                    }
                }
                "account-menu" => self.toggle_account_menu(),
                "add-account" => self.open_add_account(),
                // Capture aid: type "add-account-name:NAME" into the open Add Account sheet.
                name if name.starts_with("add-account-name:") => {
                    self.add_account.set_name(&mut self.ui, &name["add-account-name:".len()..]);
                }
                "add-account-create" => self.add_account_action(SheetAction::Create),
                // Capture aids (throwaway profiles only): "account-avatar:PATH" sets the current
                // account's profile picture, "add-account-avatar:PATH" the Add Account sheet's,
                // "account-avatar-remove" removes it.
                photo
                    if photo.starts_with("account-avatar:")
                        && std::env::var_os("AXION_PROFILE_DIR").is_some() =>
                {
                    self.apply_photo(
                        PhotoFor::Current,
                        std::path::Path::new(&photo["account-avatar:".len()..]),
                    );
                }
                photo
                    if photo.starts_with("add-account-avatar:")
                        && std::env::var_os("AXION_PROFILE_DIR").is_some() =>
                {
                    self.apply_photo(
                        PhotoFor::NewAccount,
                        std::path::Path::new(&photo["add-account-avatar:".len()..]),
                    );
                }
                "account-avatar-remove" if std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                    self.settings_outcome(Outcome::RemoveAccountPhoto);
                }
                "account-settings" => self.open_account_settings(),
                "privacy-settings" => {
                    self.settings.set_category(Category::Privacy);
                    self.run(Action::OpenSettings);
                }
                "lock-account" => self.lock_account(),
                // Capture aid (throwaway profiles only): delete this account (relaunches).
                "delete-account" if std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                    self.settings_outcome(Outcome::DeleteAccount);
                }
                // Capture aid (throwaway profiles only): unlock the lock screen with the test
                // password, as if typed.
                "unlock-private-test" if std::env::var_os("AXION_PROFILE_DIR").is_some() => {
                    self.ui.set_field_text(self.lock_screen.field(), crate::passwords::TEST_PASSWORD);
                    self.try_unlock();
                }
                // Capture aid: switch to the account at index N of the list (relaunches).
                switch if switch.starts_with("switch-account:") => {
                    let index = switch["switch-account:".len()..].parse::<usize>().ok();
                    if let Some(id) = index.and_then(|i| self.accounts.all().get(i)).map(|a| a.id.clone()) {
                        self.switch_account(&id);
                    }
                }
                "unlock-alert" => {
                    let events = self.lock_screen.action(&mut self.ui, &self.config, LockScreen::SHOW_ALERT);
                    self.handle_ui_events(events);
                }
                onboarding if onboarding.starts_with("onb") && self.onboarding_capture_step(onboarding) => {}
                other => tracing::warn!("--open={other} is not a known view"),
            }
        }
        if let Some((_, text)) = self.address_at.take_if(|(at, _)| now >= *at) {
            self.edit_address();
            self.ui.set_field_text(self.chrome.address_field, &text);
        }
        if self.wheel_frames > 0
            && let Some(view) = self.active_view()
        {
            self.wheel_frames -= 1;
            // Every other second the other way, so long runs don't stop at the page's end.
            let down = (self.wheel_frames / 120).is_multiple_of(2);
            let delta = Vec2::new(0.0, if down { -LINE_SCROLL } else { LINE_SCROLL });
            let pos = self.page_rect().size / 2.0;
            let modifiers = input::engine_modifiers(self.modifiers);
            self.engine.send_input(view, &InputEvent::Wheel { pos, delta, modifiers });
        }
        if let Some((from, to, steps, done)) = self.sweep.take() {
            let t = done as f32 / steps as f32;
            self.pointer_moved(from + (to - from) * t);
            if done < steps {
                self.sweep = Some((from, to, steps, done + 1));
            }
        }
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.ui.animate(dt);
        let was_sliding = self.chrome.is_sliding();
        self.chrome.animate_all(&mut self.ui, dt);
        if was_sliding {
            // The page's rectangle and the toolbar follow the sidebar frame by frame, but the
            // page itself is resized once, as the slide starts, to its size once it settles:
            // resizing Chromium every frame made it re-render at each size, every new size
            // starting with an empty frame. While the sidebar moves, the page is drawn from the
            // rectangle's right edge, which stays put, so its content stands still and the
            // sidebar slides over it or away from it (see `slide_page_frame`). The other tabs
            // follow once the slide has settled.
            let size = self.logical_size();
            self.chrome.apply_layout(&mut self.ui, &self.config, size);
            if self.chrome.is_sliding() && self.config.layout.sidebar_slide_resize == SidebarSlideResize::Live
            {
                self.resize_page_live(dt);
            } else if self.chrome.is_sliding() {
                let end = self.chrome.settled_page_rect(&self.config, size).size;
                if let Some(view) = self.active_view()
                    && self.slide_view_size != Some((view, end))
                {
                    if self.slide_view_size.is_none_or(|(v, _)| v != view) {
                        self.slide_snapshot = self.engine.frame(view).map(|f| self.copy_texture(&f.texture));
                        self.slide_fade = None;
                    }
                    self.slide_view_size = Some((view, end));
                    self.engine.resize(view, ViewSize { logical: end, scale: self.scale() });
                }
                // The page's frame at its new size covers the rectangle now: it takes over.
                let (scale, rect) = (self.scale(), self.page_rect().size.x);
                let covered = self.active_view().and_then(|v| self.engine.frame(v)).is_some_and(|f| {
                    let width = f.texture.width() as f32;
                    (width - (end.x * scale).round()).abs() <= 1.0 && width + 0.5 >= rect * scale
                });
                if covered {
                    self.fade_out_slide_snapshot();
                }
            } else {
                self.fade_out_slide_snapshot();
                self.slide_view_size = None;
                self.slide_live_width = None;
                self.slide_prev_width = None;
                self.slide_sent.clear();
                self.slide_seen_width = None;
                self.update_layout();
            }
        }
        if let Some((_, fade)) = &mut self.slide_fade {
            fade.step(dt);
            if fade.value <= SLIDE_FADE_DONE {
                self.slide_fade = None;
            }
        }
        self.menu.track_hover(&mut self.ui, &self.config);
        self.account_menu.track_hover(&mut self.ui, &self.config);
        self.menu.animate(&mut self.ui, dt);
        self.account_menu.animate(&mut self.ui, dt);
        self.sync_now_playing_lift();
        if self.context_menus.is_open()
            && let Some((view, private, surface)) = self.context_row
        {
            let row = self
                .chrome
                .tab_items(surface)
                .into_iter()
                .find(|i| i.view == view && i.private == private)
                .map(|i| i.node);
            self.context_menus.cover_row(&mut self.ui, row);
        }
        self.sync_page_menu();
        self.context_menus.animate(&mut self.ui, &self.config, dt);
        self.add_account.animate(&mut self.ui, dt);
        self.shields_popover.animate(&mut self.ui, dt);
        self.downloads_popover.animate(&mut self.ui, dt);
        let width = self.logical_size().x;
        let address = self.chrome.address_frame(&self.ui);
        let top = self.page_rect().origin.y;
        self.find.follow(&mut self.ui, address, width, top);
        self.find.animate(&mut self.ui, dt, &mut self.engine);
        self.lock_screen.animate(&mut self.ui, dt);
        self.onboarding.animate(&mut self.ui, &self.config, dt);
        self.settings.animate(&mut self.ui, dt);
        let address = self.chrome.address_frame(&self.ui);
        self.passwords.animate(&mut self.ui, &self.config, dt, address);
        self.animate_permissions(dt, address);
        self.update_player(dt);
        self.update_playlist_buttons();
        self.playlist_ui.popover.animate(&mut self.ui, dt);
        if self.playlist_open() {
            self.update_playlist_page(dt);
        } else if self.playlist.playback.is_some() {
            self.playlist_background_tick();
        }
        self.suggestions.animate(&mut self.ui, dt);
        if self.overview.is_open() {
            let query = self.ui.field_text(self.overview.search_field()).unwrap_or_default().to_owned();
            if query != self.overview_query {
                self.overview_query = query;
                self.rebuild_overview();
            }
            let engine = &self.engine;
            self.overview.update(&mut self.ui, dt, |view| {
                let frame = engine.frame(view)?;
                let size = (frame.texture.width(), frame.texture.height());
                Some((frame.texture.create_view(&Default::default()), size))
            });
        }
        self.settings.poll_search(&mut self.ui, &self.config, &self.keymap);
        self.poll_panel_search();
        self.toast.animate(&mut self.ui, &self.config, dt);
        self.update_alert.animate(&mut self.ui, dt);
        self.poll_updates();
        let blocked = self.tooltips_blocked();
        let window = self.logical_size();
        self.tooltip.animate(&mut self.ui, &self.config, &self.keymap, window, blocked, dt);
        self.take_engine_icons();
        self.poll_svg_icons();
        if self.now_art.poll(&self.favicons) {
            self.update_now_playing();
        }
        self.sync_password_icons();
        // Keeps the sidebar's scroll in range when its content got shorter (tabs closed).
        self.chrome.scroll_sections(&mut self.ui, 0.0);
        self.update_suggestions();
        // The lock screen covers the page wherever it is now (the sidebar slides it).
        if self.lock_shown.is_some_and(|shown| shown != self.page_rect()) {
            self.update_lock();
        }
        let hovered = if self.tab_drag.is_dragging() { None } else { self.ui.hovered() };
        self.chrome.update_hover(&mut self.ui, hovered);
        self.update_tab_drag(dt);
        let started = Instant::now();
        self.pace_page_resizes(now);
        self.frame_times.layout += ms_since(started);
        let started = Instant::now();
        self.ui.layout();
        self.frame_times.ui_layout = ms_since(started);
        self.tab_drag.animate_rows(&mut self.ui, &self.chrome, &self.config, dt);
        if let Some(view) = self.display_view() {
            self.engine.request_frame(view);
        }
        self.snapshot_background_tabs();

        // Locked private pages aren't drawn at all (the lock screen covers their place).
        let frame = self.display_view().filter(|_| !self.is_locked()).and_then(|v| self.engine.frame(v));
        self.probe_page_frame(frame.as_ref());
        // The same view while the frame texture stays: an unchanged scene keeps its fingerprint.
        let page_view = frame.as_ref().map(|f| match &self.page_view {
            Some((texture, view)) if *texture == f.texture => view.clone(),
            _ => {
                let view = f.texture.create_view(&Default::default());
                self.page_view = Some((f.texture.clone(), view.clone()));
                view
            }
        });
        let slide_frame = frame.as_ref().and_then(|f| self.slide_page_frame(&f.texture));
        let slide_view = slide_frame.as_ref().map(|t| t.create_view(&Default::default()));
        let fade = self.slide_fade.as_ref().filter(|_| frame.is_some()).map(|(t, a)| (t.clone(), a.value));
        let fade_view = fade.as_ref().map(|(t, _)| t.create_view(&Default::default()));
        let s = self.scale();
        let page = self.page_rect();
        let tint_dt = self.last_tint_step.elapsed().as_secs_f32();
        self.last_tint_step = Instant::now();
        self.adapt_appearance(page_view.as_ref(), tint_dt);
        let page_px = Rect { origin: page.origin * s, size: page.size * s };
        let anchor = self.slide_anchor();

        let theme = self.ui.theme();
        let mut layers = Vec::new();
        // The page ends where its rectangle ends (below the bar, beside a docked sidebar), with
        // rounded corners; square in page fullscreen (video).
        if let (Some(texture), Some(view)) = (&slide_frame, &slide_view) {
            push_slide_page(
                &mut layers,
                view,
                texture,
                page_px,
                self.chrome.page_corner_radius(&self.config) * s,
                anchor,
            );
        } else if let (Some(view), Some(frame)) = (&page_view, &frame)
            && self.live_stretch
            && (frame.texture.width() as f32 - page_px.size.x).abs() > 1.0
        {
            // A live-resized page (see `resize_page_live`): Chromium's frames trail the
            // rectangle by a few frames, so the latest one is drawn stretched across it. The
            // difference is a few percent: content keeps its place relative to the rectangle
            // (centred or left-aligned) and no edge is ever uncovered or cropped.
            tracing::trace!(serial = frame.serial, tex = ?(frame.texture.width(), frame.texture.height()), page = ?page_px, "page frame stretched");
            let mut page = ImageDraw::new(view, page_px);
            page.radius = self.chrome.page_corner_radius(&self.config) * s;
            layers.push(Layer::Image(page));
        } else if let (Some(view), Some(frame)) = (&page_view, &frame) {
            if self.live_stretch && !self.chrome.is_sliding() {
                // The frame of the slide's final size arrived: the page is pixel-exact again.
                self.live_stretch = false;
            }
            // Drawn at its own pixel size from the page's top-left, never stretched: while the
            // page is resized (sidebar sliding) Chromium's frames lag a little, and stretching
            // the older size into the new rectangle made the page wobble and flicker.
            let tex = Vec2::new(frame.texture.width() as f32, frame.texture.height() as f32);
            tracing::trace!(serial = frame.serial, ?tex, page = ?page_px.size, "page frame composited");
            let shown = page_px.size.min(tex);
            let radius = self.chrome.page_corner_radius(&self.config) * s;
            let texel = Vec2::ONE / tex.max(Vec2::ONE);
            // The rectangle grew faster than Chromium's frames (the first frame of a new size
            // takes a moment; F11 grows it in both directions): the frame's right column and
            // bottom row are stretched over the uncovered part, so it never shows the
            // background. Both lie under the page, the column across the whole rectangle.
            if shown.x < page_px.size.x {
                let mut edge = ImageDraw::new(view, page_px);
                // Left of the page's scrollbar, whose track would paint a grey band.
                let column = (1.0 - EDGE_SAMPLE_INSET * s * texel.x).max(0.0);
                edge.uv =
                    Rect { origin: Vec2::new(column, 0.0), size: Vec2::new(texel.x, shown.y * texel.y) };
                edge.radius = radius;
                layers.push(Layer::Image(edge));
            }
            if shown.y < page_px.size.y {
                let mut edge = ImageDraw::new(
                    view,
                    Rect { origin: page_px.origin, size: Vec2::new(shown.x, page_px.size.y) },
                );
                edge.uv = Rect {
                    origin: Vec2::new(0.0, 1.0 - texel.y),
                    size: Vec2::new(shown.x * texel.x, texel.y),
                };
                edge.radius = radius;
                layers.push(Layer::Image(edge));
            }
            let mut page = ImageDraw::new(view, Rect { origin: page_px.origin, size: shown });
            page.uv = Rect { origin: Vec2::ZERO, size: shown * texel };
            page.radius = radius;
            layers.push(Layer::Image(page));
        }
        if let (Some((texture, opacity)), Some(view)) = (&fade, &fade_view) {
            let first = layers.len();
            push_slide_page(
                &mut layers,
                view,
                texture,
                page_px,
                self.chrome.page_corner_radius(&self.config) * s,
                anchor,
            );
            for layer in &mut layers[first..] {
                if let Layer::Image(image) = layer {
                    image.opacity = opacity.clamp(0.0, 1.0);
                }
            }
        }
        let started = Instant::now();
        self.ui.paint(&mut layers);
        self.frame_times.paint = ms_since(started);
        if std::mem::take(&mut self.flat_audit) {
            if self.config.glass.is_flat() {
                let found = axion_ui::flat_violations(&layers, s);
                tracing::info!("flat-audit: {} findings", found.len());
                for line in found {
                    tracing::warn!("flat-audit: {line}");
                }
            } else {
                tracing::warn!("flat-audit: the window is not in flat mode (glass.style)");
            }
        }
        if tracing::enabled!(tracing::Level::TRACE) {
            let kinds: Vec<String> = layers
                .iter()
                .map(|l| match l {
                    Layer::Image(_) => "image".to_owned(),
                    Layer::Glass { shapes, params } => {
                        format!("glass×{}(blur {:.0})", shapes.len(), params.blur_radius)
                    }
                    Layer::Content(c) => {
                        format!(
                            "content(q{} i{} icons{} t{})",
                            c.quads.len(),
                            c.images.len(),
                            c.icons.len(),
                            c.text.len()
                        )
                    }
                })
                .collect();
            tracing::trace!(?kinds, "layers");
        }
        let clear = self
            .window_tint
            .filter(|_| !self.overview.is_open() && !self.is_locked())
            .unwrap_or(theme.color(Paint::Ground));
        let scene = Scene { clear, layers };
        let size = (self.surface_config.width, self.surface_config.height);
        // A frame that would look like the one on screen isn't drawn again (see
        // `ui.skip_unchanged_frames`): the scene's fingerprint, the window's size and every
        // page frame so far (pages' textures change content without changing identity).
        let fingerprint = {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            scene.fingerprint().hash(&mut h);
            size.hash(&mut h);
            self.engine.counters().page_frames.hash(&mut h);
            h.finish()
        };
        let capture_due = self.capture.as_ref().is_some_and(|(_, at)| Instant::now() >= *at);
        if self.config.ui.skip_unchanged_frames && self.shown_fingerprint == Some(fingerprint) && !capture_due
        {
            drop(scene);
            self.frames_skipped += 1;
            self.idle_redraw_at = Some(now + self.frame_interval);
            return Ok(());
        }
        self.idle_redraw_at = None;
        if tracing::enabled!(tracing::Level::TRACE) {
            // Debug aid: which layers changed since the frame shown (why it was drawn).
            let layers = scene.layer_fingerprints();
            let changed: Vec<usize> = (0..layers.len().max(self.shown_layers.len()))
                .filter(|&i| layers.get(i) != self.shown_layers.get(i))
                .collect();
            tracing::trace!(?changed, count = layers.len(), "frame drawn: layers changed");
            self.shown_layers = layers;
        }
        let started = Instant::now();
        let _op = crate::stall::op("frame: acquire and render");
        let acquired = self.surface.get_current_texture();
        self.frame_times.acquire = ms_since(started);
        let surface = match acquired {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                self.shown_fingerprint = None;
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.shown_fingerprint = None;
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("surface validation error"),
        };
        let output = surface.texture.create_view(&Default::default());
        let started = Instant::now();
        if let Some(log) = &mut self.perf_log {
            log.gpu_begin(&self.device, &self.queue);
        }
        self.compositor.render(&scene, &mut self.ui.fonts(), &output, size);
        if let Some(log) = &mut self.perf_log {
            log.gpu_end(&self.device, &self.queue);
        }
        self.frame_times.render = ms_since(started);
        // Kept until the next comparison: no new texture view can take one of these identities.
        self.shown_textures = scene.textures();
        drop(scene);
        let started = Instant::now();
        self.window.pre_present_notify();
        self.queue.present(surface);
        self.frame_times.present = ms_since(started);
        if self.shown_fingerprint.is_none() && self.frames_presented == 0 {
            tracing::info!("first frame presented");
        }
        self.frames_presented += 1;
        self.shown_fingerprint = Some(fingerprint);
        self.probe_presented();

        if let Some((path, at)) = &self.capture
            && Instant::now() >= *at
        {
            if self.capture_frame == 0 {
                let (w, h, pixels) = self.compositor.read_back().context("reading back the frame")?;
                image::RgbaImage::from_raw(w, h, pixels)
                    .context("frame has the wrong size")?
                    .save(path)
                    .with_context(|| format!("saving {}", path.display()))?;
            }
            tracing::info!(path = %path.display(), "saved screenshot");
            // Debug aid: AXION_CAPTURE_FRAMES=N also saves the next N frames ("shot-1.png" …).
            let wanted: u32 =
                std::env::var("AXION_CAPTURE_FRAMES").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
            if self.capture_frame < wanted {
                self.capture_frame += 1;
                if let Some((w, h, pixels)) = self.compositor.read_back()
                    && let Some(img) = image::RgbaImage::from_raw(w, h, pixels)
                {
                    self.captured_frames.push(img);
                }
                return Ok(());
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("capture").to_owned();
            for (i, img) in std::mem::take(&mut self.captured_frames).into_iter().enumerate() {
                let _ = img.save(path.with_file_name(format!("{stem}-{}.png", i + 1)));
            }
            // The PiP window too, next to it ("shot.png" → "shot-pip.png").
            if let Some((w, h, pixels)) =
                self.pip.as_ref().filter(|p| p.is_active()).and_then(PipWindow::read_back)
            {
                let pip_path = path.with_file_name(format!(
                    "{}-pip.png",
                    path.file_stem().and_then(|s| s.to_str()).unwrap_or("capture")
                ));
                if let Some(img) = image::RgbaImage::from_raw(w, h, pixels) {
                    let _ = img.save(&pip_path);
                }
            }
            bail!(CaptureDone);
        }
        Ok(())
    }

    /// Shows suggestions while the address field is being edited and differs from the page.
    /// After a keystroke in the address bar: completes what was typed inline from history, the
    /// completed part selected so the next keystroke replaces it (Backspace removes it).
    fn autocomplete_address(&mut self) {
        let field = self.chrome.address_field;
        let typed = self.ui.field_text(field).unwrap_or_default().to_owned();
        // Only while typing at the end of the text.
        if self.ui.field_caret(field) != Some((typed.len(), None)) {
            return;
        }
        let Some(completion) = self.store.as_ref().and_then(|s| s.autocomplete(&typed).ok().flatten()) else {
            return;
        };
        if completion.len() > typed.len() && completion.starts_with(&typed) {
            self.ui.set_field_text_selecting(field, &completion, typed.len()..completion.len());
        }
    }

    /// What the user typed in the address bar, without an inline completion.
    fn typed_address(&self) -> String {
        let field = self.chrome.address_field;
        let text = self.ui.field_text(field).unwrap_or_default();
        match self.ui.field_caret(field) {
            Some((_, Some(selection))) if selection.end == text.len() => text[..selection.start].to_owned(),
            _ => text.to_owned(),
        }
    }

    fn update_suggestions(&mut self) {
        let editing = self.ui.focused() == Some(self.chrome.address_field) && !self.overview.is_open();
        let text = self.typed_address();
        let typed = editing && self.tabs.active().is_none_or(|t| t.url != text);
        if !typed {
            if self.suggestions.is_visible() {
                self.suggestions.hide(&mut self.ui, &self.config);
            }
            let current = self.tabs.active().map(|t| t.url.clone());
            self.chrome.set_address_icon_override(&mut self.ui, None, current.as_deref(), &self.favicons);
            return;
        }
        let anchor = self.chrome.address_frame(&self.ui);
        let (store, search_url) = (self.store.as_ref(), self.config.general.search_url.clone());
        self.suggestions.update(&mut self.ui, &self.config, &text, anchor, &self.favicons, || {
            suggest(&text, store, &search_url, unix_now())
        });
        self.suggestions.reposition(&mut self.ui, anchor);
        let current = self.tabs.active().map(|t| t.url.clone());
        self.chrome.set_address_icon_override(
            &mut self.ui,
            Some(self.suggestions.typing_icon_url()),
            current.as_deref(),
            &self.favicons,
        );
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

    /// Capture aid (throwaway profiles only): the account gets the vault's test password.
    fn set_test_password(&mut self) {
        if !self.passwords.has_password() {
            let _ =
                self.passwords.set_up_password(&mut self.ui, &self.config, crate::passwords::TEST_PASSWORD);
            self.settings.set_account(self.account_info());
            self.sync_account_lock();
        }
    }

    /// Pages render at the refresh rate of the window's monitor (or the configured rate).
    fn sync_frame_rate(&mut self) {
        let refresh = monitor_refresh(&self.window);
        self.engine.set_frame_rate(page_frame_rate(&self.config, &self.window));
        self.frame_interval = frame_rate::display_interval(refresh);
        self.refresh_watch.applied(refresh, Instant::now());
    }

    /// Follows the refresh rate of the window's monitor: unknown on Wayland until the window
    /// is shown, and it changes when the window moves to another monitor (which Wayland
    /// doesn't report as a move).
    fn watch_refresh_rate(&mut self) {
        let now = Instant::now();
        if self.refresh_watch.due(now) && self.refresh_watch.changed(monitor_refresh(&self.window), now) {
            self.sync_frame_rate();
            tracing::debug!(interval = ?self.frame_interval, "monitor refresh rate changed");
        }
    }

    /// When the next frame should run, if not at once: after a frame that was skipped (nothing
    /// changed, nothing presented, so nothing waited for the display), one display refresh later.
    pub fn redraw_at(&self) -> Option<Instant> {
        self.idle_redraw_at
    }

    /// While the sidebar slides: the frame to show for the page, drawn from the page
    /// rectangle's right edge (see `push_slide_page`). The page's current frame, unless it is
    /// narrower than the rectangle and the frame from before the slide isn't (the sidebar is
    /// opening: the page keeps its wider layout under the sidebar until the slide settles).
    /// `None` when not sliding.
    fn slide_page_frame(&self, current: &wgpu::Texture) -> Option<wgpu::Texture> {
        self.slide_view_size?;
        let width = self.page_rect().size.x * self.scale();
        let covers = |t: &wgpu::Texture| t.width() as f32 + 0.5 >= width;
        match &self.slide_snapshot {
            Some(snapshot) if !covers(current) && covers(snapshot) => Some(snapshot.clone()),
            _ => Some(current.clone()),
        }
    }

    /// The page's frame at its new size took over from the one before the slide. If the page
    /// kept that older, wider layout under the opening sidebar, it fades into the new one
    /// instead of jumping. (A closing sidebar uncovers the page's new, wider frame as soon as it
    /// comes, a frame or two into the slide: a fade there would only ghost the moving page.)
    fn fade_out_slide_snapshot(&mut self) {
        let width = self.page_rect().size.x * self.scale();
        if let Some(snapshot) = self.slide_snapshot.take().filter(|t| t.width() as f32 > width + 0.5) {
            let mut fade = axion_physics::Animated::new(
                1.0,
                crate::drip::spring(&self.config, self.config.motion.snappy),
            );
            fade.target = 0.0;
            self.slide_fade = Some((snapshot, fade));
        }
    }

    /// `sidebar_slide_resize = "live"`: while the sidebar slides, the shown page follows the
    /// rectangle, resized again whenever its target moved by `sidebar_slide_resize_step`.
    /// Chromium's frame of a size arrives a few frames after the resize (`page_lag`, measured
    /// from the frames that come), so the page is resized to where the rectangle will be by then
    /// (its width plus its speed times the lag, never beyond the settled width). The engine
    /// keeps the last complete frame until one of the new size arrives and the page is drawn
    /// stretched over the rectangle (see `redraw_frame`): no blank frames, no jump when the
    /// slide settles.
    fn resize_page_live(&mut self, dt: f32) {
        let Some(view) = self.active_view() else { return };
        let now = Instant::now();
        let scale = self.scale();
        // How long the page's latest frame took: the one that now arrives was asked for at the
        // size it has.
        if let Some(frame) = self.engine.frame(view) {
            let arrived = frame.texture.width() as f32;
            if self.slide_seen_width != Some(arrived) {
                self.slide_seen_width = Some(arrived);
                let asked = self.slide_sent.iter().rposition(|&(_, w)| (w - arrived).abs() <= 1.0);
                if let Some(i) = asked {
                    let lag = now.duration_since(self.slide_sent[i].0).as_secs_f32();
                    let known = self.page_lag.unwrap_or(self.config.layout.sidebar_slide_lead);
                    self.page_lag = Some(known + (lag - known) * PAGE_LAG_SMOOTHING);
                    tracing::trace!(arrived, lag, known, "page frame of a live size arrived");
                    self.slide_sent.drain(..=i);
                }
            }
        }
        let rect = self.view_size();
        let end = self.chrome.settled_page_rect(&self.config, self.logical_size()).size.x;
        let speed = self.slide_prev_width.map_or(0.0, |p| (rect.logical.x - p) / dt.max(1e-3));
        self.slide_prev_width = Some(rect.logical.x);
        let lag = self.page_lag.unwrap_or(self.config.layout.sidebar_slide_lead);
        let ahead = rect.logical.x + speed * lag;
        let target = ahead.clamp(rect.logical.x.min(end), rect.logical.x.max(end));
        let step = self.config.layout.sidebar_slide_resize_step.max(0.5);
        if self.slide_live_width.is_none_or(|w| (w - target).abs() >= step) {
            self.slide_live_width = Some(target);
            self.live_stretch = true;
            self.slide_sent.push((now, (target * scale).round()));
            self.engine.resize(view, ViewSize { logical: Vec2::new(target, rect.logical.y), scale });
        }
    }

    /// Which point of the page's frame stays on the page rectangle's while the sidebar slides:
    /// its right edge, or its middle for the start page, whose content is centred: it follows
    /// the rectangle instead of standing still and jumping once the page has re-laid itself out.
    fn slide_anchor(&self) -> f32 {
        let home = self.tabs.active().is_some_and(|t| is_home_url(&t.url));
        if home { SLIDE_ANCHOR_CENTRED } else { SLIDE_ANCHOR_STILL }
    }

    /// A copy of a page frame (Chromium reuses the textures of its later frames).
    fn copy_texture(&self, texture: &wgpu::Texture) -> wgpu::Texture {
        let copy = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("page before the slide"),
            size: texture.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(texture.as_image_copy(), copy.as_image_copy(), texture.size());
        self.queue.submit([encoder.finish()]);
        copy
    }

    /// When the event loop should wake up next at the latest (the engine's next pump).
    pub fn next_wakeup(&self) -> Option<Instant> {
        self.engine.next_pump()
    }

    /// Typing probe: types the next character when it is due (between frames, as a key event
    /// would arrive), and logs the summary once everything is typed and shown.
    pub fn tick_probe(&mut self) {
        let now = Instant::now();
        let Some(probe) = &self.probe else { return };
        if let Some(c) = probe.char_due(now) {
            let target = probe.spec.target;
            match target {
                typing_probe::Target::Page { .. } => {
                    if let Some(view) = self.active_view() {
                        for key in input::typed_keys(&c.to_string()) {
                            self.engine.send_input(view, &key);
                        }
                    }
                }
                typing_probe::Target::Native => {
                    let (_, events) = self.ui.handle(UiInput::Text(c.to_string()));
                    self.handle_ui_events(events);
                }
            }
            if let Some(probe) = &mut self.probe {
                probe.tracker.sent(now);
            }
            // Like a key event: the next frame runs at once.
            self.idle_redraw_at = None;
            return;
        }
        let total = probe.spec.text.chars().count();
        let last_at = probe.started
            + probe.spec.start_after
            + probe.spec.interval * total as u32
            + std::time::Duration::from_secs(2);
        if !probe.reported && probe.tracker.sent_count() == total && now >= last_at {
            let summary = probe.tracker.summary();
            if let Some(probe) = &mut self.probe {
                probe.reported = true;
            }
            match summary {
                Some(s) => {
                    let ms: Vec<String> = s
                        .latency
                        .iter()
                        .map(|l| l.map_or("never".to_owned(), |d| format!("{:.0}", d.as_secs_f32() * 1000.0)))
                        .collect();
                    tracing::info!(latency_ms = %ms.join(" "), vanished = s.regressions, "typing probe");
                    let frames: Vec<String> = s.shown.iter().map(|(t, n)| format!("{t}:{n}")).collect();
                    tracing::debug!(frames = %frames.join(" "), "typing probe frames");
                }
                None => tracing::info!("typing probe: nothing measured"),
            }
        }
    }

    /// Typing probe, page target: samples the field in every new page frame.
    fn probe_page_frame(&mut self, frame: Option<&axion_engine::Frame>) {
        let Some(probe) = &mut self.probe else { return };
        let typing_probe::Target::Page { region } = probe.spec.target else { return };
        let Some(frame) = frame.filter(|f| f.serial != probe.last_serial) else { return };
        probe.last_serial = frame.serial;
        let Some(pixels) = typing_probe::read_region(&self.device, &self.queue, &frame.texture, region)
        else {
            return;
        };
        let started = probe.started + probe.spec.start_after;
        if Instant::now() < started || probe.tracker.sent_count() == 0 {
            probe.baseline = Some(pixels);
            return;
        }
        if let Some(baseline) = &probe.baseline {
            probe.tracker.sample(Instant::now(), typing_probe::ink(baseline, &pixels));
        }
    }

    /// Typing probe, chrome target: the frame that follows a typed character is presented.
    fn probe_presented(&mut self) {
        let Some(probe) = &mut self.probe else { return };
        if probe.spec.target == typing_probe::Target::Native && probe.tracker.sent_count() > 0 {
            let typed = probe.tracker.sent_count() as u32;
            probe.tracker.sample(Instant::now(), typed);
        }
    }

    pub fn request_redraw(&mut self) {
        if let Some(log) = &mut self.perf_log {
            log.redraw_requested();
        }
        self.window.request_redraw();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Bookmarks,
    History,
    ReadingList,
    Downloads,
}

/// Signals that `--capture` finished (not an error for the user).
impl Drop for Browser {
    fn drop(&mut self) {
        self.playlist_flush_all();
        if self.config_dirty_since.take().is_some()
            && let Err(e) = axion_config::save_config(&self.paths, &self.config)
        {
            tracing::error!("saving settings failed: {e}");
        }
        self.shields.save(self.store.as_ref());
        self.save_session_if_due(true);
        Downloads::on_quit(self.store.as_ref(), &self.config, &mut self.engine);
    }
}

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

/// Shortcuts that still work while private browsing is shown locked: nothing that shows,
/// changes or reaches the locked pages (switching to the normal tabs stays possible through
/// the sidebar and the overview).
fn allowed_while_locked(action: Action) -> bool {
    matches!(
        action,
        Action::ToggleSidebar
            | Action::TabOverview
            | Action::OpenSettings
            | Action::ToggleFullscreen
            | Action::Quit
    )
}

/// Actions that leave the Playlist page open (the others — new tab, address bar, reload … —
/// are about the tab below it, so the page gives way first).
fn keeps_playlist(action: Action) -> bool {
    action.is_media()
        || matches!(
            action,
            Action::ToggleSidebar
                | Action::OpenPlaylist
                | Action::OpenSettings
                | Action::ShowBookmarks
                | Action::ShowHistory
                | Action::OpenPasswords
                | Action::Lock
                | Action::ToggleFullscreen
                | Action::Quit
        )
}

/// Chrome controls that still work while private browsing is shown locked.
fn chrome_allowed_while_locked(action: ChromeAction) -> bool {
    !matches!(
        action,
        ChromeAction::Back
            | ChromeAction::Forward
            | ChromeAction::ReloadOrStop
            | ChromeAction::Shield
            | ChromeAction::EditAddress
            | ChromeAction::PageMenu
            | ChromeAction::Share
            | ChromeAction::NewTab
            | ChromeAction::MuteTab
            | ChromeAction::DownloadsButton
            | ChromeAction::SelectTab(_)
            | ChromeAction::CloseTab(_)
            | ChromeAction::OpenBookmark(_)
            | ChromeAction::BookmarksOverflow
    )
}

/// A tab by view, in either tab group.
fn find_tab<'a>(tabs: &'a Tabs, parked: &'a Tabs, view: ViewId) -> Option<&'a Tab> {
    tabs.all().iter().chain(parked.all()).find(|t| t.view == view)
}

fn tab_mut<'a>(tabs: &'a mut Tabs, parked: &'a mut Tabs, view: ViewId) -> Option<&'a mut Tab> {
    if tabs.all().iter().any(|t| t.view == view) { tabs.by_view_mut(view) } else { parked.by_view_mut(view) }
}

/// How the media bridge treats page players: Axion's player only in fullscreen, trusted sites
/// keep theirs.
fn media_replace(config: &Config) -> axion_media::BridgeOptions {
    let media = &config.media;
    axion_media::BridgeOptions {
        own_fullscreen: media.fullscreen_player,
        trusted_sites: media.trusted_player_sites.clone(),
    }
}

/// The part (0–1) of a frame of size `tex` that shows a video of `aspect` letterboxed in it.
fn letterbox(tex: Vec2, aspect: f32) -> Rect {
    let frame_aspect = tex.x / tex.y.max(1.0);
    let size = if aspect > frame_aspect {
        Vec2::new(1.0, frame_aspect / aspect)
    } else {
        Vec2::new(aspect / frame_aspect, 1.0)
    };
    Rect { origin: (Vec2::ONE - size) / 2.0, size }
}

/// What runs in every document: the media bridge and the password autofill script.
fn page_script(config: &Config) -> String {
    let bridge = axion_media::bridge_script(&bridge_secret(), &media_replace(config));
    format!("{bridge}\n{}", axion_vault::autofill::page_script())
}

/// A per-run secret that keeps the media bridge's frame messages apart from the page's.
fn bridge_secret() -> String {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{nanos:x}{:x}", std::process::id())
}

/// The page zoom as shown in the page menu ("100%").
fn zoom_label(zoom: f64) -> String {
    format!("{:.0}%", ZOOM_BASE.powf(zoom) * 100.0)
}

/// Milliseconds since `started` (frame timings).
fn ms_since(started: Instant) -> f32 {
    started.elapsed().as_secs_f32() * 1000.0
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Draws a page frame while the sidebar slides: at its own pixel size, its right edge on the
/// page rectangle's (which stays put while the left edge follows the sidebar), cropped to the
/// rectangle. A frame narrower than the rectangle (the sidebar is closing and the wider frame
/// hasn't come yet, a frame or two) has its left column stretched over the rest.
fn push_slide_page<'a>(
    layers: &mut Vec<Layer<'a>>,
    view: &'a wgpu::TextureView,
    texture: &wgpu::Texture,
    page: Rect,
    radius: f32,
    anchor: f32,
) {
    let tex = Vec2::new(texture.width() as f32, texture.height() as f32);
    let texel = Vec2::ONE / tex.max(Vec2::ONE);
    let shown = page.size.min(tex);
    // The frame's `anchor` point (0 left, 0.5 middle, 1 right) stays on the rectangle's: the
    // frame's left edge is `anchor * (rectangle - frame)` from the rectangle's.
    let left = anchor * (page.size.x - tex.x);
    // The leftmost part of the frame that is shown.
    let skip = (-left).max(0.0);
    if shown.x < page.size.x {
        let mut edge =
            ImageDraw::new(view, Rect { origin: page.origin, size: Vec2::new(page.size.x, shown.y) });
        edge.uv = Rect { origin: Vec2::ZERO, size: Vec2::new(texel.x, shown.y * texel.y) };
        edge.radius = radius;
        layers.push(Layer::Image(edge));
    }
    let mut image = ImageDraw::new(
        view,
        Rect { origin: Vec2::new(page.origin.x + left.max(0.0), page.origin.y), size: shown },
    );
    image.uv = Rect { origin: Vec2::new(skip * texel.x, 0.0), size: shown * texel };
    image.radius = radius;
    layers.push(Layer::Image(image));
}

/// The refresh rate (millihertz) of the window's monitor, if known (on Wayland only once the
/// window has been shown, see `frame_rate`).
fn monitor_refresh(window: &Window) -> Option<u32> {
    window.current_monitor().and_then(|m| m.refresh_rate_millihertz())
}

/// Frames per second pages render at (see `frame_rate::page_frame_rate`).
fn page_frame_rate(config: &Config, window: &Window) -> u32 {
    frame_rate::page_frame_rate(
        config.tabs.max_frame_rate,
        config.tabs.max_frame_rate_cap,
        monitor_refresh(window),
    )
}

fn is_dark(config: &Config, window: &Window, desktop_dark: Option<bool>) -> bool {
    match config.appearance.theme {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::Auto => desktop_dark.unwrap_or(window.theme() == Some(winit::window::Theme::Dark)),
    }
}

/// What the Now Playing card and the desktop show, gathered from the playing tab.
struct NowPlayingInfo {
    private: bool,
    player: axion_media::Player,
    title: String,
    artist: String,
    album: String,
    /// The media session's artwork, the page's preview image.
    artwork: String,
    image: String,
    url: String,
    host: String,
    can_prev: bool,
    can_next: bool,
}

#[cfg(test)]
mod tests {
    use crate::context_menu::CONTEXT_MENU_BASE;
    use crate::menu::{ACCOUNT_MENU_BASE, MAX_ITEMS, PAGE_MENU_BASE};

    use super::*;
    use axion_ui::ActionId;

    /// Every click goes to the first handler that claims its action id, so two parts of the
    /// UI sharing ids means a click on one does the other's thing. (The overview's buttons
    /// once shared the chrome's ids: the speaker opened a tab, the bars' pill switched to
    /// private tabs; and Passwords swallowed the sidebar's rows of the other tab group.)
    #[test]
    fn action_ids_are_never_shared() {
        for id in 0..60_000 {
            let id = ActionId(id);
            let claims = [
                ChromeAction::from_id(id).is_some(),
                OverviewAction::from_id(id).is_some(),
                Passwords::owns_action(id),
                crate::permission_prompt::PermissionPrompt::owns(id),
                LockScreen::is_lock_action(id),
                SettingsAction::from_id(id).is_some(),
                PlayerControl::from_id(id).is_some(),
                (CONTEXT_MENU_BASE..CONTEXT_MENU_BASE + 5 * MAX_ITEMS as u32).contains(&id.0),
                (PAGE_MENU_BASE..ACCOUNT_MENU_BASE + MAX_ITEMS as u32).contains(&id.0),
                crate::playlist_page::PageAction::owns(id),
                (crate::playlist_page::MENU_BASE..crate::playlist_page::MENU_BASE + 2 * MAX_ITEMS as u32)
                    .contains(&id.0),
            ];
            assert!(
                claims.iter().filter(|&&c| c).count() <= 1,
                "action {} is claimed twice: {claims:?}",
                id.0
            );
        }
    }

    #[test]
    fn a_locked_private_window_keeps_its_pages_out_of_reach() {
        for action in
            [Action::NewTab, Action::FocusAddressBar, Action::CloseTab, Action::NextTab, Action::Reload]
        {
            assert!(!allowed_while_locked(action), "{action:?}");
        }
        // The way back to the normal tabs stays open.
        assert!(allowed_while_locked(Action::TabOverview) && allowed_while_locked(Action::ToggleSidebar));
        for action in
            [ChromeAction::EditAddress, ChromeAction::Shield, ChromeAction::Share, ChromeAction::SelectTab(0)]
        {
            assert!(!chrome_allowed_while_locked(action), "{action:?}");
        }
        for action in [
            ChromeAction::SelectOtherTab(0),
            ChromeAction::ToggleTabsSection,
            ChromeAction::Overview,
            ChromeAction::Private,
            ChromeAction::Account,
        ] {
            assert!(chrome_allowed_while_locked(action), "{action:?}");
        }
    }
}
