//! The web engine behind Axion's tabs.
//!
//! [`Engine`] is what the rest of the browser talks to: it creates off-screen *views* (one
//! per tab), forwards input, and hands back each view's latest frame as a wgpu texture for
//! the compositor. Everything else (title, URL, loading state, popups, cursor) comes back as
//! [`EngineEvent`]s from [`Engine::pump`]. The Chromium backend lives in [`cef`]; another
//! engine (e.g. Servo) can be added behind the same trait.

pub mod cef;
pub mod download;
mod input;
pub mod internal;
mod policy;
mod transfer;

pub use download::{ClearData, DownloadAction, DownloadInfo, DownloadOptions, DownloadState};
pub use input::{CursorKind, InputEvent, Modifiers, MouseButton};
pub use internal::{INTERNAL_SCHEME, InternalPages, InternalRequest, InternalResponse};
pub use policy::{NavigationRequest, RequestPolicy, RequestVerdict, ResourceKind, ResourceRequest};
pub use transfer::{DropData, FindRequest, PasteData, PastedFile};

use glam::Vec2;

/// Console messages starting with this are [`EngineEvent::PageMessage`]s.
pub const PAGE_MESSAGE_PREFIX: &str = "\u{1}axion:";

/// Identifies one view (tab) for the lifetime of the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId(pub u32);

/// A view's size in logical pixels, and the scale factor to physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSize {
    pub logical: Vec2,
    pub scale: f32,
}

impl ViewSize {
    pub fn physical(&self) -> (u32, u32) {
        let p = (self.logical * self.scale).round();
        (p.x.max(1.0) as u32, p.y.max(1.0) as u32)
    }
}

/// One `<link rel="icon">` (or apple-touch-icon) of a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconLink {
    pub url: String,
    /// The link's `media` attribute (empty: all), e.g. `(prefers-color-scheme: dark)`.
    pub media: String,
    /// The link's `sizes` attribute (empty if none).
    pub sizes: String,
}

/// A `<meta name="theme-color">` of a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeColor {
    /// sRGB, 0–255.
    pub rgb: [u8; 3],
    /// The tag's `media` attribute (empty: all).
    pub media: String,
}

/// What a page's `<head>` says about its icons and colour.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageMeta {
    pub icons: Vec<IconLink>,
    pub theme_colors: Vec<ThemeColor>,
}

/// Where a page asked a new view to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewViewKind {
    ForegroundTab,
    BackgroundTab,
    /// `window.open` with window features, or a new window.
    Popup,
}

