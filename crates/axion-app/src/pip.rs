//! Picture in Picture: the video in its own small window that stays above all other windows.
//! It is a plain window of the window manager's (its corners and shadow, if it draws any for
//! a window without a title bar), just without the title bar: the video fills it, with no
//! black bars, shape, margin or transparency of ours. The window takes the video's aspect and
//! snaps back to it after the user resizes it (`media.pip_keep_aspect`); while the aspects
//! differ the video covers the window (a sliver cropped) rather than being letterboxed.
//! Dragging the video moves the window, its edges resize it. Glass controls (close, back to
//! the tab, ±10 s, play/pause, progress) show while the pointer is over it (Safari, iPadOS 26).
//!
//! The picture is the video's tab's frame: cropped to the video while that tab is shown in the
//! browser (to the picture itself, not its element's black bars), and — while another tab is
//! shown — the whole frame of the hidden tab, in which the bridge makes the video cover the
//! page ("takeover").
//!
//! The OS window only exists while PiP is on: it is destroyed when PiP ends and made anew for
//! the next one. Wayland has no way to hide a window (winit's `set_visible(false)` does nothing
//! there), so a "hidden" window stayed on screen, frozen on its last frame.
//!
//! "Always on top": winit sets it on X11. Wayland gives apps no way to ask for it, so on KDE
//! Plasma a small KWin script keeps the window above (see `keep_above_on_kwin`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use axion_compositor::{Compositor, Icon, Rect as Frame, Scene};
use axion_config::Config;
use axion_engine::ViewId;
use axion_media::{Action, Command, Player};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{
    ActionId, Align, Background, CAPSULE, GlassLevel, Paint, TextStyle, Theme, Ui, UiEvent, UiInput, Widget,
};
use glam::Vec2;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId, WindowLevel};

const ACTION_BASE: u32 = 9700;
/// The window's width (logical px); the height follows the video.
const WIDTH: f32 = 480.0;
const MAX_HEIGHT: f32 = 480.0;
const MIN_WIDTH: f32 = 240.0;
/// How close to the window's edge (logical px) a drag resizes instead of moving it: the
/// window has no frame of its own to grab.
const RESIZE_BORDER: f32 = 8.0;
const BUTTON: f32 = 34.0;
const PLAY: f32 = 50.0;
const SKIP: f32 = 40.0;
pub const TITLE: &str = "Axion - Picture in Picture";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipControl {
    Close,
    BackToTab,
    SkipBack,
    PlayPause,
    SkipForward,
}

const CONTROLS: [PipControl; 5] = [
    PipControl::Close,
    PipControl::BackToTab,
    PipControl::SkipBack,
    PipControl::PlayPause,
    PipControl::SkipForward,
];

impl PipControl {
    fn id(self) -> ActionId {
        ActionId(ACTION_BASE + CONTROLS.iter().position(|&c| c == self).unwrap_or(0) as u32)
    }

    fn from_id(id: ActionId) -> Option<Self> {
        CONTROLS.get(id.0.checked_sub(ACTION_BASE)? as usize).copied()
    }
}

/// A source frame (texture and its size) and the part of it (0–1) showing the video.
pub type Picture = ((wgpu::TextureView, (u32, u32)), Frame);

/// What the browser should do after something happened in the PiP window.
pub enum PipOutcome {
    Command(ViewId, Command),
    /// Close PiP and pause the video (× or the window was closed) …
    Close(ViewId, String),
    /// … or end it and show the video's tab.
    BackToTab(ViewId),
}

