//! Picture in Picture: the video in its own small window that stays above all other windows.
//! It is a plain window of the window manager's (its corners and shadow, if it draws any for
//! a window without a title bar), just without the title bar: the video fills it, with no
//! black bars, shape, margin or transparency of ours. The window takes the video's aspect and
//! is locked to it (`media.pip_lock_aspect`): on X11 the window manager is told (WM_NORMAL_HINTS aspect),
//! everywhere the window is snapped to the aspect while it is resized (dominant axis, rate-limited,
//! never below the minimum size). While the aspects differ in between the video covers the window
//! (a sliver cropped) rather than being letterboxed.
//! Dragging the video moves the window, its thin edge band resizes it (never the controls: they keep clear of it). Glass controls (close, back to
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

use crate::player::volume_actions;
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
/// After scrubbing, the player's reports still carry the old time for a moment; ignore them.
const SCRUB_SETTLE: Duration = Duration::from_millis(600);

/// Base sizes of the controls (logical px) at `media.pip_control_scale` 1; none gets smaller
/// than `media.pip_min_target`.
const TOP_BUTTON: f32 = 36.0;
const SKIP: f32 = 40.0;
const PLAY: f32 = 52.0;
const BAR: f32 = 36.0;
const CENTRE_GAP: f32 = 24.0;
/// The glyph's share of its button.
const ICON_RATIO: f32 = 0.5;
/// The least space between two controls, and the space the controls keep to the resize band.
const GAP_MIN: f32 = 12.0;
/// The opened volume capsule (speaker + slider) is this many times as wide as it is tall.
const VOLUME_OPEN_RATIO: f32 = 3.6;
const EDGE_CLEARANCE: f32 = 4.0;
pub const TITLE: &str = "Axion - Picture in Picture";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipControl {
    Close,
    BackToTab,
    SkipBack,
    PlayPause,
    SkipForward,
    /// The speaker of the volume control: mutes and unmutes.
    Mute,
}

const CONTROLS: [PipControl; 6] = [
    PipControl::Close,
    PipControl::BackToTab,
    PipControl::SkipBack,
    PipControl::PlayPause,
    PipControl::SkipForward,
    PipControl::Mute,
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
    /// Several commands in a row (a volume change that also unmutes).
    Commands(ViewId, Vec<Command>),
    /// Close PiP and pause the video (× or the window was closed) …
    Close(ViewId, String),
    /// … or end it and show the video's tab.
    BackToTab(ViewId),
}

/// A control: a glass circle with its glyph (and the seconds inside the skip buttons). Placed
/// and sized by [`layout`] afterwards.
struct Button {
    node: NodeId,
    glyph: NodeId,
    label: Option<NodeId>,
}

fn circle(ui: &mut Ui, parent: NodeId, control: PipControl, icon: Icon, text: Option<&str>) -> Button {
    let side = TOP_BUTTON;
    let node = ui.add(
        parent,
        Widget::container()
            .with_action(control.id())
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thick))
            .pressed_whole()
            .without_hover_fill(),
        Style {
            position: Position::Absolute,
            size: Size { width: length(side), height: length(side) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let glyph_side = side * ICON_RATIO;
    let glyph = ui.add(node, Widget::icon(icon, glyph_side).with_paint(Paint::Label), Style::default());
    let mut label_node = None;
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
        label_node = Some(label);
    }
    Button { node, glyph, label: label_node }
}

/// Where the controls go in a window of `size` (logical px): all in the same order as
/// [`CONTROLS`], plus the progress bar.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    buttons: [Frame; 5],
    bar: Frame,
    /// The volume's speaker button, and the capsule it opens into (to the right of it, in the
    /// top row between the close and the back button).
    volume: Frame,
    volume_open: Frame,
    /// The control scale actually used (the configured one, shrunk to fit a small window).
    scale: f32,
}

/// The sizes the controls are laid out with.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sizing {
    scale: f32,
    /// Width of the edge band that resizes the window.
    border: f32,
    /// No control is smaller than this.
    min_target: f32,
}

impl Sizing {
    fn new(config: &Config) -> Self {
        let m = &config.media;
        Self {
            scale: m.pip_control_scale.max(0.1),
            border: m.pip_resize_border.max(0.0),
            min_target: m.pip_min_target.max(1.0),
        }
    }

    /// How far the controls keep from the window's edges.
    fn inset(&self) -> f32 {
        self.border + EDGE_CLEARANCE
    }

    /// The shortest window the controls fit in, without overlapping each other.
    fn min_height(&self) -> f32 {
        2.0 * self.inset() + 3.0 * self.min_target + 2.0 * GAP_MIN
    }

