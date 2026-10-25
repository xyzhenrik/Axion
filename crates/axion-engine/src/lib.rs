//! The web engine behind Axion's tabs.
//!
//! [`Engine`] is what the rest of the browser talks to: it creates off-screen *views* (one
//! per tab), forwards input, and hands back each view's latest frame as a wgpu texture for
//! the compositor. Everything else (title, URL, loading state, popups, cursor) comes back as
//! [`EngineEvent`]s from [`Engine::pump`]. The Chromium backend lives in [`cef`]; another
//! engine (e.g. Servo) can be added behind the same trait.

pub mod cef;
mod input;

pub use input::{CursorKind, InputEvent, Modifiers, MouseButton};

use glam::Vec2;

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
    fn create_view(&mut self, url: &str, size: ViewSize) -> Result<ViewId, EngineError>;
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
    fn set_focus(&mut self, view: ViewId, focused: bool);
    fn send_input(&mut self, view: ViewId, event: &InputEvent);

    /// Asks the view to produce a frame now (call once per compositor frame for visible views).
    fn request_frame(&mut self, view: ViewId);
    fn frame(&self, view: ViewId) -> Option<Frame>;

    /// Runs the engine's pending work and returns what happened since the last call.
    fn pump(&mut self) -> Vec<EngineEvent>;
}