/// Something that happened in a view.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    AddressChanged {
        view: ViewId,
        url: String,
    },
    TitleChanged {
        view: ViewId,
        title: String,
    },
    LoadingStateChanged {
        view: ViewId,
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// 0.0–1.0.
    LoadProgress {
        view: ViewId,
        progress: f64,
    },
    FaviconUrlsChanged {
        view: ViewId,
        urls: Vec<String>,
    },
    /// The page's icon links (with their `media` queries) and theme colours, reported when the
    /// document is parsed and whenever they change (also after a colour scheme switch).
    PageMetaChanged {
        view: ViewId,
        meta: PageMeta,
    },
    /// A message from one of Axion's own pages (`axion://<host>/`) through its privileged
    /// channel (see [`internal`]); never from web content.
    InternalMessage {
        view: ViewId,
        host: String,
        message: String,
    },
    /// A favicon requested with [`Engine::fetch_favicon`] arrived (RGBA, straight alpha).
    /// A favicon requested with [`Engine::fetch_favicon`] couldn't be loaded or decoded.
    FaviconFailed {
        view: ViewId,
        url: String,
    },
    FaviconReady {
        view: ViewId,
        url: String,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    CursorChanged {
        view: ViewId,
        cursor: CursorKind,
    },
    FullscreenRequested {
        view: ViewId,
        fullscreen: bool,
    },
    /// The page tried to open a new tab or window. The engine never opens one by itself;
    /// the browser decides (that's where popup blocking happens). `user_gesture` is false for
    /// script-initiated opens without a click.
    NewViewRequested {
        opener: ViewId,
        url: String,
        kind: NewViewKind,
        user_gesture: bool,
        /// The URL of the frame that asked, if it isn't the page itself (an iframe).
        opener_frame: Option<String>,
    },
    /// A message from the page script (a console message starting with
    /// [`PAGE_MESSAGE_PREFIX`]; the page's own console output isn't reported).
    PageMessage {
        view: ViewId,
        message: String,
    },
    /// The [`RequestPolicy`] blocked a request of the view's page.
    RequestBlocked {
        view: ViewId,
        url: String,
    },
    /// The [`RequestPolicy`] stopped the view's page from navigating (a redirect).
    NavigationBlocked {
        view: ViewId,
        url: String,
    },
    /// A download started, progressed or finished.
    DownloadUpdated(DownloadInfo),
    /// The sites (hosts) that store cookies, with their counts — the answer to
    /// [`Engine::request_site_data`].
    SiteData(Vec<(String, usize)>),
    /// Find in page (see [`Engine::find`]) counted matches: `active` is the 1-based number of
    /// the selected one (0: none). Arrives several times while a long page is searched;
    /// `final_update` marks the last one for the search.
    FindResult {
        view: ViewId,
        active: u32,
        count: u32,
        final_update: bool,
    },
    /// The view is gone (after [`Engine::close_view`], or the engine had to drop it).
    Closed {
        view: ViewId,
    },
}

/// A view's most recent frame.
#[derive(Clone, Debug)]
pub struct Frame {
    pub texture: wgpu::Texture,
    /// Increments whenever a new frame arrives.
    pub serial: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("the engine failed to initialize: {0}")]
    Init(String),
    #[error("could not create a view: {0}")]
    CreateView(String),
}

/// An off-screen web engine.
pub trait Engine {
    /// Creates a view. Private views share one in-memory profile: nothing they do (cookies,
    /// cache, storage) touches the disk or the normal profile.
    fn create_view(&mut self, url: &str, size: ViewSize, private: bool) -> Result<ViewId, EngineError>;
    fn close_view(&mut self, view: ViewId);

    fn navigate(&mut self, view: ViewId, url: &str);
    fn go_back(&mut self, view: ViewId);
    fn go_forward(&mut self, view: ViewId);
    fn reload(&mut self, view: ViewId, ignore_cache: bool);
    fn stop(&mut self, view: ViewId);
    /// Chromium zoom level: 0 is 100%, each step is ×1.2.
    fn set_zoom_level(&mut self, view: ViewId, level: f64);

    fn resize(&mut self, view: ViewId, size: ViewSize);
    /// Hidden views stop rendering and get throttled (background tabs).
    fn set_hidden(&mut self, view: ViewId, hidden: bool);
    /// JavaScript run in every frame of every view as it starts loading (`None`: nothing);
    /// not in Axion's own pages. It can talk back with console messages starting with
    /// [`PAGE_MESSAGE_PREFIX`].
    fn set_page_script(&mut self, script: Option<String>);
    /// Installs what serves Axion's own `axion://` pages (see [`internal`]).
    fn set_internal_pages(&mut self, pages: Option<std::sync::Arc<dyn InternalPages>>);
    /// Runs JavaScript in the view's top frame; with `user_gesture` as if the user had just
    /// clicked (needed for fullscreen and unmuted playback).
    fn run_script(&mut self, view: ViewId, script: &str, user_gesture: bool);
    /// Light or dark for pages (`prefers-color-scheme`, form controls, scrollbars), applied to
    /// open pages at once.
    fn set_color_scheme(&mut self, dark: bool);
    /// Where downloads are saved, and whether to ask each time.
    fn set_download_options(&mut self, options: DownloadOptions);
    fn download_action(&mut self, id: u32, action: DownloadAction);
    /// Clears cached files and / or cookies and site storage (normal profile).
    fn clear_data(&mut self, what: &ClearData);
    /// Lists the sites with cookies; answers with [`EngineEvent::SiteData`].
    fn request_site_data(&mut self);
    /// Mutes or unmutes everything the view plays.
    fn set_audio_muted(&mut self, view: ViewId, muted: bool);
    /// Pauses a hidden view completely (no JavaScript, timers or loading) or resumes it.
    fn set_frozen(&mut self, view: ViewId, frozen: bool);
    /// Closes the view's page to free its memory, keeping its URL and last frame (still
    /// returned by [`Engine::frame`]). [`Engine::restore`] loads it again.
    fn discard(&mut self, view: ViewId);
    fn restore(&mut self, view: ViewId) -> Result<(), EngineError>;
    fn is_discarded(&self, view: ViewId) -> bool;
    /// Creates a view that starts discarded (e.g. a restored session's background tab): it
    /// costs nothing until [`Engine::restore`].
    fn create_discarded_view(&mut self, url: &str, size: ViewSize, private: bool) -> ViewId;
    fn set_focus(&mut self, view: ViewId, focused: bool);
    /// Mouse and keyboard input. While the page drags something of its own (text, a link, an
    /// image) the engine runs the drag itself: mouse moves, leaving and the release drive it.
    fn send_input(&mut self, view: ViewId, event: &InputEvent);