    fn size(&self, base: f32, scale: f32) -> f32 {
        (base * scale).max(self.min_target)
    }
}

/// Close and "back to the tab" sit in the top corners; the progress bar is a glass bar along
/// the bottom, as tall as a button and lifted clear of the resize band; skip, play and skip sit
/// between them, centred in the window as far as the space allows. The control
/// scale shrinks (to the minimum target) when the window is too small for the configured one.
fn layout(window: Vec2, sizing: Sizing) -> Layout {
    let inset = sizing.inset();
    let fits = |s: f32| {
        let vertical = 2.0 * inset
            + sizing.size(TOP_BUTTON, s)
            + sizing.size(BAR, s)
            + sizing.min_target
            + 2.0 * GAP_MIN;
        let row = 2.0 * sizing.size(SKIP, s) + sizing.size(PLAY, s) + 2.0 * GAP_MIN;
        vertical <= window.y && row <= window.x - 2.0 * inset
    };
    let mut scale = sizing.scale;
    while scale > 0.1 && !fits(scale) {
        scale -= 0.05;
    }
    let top = sizing.size(TOP_BUTTON, scale);
    let bar_h = sizing.size(BAR, scale);
    let band_top = inset + top + GAP_MIN;
    let band_bottom = window.y - inset - bar_h - GAP_MIN;
    let band = (band_bottom - band_top).max(sizing.min_target);
    let skip = sizing.size(SKIP, scale).min(band);
    let play = sizing.size(PLAY, scale).min(band);
    let gap = ((window.x - 2.0 * inset - 2.0 * skip - play) / 2.0).min(CENTRE_GAP * scale).max(GAP_MIN);
    let row = 2.0 * skip + play + 2.0 * gap;
    let x0 = (window.x - row) / 2.0;
    let play_y = (window.y / 2.0 - play / 2.0).clamp(band_top, (band_bottom - play).max(band_top));
    let centre_y = play_y + play / 2.0;
    let square = |x: f32, y: f32, side: f32| Frame { origin: Vec2::new(x, y), size: Vec2::splat(side) };
    // The speaker sits right of the close button; opened, its capsule grows rightwards but
    // stays clear of the back button.
    let volume_x = inset + top + GAP_MIN;
    let free = (window.x - inset - top - GAP_MIN - volume_x).max(top);
    let volume_w = (top * VOLUME_OPEN_RATIO).min(free).max(top);
    Layout {
        volume: square(volume_x, inset, top),
        volume_open: Frame { origin: Vec2::new(volume_x, inset), size: Vec2::new(volume_w, top) },
        buttons: [
            square(inset, inset, top),
            square(window.x - inset - top, inset, top),
            square(x0, centre_y - skip / 2.0, skip),
            square(x0 + skip + gap, play_y, play),
            square(x0 + skip + gap + play + gap, centre_y - skip / 2.0, skip),
        ],
        bar: Frame {
            origin: Vec2::new(inset, window.y - inset - bar_h),
            size: Vec2::new((window.x - 2.0 * inset).max(1.0), bar_h),
        },
        scale,
    }
}

impl Layout {
    fn rects(&self) -> impl Iterator<Item = Frame> + '_ {
        self.buttons.iter().copied().chain([self.bar, self.volume_open])
    }
}

/// What a press at `p` in a window of `size` starts. Controls win over the resize band (they
/// keep clear of it anyway); the band only resizes along the window's edges, and anything
/// else moves the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Control,
    Resize(ResizeDirection),
    Move,
}

fn press_at(layout: &Layout, size: Vec2, border: f32, p: Vec2) -> Press {
    if layout.rects().any(|r| r.contains(p)) {
        return Press::Control;
    }
    match edge_at(size, border, p) {
        Some(direction) => Press::Resize(direction),
        None => Press::Move,
    }
}