fn circle(
    ui: &mut Ui,
    parent: NodeId,
    control: PipControl,
    icon: Icon,
    side: f32,
    text: Option<&str>,
) -> NodeId {
    let node = ui.add(
        parent,
        Widget::container()
            .with_action(control.id())
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thick))
            .pressed_whole()
            .without_hover_fill(),
        Style {
            size: Size { width: length(side), height: length(side) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let glyph_side = side * 0.5;
    let glyph = ui.add(node, Widget::icon(icon, glyph_side).with_paint(Paint::Label), Style::default());
    if let Some(text) = text {
        let label = ui.add_label(
            node,
            text,
            TextStyle::Caption,
            Style {
                size: Size { width: length(glyph_side), height: length(glyph_side) },
                margin: Rect {
                    left: length(-glyph_side),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        ui.set_label_align(label, Align::Center);
    }
    glyph
}

/// The OS window and its surface: only while PiP is on. Fields drop in order, so the surface
/// goes before its window.
struct Output {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    window: Arc<Window>,
}

impl Output {
    fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: LogicalSize<f32>,
    ) -> Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(TITLE)
            // No title bar: the window manager's own window, with only the video in it.
            .with_decorations(false)
            .with_resizable(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_min_inner_size(LogicalSize::new(MIN_WIDTH, MIN_WIDTH * 9.0 / 16.0))
            .with_inner_size(size);
        #[cfg(target_os = "linux")]
        let attributes = {
            use winit::platform::wayland::WindowAttributesExtWayland as _;
            attributes.with_name("axion", "axion-pip")
        };
        let window = Arc::new(event_loop.create_window(attributes).context("opening the PiP window")?);
        let surface = instance.create_surface(window.clone()).context("PiP window surface")?;
        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .context("the GPU cannot draw into the PiP window")?;
        // Opaque: the window is the video, no shape of ours.
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        // Never wait for vsync here: the browser window already does, and both render on one
        // thread.
        let present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::Fifo);
        let inner = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: inner.width.max(1),
            height: inner.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &config);
        // Set again once mapped: X11 window managers drop a level set on an unmapped window.
        window.set_window_level(WindowLevel::AlwaysOnTop);
        keep_above_on_kwin();
        Ok(Self { surface, config, window })
    }

    fn scale(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    /// The window's size in logical px.
    fn logical_size(&self) -> Vec2 {
        let size = self.window.inner_size();
        Vec2::new(size.width as f32, size.height as f32) / self.scale()
    }
}

pub struct PipWindow {
    /// While PiP is on (from the next turn of the event loop after it starts).
    output: Option<Output>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Made with the first window, for its surface's format.
    compositor: Option<(Compositor, wgpu::TextureFormat)>,
    ui: Ui,
    picture: NodeId,
    controls: NodeId,
    play_icon: NodeId,
    progress: NodeId,
    controls_opacity: Animated<f32>,
    hovered: bool,
    /// The pointer (logical px), for resizing at the edges.
    pointer: Option<Vec2>,
    /// The video's aspect (width / height) the window keeps.
    aspect: f32,
    /// The window's size (logical px) when the last PiP ended: the next one opens as wide.
    last_size: Option<Vec2>,
    /// When to bring the window back to the video's aspect (after a resize settled, or when
    /// the video's aspect changed).
    fit_at: Option<Instant>,
    /// The window's size when it last had the video's aspect: tells which side the user
    /// dragged.
    fitted: Option<PhysicalSize<u32>>,
    /// The size the window had when its aspect was last corrected: not asked again if the
    /// window manager keeps it there (tiling, size limits).
    asked_at: Option<PhysicalSize<u32>>,
    last_frame: Instant,
    /// The PiP'd video, while PiP is on.
    pub view: Option<ViewId>,
    pub player: Option<Player>,
    /// The bridge has been asked to let the video cover its (hidden) tab.
    pub takeover: bool,
}

impl PipWindow {
    /// PiP's controls, without a window yet: [`PipWindow::show`] starts PiP, and
    /// [`PipWindow::open`] (from the event loop) then opens the window.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, config: &Config) -> Self {
        let mut ui = Ui::new(Theme::new(config, true), 1.0);
        let root = ui.root();
        let picture = ui.add(
            root,
            Widget::image(),
            Style {
                position: Position::Absolute,
                size: Size { width: percent(1.0), height: percent(1.0) },
                ..Default::default()
            },
        );
        let controls = ui.add(
            root,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(axion_config::Color::rgb(0x000000)), 0.2)),
            Style {
                position: Position::Absolute,
                size: Size { width: percent(1.0), height: percent(1.0) },
                ..Default::default()
            },
        );
        let top = ui.add(
            controls,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(10.0), right: length(10.0), top: length(10.0), bottom: auto() },
                justify_content: JustifyContent::SPACE_BETWEEN,
                flex_direction: FlexDirection::Row,
                ..Default::default()
            },
        );
        circle(&mut ui, top, PipControl::Close, Icon::Close, BUTTON, None);
        circle(&mut ui, top, PipControl::BackToTab, Icon::PictureInPicture, BUTTON, None);
        let centre = ui.add(
            controls,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: percent(1.0) },
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(26.0), height: length(0.0) },
                ..Default::default()
            },
        );
        circle(&mut ui, centre, PipControl::SkipBack, Icon::SkipBack, SKIP, Some("10"));
        let play_icon = circle(&mut ui, centre, PipControl::PlayPause, Icon::Pause, PLAY, None);
        circle(&mut ui, centre, PipControl::SkipForward, Icon::SkipForward, SKIP, Some("10"));
        let progress = ui.add(
            controls,
            Widget::line_slider(0.0),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(16.0), right: length(16.0), top: auto(), bottom: length(10.0) },
                size: Size { width: auto(), height: length(10.0) },
                ..Default::default()
            },
        );
        Self {
            output: None,
            device: device.clone(),
            queue: queue.clone(),
            compositor: None,
            ui,
            picture,
            controls,
            play_icon,
            progress,
            controls_opacity: Animated::new(0.0, Spring::SNAPPY),
            hovered: false,
            pointer: None,
            aspect: 16.0 / 9.0,
            last_size: None,
            fit_at: None,
            fitted: None,
            asked_at: None,
            last_frame: Instant::now(),
            view: None,
            player: None,
            takeover: false,
        }
    }

    /// The window, while PiP is on and it is open.
    pub fn id(&self) -> Option<WindowId> {
        self.output.as_ref().map(|o| o.window.id())
    }

    pub fn is_active(&self) -> bool {
        self.view.is_some()
    }

    /// PiP is on but its window is not open yet.
    pub fn needs_window(&self) -> bool {
        self.is_active() && self.output.is_none()
    }

    /// Starts PiP for `player` of `view` (or switches it to that video). The window opens with
    /// the next [`PipWindow::open`].
    pub fn show(&mut self, view: ViewId, player: Player) {
        self.aspect = player.aspect().max(0.2);
        self.view = Some(view);
        self.player = Some(player);
        self.takeover = false;
        // Another video in the open window: to its aspect right away.
        self.fit_at = Some(Instant::now());
        self.asked_at = None;
    }

    /// Opens the window for the running PiP, sized to the video: as wide as the last one (or
    /// the default width) at the video's aspect.
    pub fn open(
        &mut self,
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
    ) -> Result<()> {
        if !self.needs_window() {
            return Ok(());
        }
        let size = match self.last_size {
            Some(last) => Vec2::new(last.x, last.x / self.aspect),
            None => {
                let height = (WIDTH / self.aspect).min(MAX_HEIGHT);
                Vec2::new(height * self.aspect, height)
            }
        };
        let output =
            Output::open(event_loop, instance, adapter, &self.device, LogicalSize::new(size.x, size.y))?;
        let format = output.config.format;
        if self.compositor.as_ref().is_none_or(|(_, f)| *f != format) {
            let compositor = Compositor::new(&self.device, &self.queue, format, &mut self.ui.fonts());
            self.compositor = Some((compositor, format));
        }
        self.ui.set_viewport(output.logical_size(), output.scale());
        self.fitted = Some(output.window.inner_size());
        self.fit_at = None;
        self.asked_at = None;
        self.last_frame = Instant::now();
        self.output = Some(output);
        Ok(())
    }

    /// Ends PiP: the window closes (it is opened anew for the next PiP).
    pub fn hide(&mut self) {
        self.view = None;
        self.player = None;
        self.takeover = false;
        self.hovered = false;
        self.pointer = None;
        self.ui.handle(UiInput::PointerLeave);
        self.controls_opacity = Animated::new(0.0, Spring::SNAPPY);
        if let Some(output) = self.output.take() {
            self.last_size = Some(output.logical_size());
            output.window.set_visible(false);
            // The surface first, then the window (field order).
            drop(output);
        }
        self.fit_at = None;
    }

    /// Resizes the open window (a capture aid standing in for the user's resize).
    pub fn request_size(&mut self, size: LogicalSize<f64>, config: &Config) {
        let Some(output) = &self.output else { return };
        if let Some(size) = output.window.request_inner_size(size) {
            self.window_event(WindowEvent::Resized(size), config);
        }
    }

    /// The window has a new size (physical px): the surface and the controls follow.
    fn resized(&mut self, size: PhysicalSize<u32>) {
        let Some(output) = &mut self.output else { return };
        if size.width == 0 || size.height == 0 {
            return;
        }
        output.config.width = size.width;
        output.config.height = size.height;
        output.surface.configure(&self.device, &output.config);
        let scale = output.scale();
        self.ui.set_viewport(Vec2::new(size.width as f32, size.height as f32) / scale, scale);
    }

    /// The last rendered frame (for `--capture`).
    pub fn read_back(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.compositor.as_ref()?.0.read_back()
    }

    pub fn set_theme(&mut self, config: &Config) {
        self.ui.set_theme(Theme::new(config, true));
    }

    /// Which edge or corner of the window `p` (logical px) grabs, if any.
    fn edge_at(&self, p: Vec2) -> Option<ResizeDirection> {
        let size = self.output.as_ref()?.logical_size();
        let (left, right) = (p.x < RESIZE_BORDER, p.x > size.x - RESIZE_BORDER);
        let (top, bottom) = (p.y < RESIZE_BORDER, p.y > size.y - RESIZE_BORDER);
        Some(match (left, right, top, bottom) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    /// Input for the PiP window. Dragging the edges resizes it, anywhere else but on a button
    /// moves it.
    pub fn window_event(&mut self, event: WindowEvent, config: &Config) -> Option<PipOutcome> {
        match event {
            WindowEvent::CloseRequested => {
                let (view, player) = (self.view?, self.player.clone()?);
                return Some(PipOutcome::Close(view, player.id));
            }
            WindowEvent::Resized(size) => {
                self.resized(size);
                // Back to the video's aspect once the user stops resizing.
                let settle = Duration::from_secs_f32(config.media.pip_aspect_settle_seconds.max(0.0));
                self.fit_at = Some(Instant::now() + settle);
            }
            WindowEvent::CursorEntered { .. } => self.hovered = true,
            WindowEvent::CursorLeft { .. } => {
                self.hovered = false;
                self.pointer = None;
                self.ui.handle(UiInput::PointerLeave);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.hovered = true;
                let output = self.output.as_ref()?;
                let p = Vec2::new(position.x as f32, position.y as f32) / output.scale();
                self.pointer = Some(p);
                self.ui.handle(UiInput::PointerMove(p));
                let cursor = match self.edge_at(p) {
                    Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => CursorIcon::NwseResize,
                    Some(ResizeDirection::NorthEast | ResizeDirection::SouthWest) => CursorIcon::NeswResize,
                    Some(ResizeDirection::West | ResizeDirection::East) => CursorIcon::EwResize,
                    Some(ResizeDirection::North | ResizeDirection::South) => CursorIcon::NsResize,
                    None => CursorIcon::Default,
                };
                if let Some(output) = &self.output {
                    output.window.set_cursor(cursor);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    let window = self.output.as_ref()?.window.clone();
                    if let Some(direction) = self.pointer.and_then(|p| self.edge_at(p)) {
                        // The window manager resizes it (there is no frame of its own to grab).
                        if let Err(e) = window.drag_resize_window(direction) {
                            tracing::debug!("PiP window resize: {e}");
                        }
                        return None;
                    }
                    let (consumed, _) = self.ui.handle(UiInput::PointerDown);
                    if (!consumed || self.ui.pressed_action().is_none())
                        && let Err(e) = window.drag_window()
                    {
                        // Not on a button: the window follows the pointer (the window manager
                        // moves it, so it can go anywhere on the screen).
                        tracing::debug!("PiP window move: {e}");
                    }
                } else {
                    let (_, events) = self.ui.handle(UiInput::PointerUp);
                    for event in events {
                        if let UiEvent::Clicked { action, .. } = event
                            && let Some(control) = PipControl::from_id(action)
                        {
                            return self.click(control, config);
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }

    pub fn click(&mut self, control: PipControl, config: &Config) -> Option<PipOutcome> {
        let (view, player) = (self.view?, self.player.clone()?);
        let skip = f64::from(config.media.skip_seconds);
        let action = match control {
            PipControl::Close => return Some(PipOutcome::Close(view, player.id)),
            PipControl::BackToTab => return Some(PipOutcome::BackToTab(view)),
            PipControl::SkipBack => Action::Skip(-skip),
            PipControl::SkipForward => Action::Skip(skip),
            PipControl::PlayPause => Action::Toggle,
        };
        Some(PipOutcome::Command(view, Command::new(&player.id, action)))
    }

    /// Draws the newest picture: `picture` is the source frame and the part of it showing the
    /// video (0–1).
    pub fn render(&mut self, picture: Option<Picture>, config: &Config) {
        if !self.is_active() || self.output.is_none() {
            return;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        if let Some(player) = &self.player {
            let aspect = player.aspect().max(0.2);
            // The video's size became known, or it changed (another rendition): the window
            // follows.
            if (aspect / self.aspect - 1.0).abs() > ASPECT_TOLERANCE {
                self.aspect = aspect;
                self.fit_at = Some(now);
                self.asked_at = None;
            }
        }
        if config.media.pip_keep_aspect && self.fit_at.is_some_and(|at| at <= now) {
            self.fit_at = None;
            self.fit_window();
        }
        let Some(output) = &self.output else { return };
        let target_size = Vec2::new(output.config.width as f32, output.config.height as f32);
        if let Some((image, uv)) = picture {
            let (w, h) = image.1;
            // The video covers the window: while the window's aspect differs (during a resize,
            // or where the window manager keeps another size) a sliver is cropped, never
            // black bars.
            let uv = cover(uv, Vec2::new(w as f32, h as f32), target_size.x / target_size.y.max(1.0));
            self.ui.set_image(self.picture, Some(image));
            self.ui.set_image_crop(self.picture, Some(uv));
        }
        if let Some(player) = &self.player {
            self.ui.set_icon(self.play_icon, if player.is_playing() { Icon::Pause } else { Icon::Play });
            if player.duration > 0.0 {
                self.ui.set_value(self.progress, (player.time / player.duration) as f32);
            }
        }
        let paused = self.player.as_ref().is_some_and(|p| !p.is_playing());
        self.controls_opacity.target = if self.hovered || paused { 1.0 } else { 0.0 };
        self.controls_opacity.step(dt);
        self.ui.set_opacity(self.controls, self.controls_opacity.value.clamp(0.0, 1.0));
        self.ui.set_visible(self.controls, self.controls_opacity.value > 0.01);
        self.ui.animate(dt);
        self.ui.layout();

        let Some(output) = &self.output else { return };
        let Some((compositor, _)) = &mut self.compositor else { return };
        let frame = match output.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                output.surface.configure(&self.device, &output.config);
                return;
            }
            _ => return,
        };
        let target = frame.texture.create_view(&Default::default());
        let mut layers = Vec::new();
        self.ui.paint(&mut layers);
        // Black only until the first picture arrives.
        let scene = Scene { clear: [0.0, 0.0, 0.0, 1.0], layers };
        let size = (output.config.width, output.config.height);
        compositor.render(&scene, &mut self.ui.fonts(), &target, size);
        drop(scene);
        self.queue.present(frame);
    }

    /// Resizes the window to the video's aspect, keeping the side the user dragged.
    fn fit_window(&mut self) {
        let Some(output) = &self.output else { return };
        let size = output.window.inner_size();
        if size.width == 0 || size.height == 0 || self.asked_at == Some(size) {
            return;
        }
        let current = Vec2::new(size.width as f32, size.height as f32);
        let reference = self.fitted.map_or(current, |f| Vec2::new(f.width as f32, f.height as f32));
        let wanted = aspect_size(current, reference, self.aspect).round();
        let wanted = PhysicalSize::new(wanted.x.max(1.0) as u32, wanted.y.max(1.0) as u32);
        self.fitted = Some(wanted);
        if (wanted.width.abs_diff(size.width)).max(wanted.height.abs_diff(size.height)) <= 1 {
            return;
        }
        // Not asked again if the window manager leaves it at this size.
        self.asked_at = Some(size);
        tracing::debug!(from = ?size, to = ?wanted, "PiP window to the video's aspect");
        if let Some(granted) = output.window.request_inner_size(wanted) {
            // Applied right away (Wayland: the window is as big as what it draws): no
            // `Resized` follows for it.
            self.fitted = Some(granted);
            self.resized(granted);
        }
    }
}

/// Aspects closer than this (relative) count as the same.
const ASPECT_TOLERANCE: f32 = 0.005;

/// The size of aspect `aspect` (width / height) for a window that is now `current` and last
/// had the aspect at `reference`: the side the user changed more is kept, the other follows.
fn aspect_size(current: Vec2, reference: Vec2, aspect: f32) -> Vec2 {
    if aspect <= 0.0 || current.x <= 0.0 || current.y <= 0.0 {
        return current;
    }
    let changed = |now: f32, before: f32| if before > 0.0 { (now / before - 1.0).abs() } else { 0.0 };
    if changed(current.y, reference.y) > changed(current.x, reference.x) {
        Vec2::new(current.y * aspect, current.y)
    } else {
        Vec2::new(current.x, current.x / aspect)
    }
}

/// The part of `uv` (a crop of a `tex`-sized frame) that covers a window of aspect `window`
/// (width / height), centred: the video fills the window, overflow cropped.
fn cover(uv: Frame, tex: Vec2, window: f32) -> Frame {
    let video = tex * uv.size;
    if video.x <= 0.0 || video.y <= 0.0 || window <= 0.0 {
        return uv;
    }
    let aspect = video.x / video.y;
    let mut size = uv.size;
    if aspect > window {
        size.x *= window / aspect;
    } else {
        size.y *= aspect / window;
    }
    Frame { origin: uv.origin + (uv.size - size) / 2.0, size }
}

/// On KDE Plasma (Wayland), asks KWin to keep the PiP window above all others: Wayland has no
/// way for an app to ask for that itself. Runs once per session; harmless elsewhere.
fn keep_above_on_kwin() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    let on_kde_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("KDE"));
    if !on_kde_wayland {
        return;
    }
    ONCE.call_once(|| {
        std::thread::spawn(|| {
            if let Err(e) = load_kwin_script() {
                tracing::warn!("could not keep the PiP window on top (KWin): {e}");
            }
        });
    });
}

fn load_kwin_script() -> Result<()> {
    // KWin 6 (`windowList`, `windowAdded`) with a fallback for KWin 5 (`clientList`).
    let script = format!(
        r#"function keep(w) {{ if (w && w.caption === "{TITLE}") {{ w.keepAbove = true; w.onAllDesktops = true; w.skipTaskbar = true; }} }}
var list = workspace.windowList ? workspace.windowList() : workspace.clientList();
for (var i = 0; i < list.length; i++) keep(list[i]);
(workspace.windowAdded || workspace.clientAdded).connect(keep);
"#
    );
    let path = std::env::temp_dir().join(format!("axion-pip-{}.js", std::process::id()));
    std::fs::write(&path, script)?;
    let connection = zbus::blocking::Connection::session()?;
    let reply = connection.call_method(
        Some("org.kde.KWin"),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        "loadScript",
        &(path.to_string_lossy().as_ref(), format!("axion-pip-{}", std::process::id())),
    )?;
    let id: i32 = reply.body().deserialize()?;
    let object = if id >= 0 { format!("/Scripting/Script{id}") } else { "/Scripting".into() };
    let interface = if id >= 0 { "org.kde.kwin.Script" } else { "org.kde.kwin.Scripting" };
    let method = if id >= 0 { "run" } else { "start" };
    connection.call_method(Some("org.kde.KWin"), object.as_str(), Some(interface), method, &())?;
    tracing::info!("KWin keeps the PiP window on top");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_video_covers_the_window() {
        let whole = Frame { origin: Vec2::ZERO, size: Vec2::ONE };
        // Same aspect: all of it.
        assert_eq!(cover(whole, Vec2::new(1600.0, 900.0), 16.0 / 9.0), whole);
        // A wider window: a sliver off the top and bottom.
        let c = cover(whole, Vec2::new(1600.0, 900.0), 2.0);
        assert!((c.size.y - 0.888_889).abs() < 1e-4 && c.size.x == 1.0);
        assert!((c.origin.y - (1.0 - c.size.y) / 2.0).abs() < 1e-6);
        // A taller window: off the sides, inside the crop it started from.
        let part = Frame { origin: Vec2::new(0.25, 0.25), size: Vec2::splat(0.5) };
        let c = cover(part, Vec2::new(1000.0, 1000.0), 0.5);
        assert_eq!(c, Frame { origin: Vec2::new(0.375, 0.25), size: Vec2::new(0.25, 0.5) });
    }

    #[test]
    fn resizing_keeps_the_dragged_side() {
        let start = Vec2::new(480.0, 270.0);
        // Wider: the width stays, the height follows.
        assert_eq!(aspect_size(Vec2::new(640.0, 280.0), start, 16.0 / 9.0), Vec2::new(640.0, 360.0));
        // Taller: the height stays.
        assert_eq!(aspect_size(Vec2::new(490.0, 360.0), start, 16.0 / 9.0), Vec2::new(640.0, 360.0));
        // A new video's aspect, same size: the width stays.
        assert_eq!(aspect_size(start, start, 4.0 / 3.0), Vec2::new(480.0, 360.0));
    }

    #[test]
    fn control_ids_round_trip() {
        for c in CONTROLS {
            assert_eq!(PipControl::from_id(c.id()), Some(c));
        }
    }
}