    /// Finds text in the page, highlighting every match and scrolling to the selected one.
    /// Results come as [`EngineEvent::FindResult`]s. A new `text` starts a new search; the
    /// same text with `next` moves to the following (or previous) match.
    fn find(&mut self, view: ViewId, request: &FindRequest);
    /// Ends find in page and removes the highlights; the selected match stays selected.
    fn stop_finding(&mut self, view: ViewId);

    /// Something from outside (files from the file manager) is dragged onto the view at `pos`
    /// (view coordinates, logical px). Follow with [`Engine::drag_over`] while it moves, and
    /// [`Engine::drag_leave`] or [`Engine::drag_drop`].
    fn drag_enter(&mut self, view: ViewId, data: &DropData, pos: Vec2, modifiers: Modifiers);
    fn drag_over(&mut self, view: ViewId, pos: Vec2, modifiers: Modifiers);
    fn drag_leave(&mut self, view: ViewId);
    fn drag_drop(&mut self, view: ViewId, pos: Vec2, modifiers: Modifiers);

    /// Whether pages reach the desktop clipboard by themselves (Ctrl+V and Ctrl+C work
    /// inside the engine). When they don't, the browser reads the clipboard and hands it over
    /// with [`Engine::paste`].
    fn has_system_clipboard(&self) -> bool;
    /// Pastes into the view's focused element as if from the clipboard: the page gets a
    /// `paste` event with the data (text, HTML, image files); if it doesn't handle it, the
    /// text (or an image in rich text) is inserted.
    fn paste(&mut self, view: ViewId, data: &PasteData);

    /// Downloads and decodes a favicon through the view's network stack (cache, cookies,
    /// blocking rules apply). Arrives as [`EngineEvent::FaviconReady`].
    fn fetch_favicon(&mut self, view: ViewId, url: &str, max_size: u32);

    /// Installs what decides which requests and navigations pages may make (Shields). It
    /// applies to every view, also ones created before.
    fn set_request_policy(&mut self, policy: Option<std::sync::Arc<dyn RequestPolicy>>);

    /// Asks the view to produce a frame now (call once per compositor frame for visible views).
    /// Engines that render on their own timer (see [`Engine::set_frame_rate`]) ignore it.
    fn request_frame(&mut self, view: ViewId);
    /// The most frames per second views render (the display's refresh rate, usually); applies
    /// to existing views too.
    fn set_frame_rate(&mut self, fps: u32);
    fn frame(&self, view: ViewId) -> Option<Frame>;

    /// Runs the engine's pending work and returns what happened since the last call.
    fn pump(&mut self) -> Vec<EngineEvent>;

    /// When the engine wants [`Engine::pump`] called next, so the embedder's event loop can
    /// sleep until then (`None`: it must be pumped continuously). An engine that gets work
    /// sooner wakes the event loop itself (e.g. through a waker it was created with).
    fn next_pump(&self) -> Option<std::time::Instant> {
        None
    }
}