/// Which edge or corner of a window of `size` the point `p` (logical px) grabs, if any.
fn edge_at(size: Vec2, border: f32, p: Vec2) -> Option<ResizeDirection> {
    let (left, right) = (p.x < border, p.x > size.x - border);
    let (top, bottom) = (p.y < border, p.y > size.y - border);
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
        min_size: LogicalSize<f32>,
    ) -> Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(TITLE)
            .with_window_icon(crate::app_icon::window_icon())
            // No title bar: the window manager's own window, with only the video in it.
            .with_decorations(false)
            .with_resizable(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_min_inner_size(min_size)
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
    buttons: [Button; 5],
    play_icon: NodeId,
    bar: NodeId,
    progress: NodeId,
    /// The volume: a capsule with the speaker (mute) and a slider that opens when the pointer
    /// is over it and closes when it leaves.
    volume_capsule: NodeId,
    volume_button: NodeId,
    volume_icon: NodeId,
    volume_slider: NodeId,
    /// 0 closed, 1 open.
    volume_open: Animated<f32>,
    /// The volume slider was moved a moment ago.
    volume_set_at: Option<Instant>,
    /// The window size and control sizing the controls were last laid out for.
    laid_out: Option<(Vec2, Sizing)>,
    sizing: Sizing,
    /// The narrowest window (logical px).
    min_width: f32,
    /// `media.pip_lock_aspect`.
    lock_aspect: bool,
    /// The controls' places, for hit-testing (valid once laid out).
    layout: Option<Layout>,
    /// The progress bar is being scrubbed, or was a moment ago.
    scrubbed_at: Option<Instant>,
    /// Presses and window-manager calls are simulated (capture aids): no real grab.
    pub synthetic: bool,
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
    /// When the window was last brought to the video's aspect (rate limit while resizing).
    last_fit: Option<Instant>,
    /// The aspect last told to the window manager (X11 size hint) and the lock state.
    hinted: Option<Option<f32>>,
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
        let make = |ui: &mut Ui, control, icon, text| circle(ui, controls, control, icon, text);
        let buttons = [
            make(&mut ui, PipControl::Close, Icon::Close, None),
            make(&mut ui, PipControl::BackToTab, Icon::PictureInPicture, None),
            make(&mut ui, PipControl::SkipBack, Icon::SkipBack, Some("10")),
            make(&mut ui, PipControl::PlayPause, Icon::Pause, None),
            make(&mut ui, PipControl::SkipForward, Icon::SkipForward, Some("10")),
        ];
        let play_icon = buttons[3].glyph;
        // The progress bar: a glass capsule as tall as a button around the slider, so the
        // whole capsule is the thing to grab (not a thin line).
        let bar = ui.add(
            controls,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_button_tint()
                .with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Row,
                padding: Rect {
                    left: length(16.0),
                    right: length(16.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        let progress = ui.add(
            bar,
            Widget::line_slider(0.0).with_track(config.media.progress_thickness),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(20.0), height: auto() },
                ..Default::default()
            },
        );
        // The volume: the speaker button, and the slider that opens beside it.
        let volume_capsule = ui.add(
            controls,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_button_tint()
                .with_radius(CAPSULE)
                .with_clip(),
            Style { position: Position::Absolute, ..Default::default() },
        );
        let volume_button = ui.add(
            volume_capsule,
            Widget::container().with_action(PipControl::Mute.id()).with_radius(CAPSULE).without_hover_fill(),
            Style {
                position: Position::Absolute,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let volume_icon = ui.add(
            volume_button,
            Widget::icon(Icon::Volume, TOP_BUTTON * ICON_RATIO).with_paint(Paint::Label),
            Style::default(),
        );
        let volume_slider = ui.add(
            volume_capsule,
            Widget::line_slider(1.0).with_track(config.media.progress_thickness),
            Style { position: Position::Absolute, ..Default::default() },
        );
        Self {
            output: None,
            device: device.clone(),
            queue: queue.clone(),
            compositor: None,
            ui,
            picture,
            controls,
            buttons,
            play_icon,
            bar,
            progress,
            volume_capsule,
            volume_button,
            volume_icon,
            volume_slider,
            volume_open: Animated::new(0.0, Spring::SNAPPY),
            volume_set_at: None,
            laid_out: None,
            sizing: Sizing::new(config),
            min_width: config.media.pip_min_width,
            lock_aspect: config.media.pip_lock_aspect,
            layout: None,
            scrubbed_at: None,
            synthetic: false,
            controls_opacity: Animated::new(0.0, Spring::SNAPPY),
            hovered: false,
            pointer: None,
            aspect: 16.0 / 9.0,
            last_size: None,
            fit_at: None,
            fitted: None,
            last_fit: None,
            hinted: None,
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
        let min = self.min_size();
        let size = match self.last_size {
            Some(last) => Vec2::new(last.x, last.x / self.aspect),
            None => {
                let height = (WIDTH / self.aspect).min(MAX_HEIGHT);
                Vec2::new(height * self.aspect, height)
            }
        };
        // Tall videos: not taller than the default's maximum, but never below the minimum.
        let size = if size.y > MAX_HEIGHT { size * (MAX_HEIGHT / size.y) } else { size };
        let size = if self.lock_aspect { fit_minimum(size, self.aspect, min) } else { size };
        let output = Output::open(
            event_loop,
            instance,
            adapter,
            &self.device,
            LogicalSize::new(size.x, size.y),
            LogicalSize::new(min.x, min.y),
        )?;
        let format = output.config.format;
        if self.compositor.as_ref().is_none_or(|(_, f)| *f != format) {
            let compositor = Compositor::new(&self.device, &self.queue, format, &mut self.ui.fonts());
            self.compositor = Some((compositor, format));
        }
        self.ui.set_viewport(output.logical_size(), output.scale());
        self.fitted = Some(output.window.inner_size());
        self.fit_at = None;
        self.asked_at = None;
        self.last_fit = None;
        self.hinted = None;
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
        self.ui.end_interaction();
        self.ui.handle(UiInput::PointerLeave);
        self.controls_opacity = Animated::new(0.0, Spring::SNAPPY);
        self.volume_open = Animated::new(0.0, Spring::SNAPPY);
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
        self.sizing = Sizing::new(config);
        self.min_width = config.media.pip_min_width;
        self.lock_aspect = config.media.pip_lock_aspect;
        self.laid_out = None;
    }

    /// Places and sizes the controls for the window's size (when it or the sizing changed).
    fn apply_layout(&mut self) {
        let Some(output) = &self.output else { return };
        let size = output.logical_size();
        if self.laid_out == Some((size, self.sizing)) {
            return;
        }
        self.laid_out = Some((size, self.sizing));
        let l = layout(size, self.sizing);
        self.layout = Some(l);
        let place = |ui: &mut Ui, node: NodeId, r: Frame| {
            let mut style = ui.style(node);
            style.position = Position::Absolute;
            style.inset =
                Rect { left: length(r.origin.x), top: length(r.origin.y), right: auto(), bottom: auto() };
            style.size = Size { width: length(r.size.x), height: length(r.size.y) };
            ui.set_style(node, style);
        };
        for (button, rect) in self.buttons.iter().zip(l.buttons) {
            place(&mut self.ui, button.node, rect);
            let glyph = rect.size.x * ICON_RATIO;
            self.ui.set_icon_size(button.glyph, glyph);
            if let Some(label) = button.label {
                let mut style = self.ui.style(label);
                style.size = Size { width: length(glyph), height: length(glyph) };
                style.margin.left = length(-glyph);
                self.ui.set_style(label, style);
            }
        }
        place(&mut self.ui, self.bar, l.bar);
    }

    /// Places the volume capsule: the speaker button, and the slider the capsule opens to
    /// (every frame while it moves).
    fn place_volume(&mut self) {
        let Some(l) = self.layout else { return };
        let open = self.volume_open.value.clamp(0.0, 1.0);
        let side = l.volume.size.x;
        let width = side + (l.volume_open.size.x - side) * open;
        let place = |ui: &mut Ui, node: NodeId, x: f32, y: f32, w: f32, h: f32| {
            let mut style = ui.style(node);
            style.position = Position::Absolute;
            style.inset = Rect { left: length(x), top: length(y), right: auto(), bottom: auto() };
            style.size = Size { width: length(w), height: length(h) };
            if style != ui.style(node) {
                ui.set_style(node, style);
            }
        };
        place(&mut self.ui, self.volume_capsule, l.volume.origin.x, l.volume.origin.y, width, side);
        place(&mut self.ui, self.volume_button, 0.0, 0.0, side, side);
        // The slider keeps its size while the capsule opens over it (the capsule clips it).
        let slider = (l.volume_open.size.x - side - side * 0.4).max(1.0);
        place(&mut self.ui, self.volume_slider, side, 0.0, slider, side);
        self.ui.set_icon_size(self.volume_icon, side * ICON_RATIO);
    }

    /// What a press at `p` (logical px) starts.
    fn press_at(&self, p: Vec2) -> Option<Press> {
        let size = self.output.as_ref()?.logical_size();
        let layout = self.layout?;
        Some(press_at(&layout, size, self.sizing.border, p))
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
                // Back to the video's aspect while the user resizes (rate-limited) and once
                // they stop.
                let now = Instant::now();
                let live = config.media.pip_aspect_live_seconds.max(0.0);
                self.fit_at = Some(if live > 0.0 {
                    let next = self.last_fit.map_or(now, |t| t + Duration::from_secs_f32(live));
                    next.max(now)
                } else {
                    now + Duration::from_secs_f32(config.media.pip_aspect_settle_seconds.max(0.0))
                });
            }
            WindowEvent::CursorEntered { .. } => self.hovered = true,
            // The button-up may go to another window (or the window manager's move/resize
            // grab): nothing stays held.
            WindowEvent::Focused(false) => {
                self.ui.end_interaction();
            }
            WindowEvent::CursorLeft { .. } => {
                // A slider being scrubbed keeps the pointer even outside the window.
                if self.ui.dragging().is_none() {
                    self.hovered = false;
                    self.pointer = None;
                    self.ui.handle(UiInput::PointerLeave);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.output.as_ref()?.scale();
                return self.pointer_moved(Vec2::new(position.x as f32, position.y as f32) / scale, config);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                return self.button(state == ElementState::Pressed, config);
            }
            _ => {}
        }
        None
    }

    /// The pointer moved to `p` (logical px): hover, the resize cursor, a scrub in progress.
    pub fn pointer_moved(&mut self, p: Vec2, config: &Config) -> Option<PipOutcome> {
        self.hovered = true;
        let output = self.output.as_ref()?;
        self.pointer = Some(p);
        let (_, events) = self.ui.handle(UiInput::PointerMove(p));
        let cursor = match self.press_at(p).filter(|_| self.ui.dragging().is_none()) {
            Some(Press::Resize(ResizeDirection::NorthWest | ResizeDirection::SouthEast)) => {
                CursorIcon::NwseResize
            }
            Some(Press::Resize(ResizeDirection::NorthEast | ResizeDirection::SouthWest)) => {
                CursorIcon::NeswResize
            }
            Some(Press::Resize(ResizeDirection::West | ResizeDirection::East)) => CursorIcon::EwResize,
            Some(Press::Resize(ResizeDirection::North | ResizeDirection::South)) => CursorIcon::NsResize,
            _ => CursorIcon::Default,
        };
        output.window.set_cursor(cursor);
        self.outcome_of(events, config)
    }

    /// The left button went down or up at the pointer.
    pub fn button(&mut self, pressed: bool, config: &Config) -> Option<PipOutcome> {
        if pressed {
            let window = self.output.as_ref()?.window.clone();
            let press = self.pointer.and_then(|p| self.press_at(p)).unwrap_or(Press::Move);
            if let Press::Resize(direction) = press {
                // The window manager resizes it (there is no frame of its own to grab).
                // Only in the narrow edge band: the controls never reach it.
                if !self.synthetic
                    && let Err(e) = window.drag_resize_window(direction)
                {
                    tracing::debug!("PiP window resize: {e}");
                }
                return None;
            }
            let (_, events) = self.ui.handle(UiInput::PointerDown);
            // A button or the progress bar took the press: it never moves the window.
            let on_control = self.ui.pressed_action().is_some() || self.ui.dragging().is_some();
            if !on_control
                && !self.synthetic
                && let Err(e) = window.drag_window()
            {
                // Not on a control: the window follows the pointer (the window manager
                // moves it, so it can go anywhere on the screen).
                tracing::debug!("PiP window move: {e}");
            }
            return self.outcome_of(events, config);
        }
        let (_, events) = self.ui.handle(UiInput::PointerUp);
        self.outcome_of(events, config)
    }

    /// What the browser should do after these UI events: a click on a control, or the
    /// progress bar seeking (live while it is dragged).
    fn outcome_of(&mut self, events: Vec<UiEvent>, config: &Config) -> Option<PipOutcome> {
        let mut seek = None;
        let mut volume = None;
        for event in events {
            match event {
                UiEvent::Clicked { action, .. } => {
                    if let Some(control) = PipControl::from_id(action) {
                        return self.click(control, config);
                    }
                }
                UiEvent::ValueChanged { node, value } if node == self.progress => seek = Some(value),
                UiEvent::ValueChanged { node, value } if node == self.volume_slider => volume = Some(value),
                _ => {}
            }
        }
        if let Some(value) = volume {
            let (view, player) = (self.view?, self.player.clone()?);
            self.volume_set_at = Some(Instant::now());
            let commands = volume_actions(&player, value).into_iter().map(|a| Command::new(&player.id, a));
            return Some(PipOutcome::Commands(view, commands.collect()));
        }
        let value = seek?;
        let (view, player) = (self.view?, self.player.clone()?);
        if player.duration <= 0.0 {
            return None;
        }
        self.scrubbed_at = Some(Instant::now());
        Some(PipOutcome::Command(
            view,
            Command::new(&player.id, Action::Seek(f64::from(value) * player.duration)),
        ))
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
            PipControl::Mute => Action::Mute(!player.muted),
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
        self.lock_aspect = config.media.pip_lock_aspect;
        self.update_hints();
        if self.lock_aspect && self.fit_at.is_some_and(|at| at <= now) {
            self.fit_at = None;
            self.last_fit = Some(now);
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
            // Not while the user scrubs (or a moment after): the reports still carry the old time.
            let scrubbing = self.ui.dragging() == Some(self.progress)
                || self.scrubbed_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
            if player.live {
                // A live stream has no end to measure against: the timeline is always full.
                self.ui.set_value(self.progress, 1.0);
            } else if !scrubbing && player.duration > 0.0 {
                self.ui.set_value(self.progress, (player.time / player.duration) as f32);
            }
        }
        if let Some(player) = &self.player {
            let silent = player.muted || player.volume <= 0.0;
            self.ui.set_icon(self.volume_icon, if silent { Icon::Muted } else { Icon::Volume });
            let adjusting = self.ui.dragging() == Some(self.volume_slider)
                || self.volume_set_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
            if !adjusting {
                self.ui.set_value(self.volume_slider, if player.muted { 0.0 } else { player.volume as f32 });
            }
        }
        // The volume opens while the pointer is on it (or drags its slider), and closes when it
        // leaves.
        let on_volume = self.pointer.zip(self.layout).is_some_and(|(p, l)| {
            let area = if self.volume_open.target > 0.5 { l.volume_open } else { l.volume };
            area.contains(p)
        });
        let dragging = self.ui.dragging() == Some(self.volume_slider);
        self.volume_open.target = if self.hovered && (on_volume || dragging) { 1.0 } else { 0.0 };
        if config.glass.reduce_motion {
            self.volume_open.snap(self.volume_open.target);
        } else {
            self.volume_open.step(dt);
        }
        let paused = self.player.as_ref().is_some_and(|p| !p.is_playing());
        self.controls_opacity.target = if self.hovered || paused { 1.0 } else { 0.0 };
        self.controls_opacity.step(dt);
        self.ui.set_opacity(self.controls, self.controls_opacity.value.clamp(0.0, 1.0));
        self.ui.set_visible(self.controls, self.controls_opacity.value > 0.01);
        self.apply_layout();
        self.place_volume();
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

    /// The smallest window (logical px) of the video's aspect: at least `media.pip_min_width`
    /// wide and as tall as the controls need.
    fn min_size(&self) -> Vec2 {
        if self.lock_aspect {
            min_window(self.aspect, self.min_width, self.sizing.min_height())
        } else {
            Vec2::new(self.min_width, self.sizing.min_height())
        }
    }

    /// Tells the window manager the aspect (X11 size hint) and the smallest size (for the
    /// video's aspect) when they changed.
    fn update_hints(&mut self) {
        let Some(output) = &self.output else { return };
        let hint = self.lock_aspect.then_some(self.aspect);
        let key = hint.map(|a| (a * 1000.0).round() / 1000.0);
        if self.hinted == Some(key) {
            return;
        }
        self.hinted = Some(key);
        let min = self.min_size();
        output.window.set_min_inner_size(Some(LogicalSize::new(min.x, min.y)));
        #[cfg(target_os = "linux")]
        set_x11_aspect(&output.window, key);
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
        let scale = output.scale();
        let wanted = snap_aspect(current, reference, self.aspect, self.min_size() * scale).round();
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

/// The smallest window of aspect `aspect` (width / height) that is at least `min_width` wide
/// and `min_height` tall: a very wide or tall video grows the other side rather than being
/// letterboxed.
fn min_window(aspect: f32, min_width: f32, min_height: f32) -> Vec2 {
    let aspect = aspect.max(0.01);
    let width = min_width.max(min_height * aspect);
    Vec2::new(width, width / aspect)
}

/// `size` grown (keeping `aspect`) until it is no smaller than `min`.
fn fit_minimum(size: Vec2, aspect: f32, min: Vec2) -> Vec2 {
    let floor = min_window(aspect, min.x, min.y);
    if size.x < floor.x || size.y < floor.y { floor.max(size) } else { size }
}

/// The size of aspect `aspect` (width / height) for a window that is now `current` and last
/// had the aspect at `reference`: the side the user changed more (the dominant axis of the
/// drag) is kept, the other follows. Never smaller than `min` (the minimum rules win: the
/// result grows with the aspect kept).
fn snap_aspect(current: Vec2, reference: Vec2, aspect: f32, min: Vec2) -> Vec2 {
    if aspect <= 0.0 || current.x <= 0.0 || current.y <= 0.0 {
        return current;
    }
    let changed = |now: f32, before: f32| if before > 0.0 { (now / before - 1.0).abs() } else { 0.0 };
    let snapped = if changed(current.y, reference.y) > changed(current.x, reference.x) {
        Vec2::new(current.y * aspect, current.y)
    } else {
        Vec2::new(current.x, current.x / aspect)
    };
    fit_minimum(snapped, aspect, min)
}

/// Sets (or clears) the aspect-ratio size hint of an X11 window: the window manager then only
/// resizes it with that aspect. Wayland has no such thing (xdg-toplevel cannot lock an aspect,
/// and KWin's scripting API has no aspect property), so there only the snapping works.
#[cfg(target_os = "linux")]
fn set_x11_aspect(window: &Window, aspect: Option<f32>) {
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use x11rb::connection::Connection as _;
    use x11rb::properties::{AspectRatio, WmSizeHints};
    let id = match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
        Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
        _ => return,
    };
    let result = (|| -> Result<()> {
        let (conn, _) = x11rb::connect(None)?;
        let mut hints = WmSizeHints::get_normal_hints(&conn, id)?.reply()?.unwrap_or_default();
        hints.aspect = aspect.map(|a| {
            let ratio = AspectRatio::new((a * 1000.0).round().max(1.0) as i32, 1000);
            (ratio, ratio)
        });
        hints.set_normal_hints(&conn, id)?;
        conn.flush()?;
        Ok(())
    })();
    match result {
        Ok(()) => tracing::debug!(?aspect, "PiP aspect size hint set"),
        Err(e) => tracing::warn!("could not set the PiP aspect hint: {e}"),
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
        let min = Vec2::ZERO;
        // Wider: the width stays, the height follows.
        assert_eq!(snap_aspect(Vec2::new(640.0, 280.0), start, 16.0 / 9.0, min), Vec2::new(640.0, 360.0));
        // Taller: the height stays.
        assert_eq!(snap_aspect(Vec2::new(490.0, 360.0), start, 16.0 / 9.0, min), Vec2::new(640.0, 360.0));
        // A new video's aspect, same size: the width stays.
        assert_eq!(snap_aspect(start, start, 4.0 / 3.0, min), Vec2::new(480.0, 360.0));
        // Already at the aspect: unchanged.
        assert_eq!(snap_aspect(start, start, 16.0 / 9.0, min), start);
    }

    #[test]
    fn snapping_never_goes_below_the_minimum() {
        let z = sizing();
        let m = Config::default().media;
        for aspect in [21.0 / 9.0, 16.0 / 9.0, 1.0, 9.0 / 16.0, 1.0 / 3.0, 4.0] {
            let min = min_window(aspect, m.pip_min_width, z.min_height());
            assert!(min.x >= m.pip_min_width - 0.01 && min.y >= z.min_height() - 0.01, "{aspect}: {min:?}");
            assert!((min.x / min.y - aspect).abs() < 1e-3, "{aspect}: {min:?}");
            // A drag far below the minimum from either side lands on the minimum, aspect kept.
            let start = min * 2.0;
            for current in [Vec2::new(10.0, start.y), Vec2::new(start.x, 10.0), Vec2::new(1.0, 1.0)] {
                let out = snap_aspect(current, start, aspect, min);
                assert!(out.x >= min.x - 0.01 && out.y >= min.y - 0.01, "{aspect}: {out:?}");
                assert!((out.x / out.y - aspect).abs() < 1e-3, "{aspect}: {out:?}");
            }
        }
        // 21:9 is limited by the width, 9:16 by the controls' height, never letterboxed.
        assert_eq!(min_window(21.0 / 9.0, 272.0, 100.0).x, 272.0);
        assert!(min_window(9.0 / 16.0, 272.0, 100.0).y > 272.0);
        assert!(min_window(4.0, 272.0, 100.0).y >= 100.0 - 0.01);
    }

    #[test]
    fn the_dominant_axis_decides_for_corner_drags() {
        let start = Vec2::new(480.0, 270.0);
        let a = 16.0 / 9.0;
        // Mostly wider (+25 % vs. +10 % height): the width wins.
        let out = snap_aspect(Vec2::new(600.0, 297.0), start, a, Vec2::ZERO);
        assert_eq!(out.x, 600.0);
        // Mostly taller: the height wins.
        let out = snap_aspect(Vec2::new(500.0, 400.0), start, a, Vec2::ZERO);
        assert_eq!(out.y, 400.0);
        // The snapped size, used as the next reference, stays put (no feedback loop).
        let snapped = snap_aspect(Vec2::new(600.0, 297.0), start, a, Vec2::ZERO);
        assert_eq!(snap_aspect(snapped, snapped, a, Vec2::ZERO), snapped);
    }

    #[test]
    fn the_layout_works_at_extreme_aspects() {
        let z = sizing();
        let m = Config::default().media;
        for aspect in [21.0 / 9.0, 2.4, 1.0, 9.0 / 16.0, 0.4] {
            let min = min_window(aspect, m.pip_min_width, z.min_height());
            for k in [1.0, 1.5, 2.5] {
                let size = min * k;
                let l = layout(size, z);
                let all: Vec<Frame> = l.rects().collect();
                for (i, r) in all.iter().enumerate() {
                    assert!(
                        r.origin.x >= z.border - 0.01 && r.origin.y >= z.border - 0.01,
                        "{size:?}: {r:?}"
                    );
                    assert!(
                        r.origin.x + r.size.x <= size.x - z.border + 0.01
                            && r.origin.y + r.size.y <= size.y - z.border + 0.01,
                        "{size:?}: {r:?} out of the window"
                    );
                    for b in &all[i + 1..] {
                        let apart = r.origin.x + r.size.x <= b.origin.x + 0.01
                            || b.origin.x + b.size.x <= r.origin.x + 0.01
                            || r.origin.y + r.size.y <= b.origin.y + 0.01
                            || b.origin.y + b.size.y <= r.origin.y + 0.01;
                        assert!(apart, "{size:?}: {r:?} overlaps {b:?}");
                    }
                }
            }
        }
    }

    fn sizing() -> Sizing {
        Sizing::new(&Config::default())
    }

    /// The smallest and the default window, and a few other shapes.
    const WINDOWS: [(f32, f32); 6] =
        [(272.0, 153.0), (272.0, 204.0), (320.0, 180.0), (480.0, 270.0), (480.0, 480.0), (640.0, 360.0)];

    #[test]
    fn controls_are_big_enough_and_clear_of_the_resize_band() {
        let z = sizing();
        for (w, h) in WINDOWS {
            let size = Vec2::new(w, h);
            let l = layout(size, z);
            for r in l.rects() {
                assert!(r.size.x >= z.min_target && r.size.y >= z.min_target, "{w}x{h}: {r:?} too small");
                assert!(r.origin.x >= z.border && r.origin.y >= z.border, "{w}x{h}: {r:?} in the band");
                assert!(
                    r.origin.x + r.size.x <= w - z.border && r.origin.y + r.size.y <= h - z.border,
                    "{w}x{h}: {r:?} in the band"
                );
            }
            // No two controls overlap.
            let all: Vec<Frame> = l.rects().collect();
            for (i, a) in all.iter().enumerate() {
                for b in &all[i + 1..] {
                    let apart = a.origin.x + a.size.x <= b.origin.x
                        || b.origin.x + b.size.x <= a.origin.x
                        || a.origin.y + a.size.y <= b.origin.y
                        || b.origin.y + b.size.y <= a.origin.y;
                    assert!(apart, "{w}x{h}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_minimum_window_fits_the_controls() {
        let z = sizing();
        let c = Config::default().media;
        assert!(c.pip_min_width * 9.0 / 16.0 >= z.min_height() - 1.0, "min height {}", z.min_height());
        // Default size: larger than the base sizes.
        assert!(layout(Vec2::new(480.0, 270.0), z).scale > 1.2);
    }

    #[test]
    fn controls_win_over_the_resize_band() {
        let z = sizing();
        let size = Vec2::new(480.0, 270.0);
        let l = layout(size, z);
        // The middle of the progress bar and of its bottom, just above the band: the bar.
        let bar = l.bar;
        let bottom = Vec2::new(bar.origin.x + bar.size.x / 2.0, bar.origin.y + bar.size.y - 1.0);
        assert_eq!(press_at(&l, size, z.border, bottom), Press::Control);
        assert!(bottom.y < size.y - z.border);
        // Every control's centre and corners (inside) are controls.
        for r in l.rects() {
            for p in
                [r.origin + r.size / 2.0, r.origin + Vec2::splat(1.0), r.origin + r.size - Vec2::splat(1.0)]
            {
                assert_eq!(press_at(&l, size, z.border, p), Press::Control);
            }
        }
        // The band still resizes, corners too; the open video moves the window.
        assert_eq!(
            press_at(&l, size, z.border, Vec2::new(240.0, size.y - 1.0)),
            Press::Resize(ResizeDirection::South)
        );
        assert_eq!(
            press_at(&l, size, z.border, Vec2::new(1.0, 1.0)),
            Press::Resize(ResizeDirection::NorthWest)
        );
        assert_eq!(
            press_at(&l, size, z.border, Vec2::new(size.x - 1.0, size.y - 1.0)),
            Press::Resize(ResizeDirection::SouthEast)
        );
        assert_eq!(press_at(&l, size, z.border, Vec2::new(240.0, 80.0)), Press::Move);
    }

    #[test]
    fn control_ids_round_trip() {
        for c in CONTROLS {
            assert_eq!(PipControl::from_id(c.id()), Some(c));
        }
    }
}
