//! Picture in Picture: the video in its own small window that stays above all other windows.
//! It is a plain window of the window manager's (its corners and shadow, if it draws any for
//! a window without a title bar), just without the title bar: the video fills it, with no
//! black bars, shape, margin or transparency of ours. The window takes the video's aspect and
//! is locked to it (`media.pip_lock_aspect`).
//!
//! Resizing: the window manager (or compositor) runs the drag (`drag_resize_window`, any edge or
//! corner) and the window follows the pointer *continuously*: every new size it reports is
//! answered at once with the size of the video's aspect that the pointer asks for (an edge: that
//! side drives, the other follows; a corner: the side the pointer moved further, relative to the
//! size when the drag began), within `media.pip_min_width` .. `pip_max_width` / the monitor.
//! There is no delayed snap-back. On X11 the window manager is also told the aspect and the
//! size limits (WM_NORMAL_HINTS), so it already hands out right-shaped sizes; xdg-toplevel
//! (Wayland) has no aspect hint, so there the immediate answer, plus the min/max size, is
//! the constraint. While the aspects differ in between the video covers the window (a sliver
//! cropped) rather than being letterboxed.
//!
//! The size is remembered (`window.toml`, `window.restore_pip_size`): as an area, so the next
//! video, of whatever shape, appears as big. A video of another aspect in the open window
//! re-fits the window once, keeping its area, centred where it was and on screen, animated by
//! `motion.smooth` (Reduce Motion: at once).
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

use crate::drip;
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
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId, WindowLevel};

const ACTION_BASE: u32 = 9700;
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

/// A resize in progress: what the size is measured against. Begins when the user grabs an
/// edge (`direction` known) or, for a resize the window manager started itself (Alt + right
/// drag, tiling), at its first new size.
#[derive(Clone, Copy, Debug)]
struct ResizeDrag {
    direction: Option<ResizeDirection>,
    /// The window's size (physical px) when the drag began.
    start: Vec2,
    /// When a new size last arrived.
    last_size_at: Instant,
    /// At least one new size arrived (the window manager has the pointer).
    moved: bool,
    /// The user's doing: its end size is remembered. Not so for a window manager's answer to
    /// a re-fit (it clamped the size), which is only brought to the aspect.
    user: bool,
}

/// The window re-fitting itself to another video's aspect.
struct Refit {
    from: Vec2,
    to: Vec2,
    /// The window's centre (physical px, screen) where it can be placed (not on Wayland).
    centre: Option<Vec2>,
    /// 0 → 1.
    progress: Animated<f32>,
    /// The size limits and the aspect hint were updated for it (the first frame only does
    /// that, so the window manager knows them before it is asked for the new size).
    armed: bool,
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
    /// The loading ring over the middle of the picture (always shown while loading, also when
    /// the controls are hidden).
    spinner: crate::spinner::Spinner,
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
    /// The size (logical px) the user last made the window: the next one has the same area.
    remembered: Option<Vec2>,
    /// A size the user just settled on, for the browser to persist (`take_user_size`).
    user_size: Option<Vec2>,
    /// A resize in progress.
    drag: Option<ResizeDrag>,
    /// An aspect change being animated.
    refit: Option<Refit>,
    /// When the last re-fit ended.
    refit_ended: Option<Instant>,
    /// The size (physical px) the window is meant to have: what the last answer asked for.
    settled: Option<PhysicalSize<u32>>,
    /// What was last told to the window manager: aspect, size limits (as whole logical px),
    /// and whether a re-fit is under way.
    hinted: Option<(Option<f32>, [i32; 4], bool)>,
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
        let ring_overlay = ui.add(
            root,
            Widget::container(),
            Style {
                position: Position::Absolute,
                size: Size { width: percent(1.0), height: percent(1.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let spinner = crate::spinner::Spinner::build(&mut ui, ring_overlay, SPINNER_SIDE);
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
            spinner,
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
            remembered: None,
            user_size: None,
            drag: None,
            refit: None,
            refit_ended: None,
            settled: None,
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
        // An open window follows the new video's aspect with a re-fit (see `render`).
        if self.output.is_none() {
            self.aspect = player.aspect().max(0.2);
        }
        self.view = Some(view);
        self.player = Some(player);
        self.takeover = false;
    }

    /// The size the user last made the window (logical px), from `window.toml`.
    pub fn set_remembered(&mut self, size: Option<Vec2>) {
        self.remembered = size.filter(|s| s.x >= 1.0 && s.y >= 1.0);
    }

    /// A size the user settled on since the last call, to be remembered.
    pub fn take_user_size(&mut self) -> Option<Vec2> {
        self.user_size.take()
    }

    /// Opens the window for the running PiP, sized to the video: as big (in area) as the
    /// user last made it, else the default width.
    pub fn open(
        &mut self,
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        config: &Config,
    ) -> Result<()> {
        if !self.needs_window() {
            return Ok(());
        }
        self.lock_aspect = config.media.pip_lock_aspect;
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        let (min, max) = self.limits(config, monitor.as_ref());
        let size = match self.remembered {
            Some(r) => size_for_area(r.x * r.y, self.aspect),
            None => {
                // No size remembered: the video fits a square of the default width.
                let side = config.media.pip_default_width.max(1.0);
                let height = (side / self.aspect).min(side);
                Vec2::new(height * self.aspect, height)
            }
        };
        let size = if self.lock_aspect { clamp_size(size, self.aspect, min, max) } else { size.max(min) };
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
        self.settled = Some(output.window.inner_size());
        self.drag = None;
        self.refit = None;
        self.asked_at = None;
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
        self.end_drag();
        self.refit = None;
        if let Some(output) = self.output.take() {
            output.window.set_visible(false);
            // The surface first, then the window (field order).
            drop(output);
        }
    }

    /// Resizes the open window (a capture aid standing in for the user's resize).
    pub fn request_size(&mut self, size: LogicalSize<f64>, config: &Config) {
        let Some(output) = &self.output else { return };
        if let Some(size) = output.window.request_inner_size(size) {
            self.window_event(WindowEvent::Resized(size), config);
        }
    }

    /// Capture aid: the grab of an edge or corner (`direction` like "se"), as pressing it.
    pub fn grab_edge(&mut self, direction: &str) {
        self.begin_drag(match direction {
            "n" => Some(ResizeDirection::North),
            "s" => Some(ResizeDirection::South),
            "e" => Some(ResizeDirection::East),
            "w" => Some(ResizeDirection::West),
            "ne" => Some(ResizeDirection::NorthEast),
            "nw" => Some(ResizeDirection::NorthWest),
            "se" => Some(ResizeDirection::SouthEast),
            "sw" => Some(ResizeDirection::SouthWest),
            _ => None,
        });
    }

    /// Capture aid: the pointer of a grabbed edge is `dx`, `dy` (logical px) from where it
    /// was grabbed: the window manager's answer is the start size changed by that, on the sides
    /// the grabbed edge moves.
    pub fn pull_edge(&mut self, dx: f64, dy: f64, config: &Config) {
        let Some(drag) = self.drag else { return };
        let Some(output) = &self.output else { return };
        use ResizeDirection::*;
        let scale = f64::from(output.scale());
        let (sx, sy) = match drag.direction {
            Some(East) => (1.0, 0.0),
            Some(West) => (-1.0, 0.0),
            Some(South) => (0.0, 1.0),
            Some(North) => (0.0, -1.0),
            Some(SouthEast) => (1.0, 1.0),
            Some(SouthWest) => (-1.0, 1.0),
            Some(NorthEast) => (1.0, -1.0),
            Some(NorthWest) => (-1.0, -1.0),
            None => (0.0, 0.0),
        };
        let raw = Vec2::new(
            (f64::from(drag.start.x) + dx * sx * scale).max(1.0) as f32,
            (f64::from(drag.start.y) + dy * sy * scale).max(1.0) as f32,
        );
        tracing::info!(?raw, "PiP resize drag: the window manager asks for");
        self.request_size(LogicalSize::new(f64::from(raw.x) / scale, f64::from(raw.y) / scale), config);
    }

    /// Capture aid: lets go of the grabbed edge.
    pub fn release_edge(&mut self) {
        self.end_drag();
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
                self.size_arrived(size, config);
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
                // The pointer is ours again: the window manager's resize grab is over.
                if self.drag.is_some_and(|d| d.moved && d.direction.is_some()) {
                    self.end_drag();
                }
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
                self.begin_drag(Some(direction));
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
                // Once, cleanly, unless the user is dragging an edge right now.
                if self.lock_aspect && self.drag.is_none() {
                    self.start_refit(config);
                }
            }
        }
        self.lock_aspect = config.media.pip_lock_aspect;
        let idle = Duration::from_secs_f32(config.media.pip_resize_idle_seconds.max(0.0));
        // (A grabbed edge ends with the pointer coming back, not with a timer: the user may
        // hold still mid-drag.)
        if self.drag.is_some_and(|d| d.moved && d.direction.is_none() && d.last_size_at.elapsed() >= idle) {
            self.end_drag();
        }
        self.update_hints(config);
        self.step_refit(dt, config);
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
        let loading = self.player.as_ref().is_some_and(|p| p.loading && !p.paused);
        self.spinner.update(&mut self.ui, config, loading, dt);
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

    /// The smallest and largest window (logical px) for the video's aspect: the largest is
    /// bounded by `media.pip_max_width` and `pip_max_screen_fraction` of `monitor`, and is never
    /// smaller than the smallest.
    fn limits(&self, config: &Config, monitor: Option<&winit::monitor::MonitorHandle>) -> (Vec2, Vec2) {
        let min = self.min_size();
        let m = &config.media;
        let screen = monitor.map(|h| {
            let size = h.size();
            Vec2::new(size.width as f32, size.height as f32) / (h.scale_factor() as f32).max(0.1)
        });
        let bound = box_limit(m.pip_max_width, m.pip_max_screen_fraction, screen, min);
        let max = if self.lock_aspect { max_window(self.aspect, bound, min) } else { bound.max(min) };
        (min, max)
    }

    fn current_limits(&self, config: &Config) -> (Vec2, Vec2) {
        let monitor = self.output.as_ref().and_then(|o| o.window.current_monitor());
        self.limits(config, monitor.as_ref())
    }

    /// Tells the window manager the aspect (X11 size hint) and the size limits when they
    /// changed. During a re-fit the aspect and the minimum are left out, so the window manager
    /// does not bend the animation's in-between sizes.
    fn update_hints(&mut self, config: &Config) {
        let Some(output) = &self.output else { return };
        let refitting = self.refit.is_some();
        let hint = (self.lock_aspect && !refitting).then_some(self.aspect);
        let (min, max) = self.current_limits(config);
        let key = (
            hint.map(|a| (a * 1000.0).round() / 1000.0),
            [min.x.round() as i32, min.y.round() as i32, max.x.round() as i32, max.y.round() as i32],
            refitting,
        );
        if self.hinted == Some(key) {
            return;
        }
        self.hinted = Some(key);
        output.window.set_min_inner_size((!refitting).then(|| LogicalSize::new(min.x, min.y)));
        output.window.set_max_inner_size(Some(LogicalSize::new(max.x, max.y)));
        #[cfg(target_os = "linux")]
        set_x11_aspect(&output.window, key.0);
    }

    /// A drag of the window's edge begins (`direction` known), measured from its size now.
    fn begin_drag(&mut self, direction: Option<ResizeDirection>) {
        let Some(output) = &self.output else { return };
        if self.refit.is_some() {
            return;
        }
        let size = output.window.inner_size();
        self.asked_at = None;
        self.drag = Some(ResizeDrag {
            direction,
            start: Vec2::new(size.width as f32, size.height as f32),
            last_size_at: Instant::now(),
            moved: false,
            user: true,
        });
    }

    /// The resize is over: the size it ended at is what the user chose.
    fn end_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        if !drag.moved || !drag.user {
            return;
        }
        if let Some(output) = &self.output {
            let size = output.logical_size();
            self.remembered = Some(size);
            self.user_size = Some(size);
        }
    }

    /// The window has a new size from outside (the user dragging an edge, a tiling window
    /// manager, ...): answered at once, with the size of the video's aspect the pointer asks
    /// for. No delay, no rate limit: the window follows the pointer.
    fn size_arrived(&mut self, size: PhysicalSize<u32>, config: &Config) {
        if self.refit.is_some() || size.width == 0 || size.height == 0 {
            return;
        }
        let current = Vec2::new(size.width as f32, size.height as f32);
        let settled = self.settled.map_or(current, |s| Vec2::new(s.width as f32, s.height as f32));
        if self.drag.is_none() {
            // Nothing new: the size we meant to have, or a first configure.
            if (current - settled).abs().max_element() <= 1.0 {
                self.settled = Some(size);
                return;
            }
            // Right after a re-fit this is the window manager clamping the new size, not the user.
            let grace = Duration::from_secs_f32(config.media.pip_resize_idle_seconds.max(0.0));
            let user = self.refit_ended.is_none_or(|t| t.elapsed() > grace);
            self.drag = Some(ResizeDrag {
                direction: None,
                start: settled,
                last_size_at: Instant::now(),
                moved: false,
                user,
            });
        }
        let Some(drag) = &mut self.drag else { return };
        drag.last_size_at = Instant::now();
        drag.moved = true;
        let drag = *drag;
        if !self.lock_aspect {
            self.settled = Some(size);
            return;
        }
        let Some(output) = &self.output else { return };
        let scale = output.scale();
        let (min, max) = self.current_limits(config);
        let wanted =
            resize_fit(current, drag.start, drag.direction, self.aspect, min * scale, max * scale).round();
        let wanted = PhysicalSize::new(wanted.x.max(1.0) as u32, wanted.y.max(1.0) as u32);
        self.settled = Some(wanted);
        if wanted.width.abs_diff(size.width).max(wanted.height.abs_diff(size.height)) <= 1 {
            return;
        }
        // Not asked again if the window manager keeps this very size (tiling, size limits).
        if self.asked_at == Some(size) {
            return;
        }
        self.asked_at = Some(size);
        tracing::debug!(from = ?size, to = ?wanted, "PiP window follows the pointer at the video's aspect");
        if let Some(granted) = output.window.request_inner_size(wanted) {
            // Applied right away (Wayland: the window is as big as what it draws): no
            // `Resized` follows for it.
            self.settled = Some(granted);
            self.resized(granted);
        }
    }

    /// Another video's aspect: the window takes it once, keeping its area, centred where it
    /// was, and on screen (placing it needs a platform that allows it; Wayland leaves that to
    /// the compositor, which keeps the corner).
    fn start_refit(&mut self, config: &Config) {
        let Some(output) = &self.output else { return };
        let from = output.logical_size();
        let (min, max) = self.current_limits(config);
        let to = clamp_size(size_for_area(from.x * from.y, self.aspect), self.aspect, min, max);
        let centre = output.window.outer_position().ok().map(|p| {
            let size = output.window.outer_size();
            Vec2::new(p.x as f32 + size.width as f32 / 2.0, p.y as f32 + size.height as f32 / 2.0)
        });
        tracing::debug!(?from, ?to, "PiP window re-fits to the new video's aspect");
        let mut progress = Animated::new(0.0, drip::spring(config, config.motion.smooth));
        progress.target = 1.0;
        self.drag = None;
        self.refit = Some(Refit { from, to, centre, progress, armed: false });
    }

    /// One frame of the re-fit: the window's size (and place) in between.
    fn step_refit(&mut self, dt: f32, config: &Config) {
        let Some(refit) = &mut self.refit else { return };
        if !refit.armed {
            refit.armed = true;
            return;
        }
        let reduce = config.glass.reduce_motion;
        if reduce {
            refit.progress.snap(1.0);
        } else {
            refit.progress.step(dt);
        }
        let done = reduce || refit.progress.is_settled(0.002);
        let t = if done { 1.0 } else { refit.progress.value };
        let (from, to, centre) = (refit.from, refit.to, refit.centre);
        let size = from + (to - from) * t;
        let Some(output) = &self.output else { return };
        let scale = output.scale();
        let physical = PhysicalSize::new(
            (size.x * scale).round().max(1.0) as u32,
            (size.y * scale).round().max(1.0) as u32,
        );
        // The place keeps the centre, kept on the monitor.
        if let Some(centre) = centre {
            let mut origin = centre - Vec2::new(physical.width as f32, physical.height as f32) / 2.0;
            if let Some(m) = output.window.current_monitor() {
                let (mp, ms) = (m.position(), m.size());
                let low = Vec2::new(mp.x as f32, mp.y as f32);
                let high = (low + Vec2::new(ms.width as f32, ms.height as f32)
                    - Vec2::new(physical.width as f32, physical.height as f32))
                .max(low);
                origin = origin.clamp(low, high);
            }
            output
                .window
                .set_outer_position(PhysicalPosition::new(origin.x.round() as i32, origin.y.round() as i32));
        }
        if let Some(granted) = output.window.request_inner_size(physical) {
            self.resized(granted);
        }
        self.settled = Some(physical);
        if done {
            self.refit = None;
            self.refit_ended = Some(Instant::now());
            // The aspect hint and the minimum size come back.
            self.hinted = None;
            self.asked_at = None;
        }
    }
}

/// Side (logical px) of the loading ring's disc.
const SPINNER_SIDE: f32 = 52.0;

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

/// The box the window must stay in: `max_width` wide at most and `fraction` of the monitor's
/// size either way (no monitor known: only the width limits), never smaller than `min`.
fn box_limit(max_width: f32, fraction: f32, monitor: Option<Vec2>, min: Vec2) -> Vec2 {
    let fraction = fraction.clamp(0.1, 1.0);
    let mut bound = Vec2::new(max_width, f32::MAX);
    if let Some(screen) = monitor {
        bound = bound.min(screen * fraction);
    }
    bound.max(min)
}

/// The largest window of aspect `aspect` that fits `bound`, and is no smaller than `min`.
fn max_window(aspect: f32, bound: Vec2, min: Vec2) -> Vec2 {
    let aspect = aspect.max(0.01);
    let width = bound.x.min(bound.y * aspect);
    let size = Vec2::new(width, width / aspect);
    size.max(min_window(aspect, min.x, min.y))
}

/// `size` scaled (keeping `aspect`) into `min` .. `max` (the minimum wins where they clash).
fn clamp_size(size: Vec2, aspect: f32, min: Vec2, max: Vec2) -> Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return min_window(aspect, min.x, min.y);
    }
    let shrink = (max.x / size.x).min(max.y / size.y).min(1.0);
    let size = size * shrink;
    let floor = min_window(aspect, min.x, min.y);
    let grow = (floor.x / size.x).max(floor.y / size.y).max(1.0);
    size * grow
}

/// The window of aspect `aspect` (width / height) with the given area.
fn size_for_area(area: f32, aspect: f32) -> Vec2 {
    let aspect = aspect.max(0.01);
    let width = (area.max(1.0) * aspect).sqrt();
    Vec2::new(width, width / aspect)
}

/// The size of aspect `aspect` (width / height) the pointer asks for when the window manager
/// reports `current` for a resize that began at `start`: an edge drag's side drives and the
/// other follows (`direction` North/South: the height, East/West: the width); at a corner (or
/// with no direction) the side that changed more, relative to `start`, drives. Within
/// `min` .. `max`.
fn resize_fit(
    current: Vec2,
    start: Vec2,
    direction: Option<ResizeDirection>,
    aspect: f32,
    min: Vec2,
    max: Vec2,
) -> Vec2 {
    if aspect <= 0.0 || current.x <= 0.0 || current.y <= 0.0 {
        return current;
    }
    let changed = |now: f32, before: f32| if before > 0.0 { (now / before - 1.0).abs() } else { 0.0 };
    let height_drives = match direction {
        Some(ResizeDirection::North | ResizeDirection::South) => true,
        Some(ResizeDirection::East | ResizeDirection::West) => false,
        _ => changed(current.y, start.y) > changed(current.x, start.x),
    };
    let size = if height_drives {
        Vec2::new(current.y * aspect, current.y)
    } else {
        Vec2::new(current.x, current.x / aspect)
    };
    clamp_size(size, aspect, min, max)
}

/// Sets (or clears) the aspect-ratio size hint of an X11 window: the window manager then only
/// resizes it with that aspect. Wayland has no such thing (xdg-toplevel cannot lock an aspect,
/// and KWin's scripting API has no aspect property), so there the window answers every new size at once instead.
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

    const FAR: Vec2 = Vec2::splat(100_000.0);

    #[test]
    fn an_edge_drives_its_own_axis() {
        let start = Vec2::new(480.0, 270.0);
        let a = 16.0 / 9.0;
        // East/west: the width is the pointer's, the height follows.
        for d in [ResizeDirection::East, ResizeDirection::West] {
            let out = resize_fit(Vec2::new(640.0, 270.0), start, Some(d), a, Vec2::ZERO, FAR);
            assert_eq!(out, Vec2::new(640.0, 360.0));
        }
        // North/south: the height is the pointer's, even if the width also changed.
        for d in [ResizeDirection::North, ResizeDirection::South] {
            let out = resize_fit(Vec2::new(500.0, 360.0), start, Some(d), a, Vec2::ZERO, FAR);
            assert_eq!(out, Vec2::new(640.0, 360.0));
        }
    }

    #[test]
    fn a_corner_follows_the_side_the_pointer_moved_further() {
        let start = Vec2::new(480.0, 270.0);
        let a = 16.0 / 9.0;
        let se = Some(ResizeDirection::SouthEast);
        // Mostly wider (+25 % vs. +10 % height): the width wins.
        assert_eq!(resize_fit(Vec2::new(600.0, 297.0), start, se, a, Vec2::ZERO, FAR).x, 600.0);
        // Mostly taller: the height wins.
        assert_eq!(resize_fit(Vec2::new(500.0, 400.0), start, se, a, Vec2::ZERO, FAR).y, 400.0);
        // Already at the aspect: unchanged, and the result is stable when fed back.
        assert_eq!(resize_fit(start, start, se, a, Vec2::ZERO, FAR), start);
        let out = resize_fit(Vec2::new(600.0, 297.0), start, se, a, Vec2::ZERO, FAR);
        assert_eq!(resize_fit(out, start, se, a, Vec2::ZERO, FAR), out);
    }

    #[test]
    fn dragging_is_linear_and_continuous() {
        // Pulling an east edge steadily: the answer grows steadily, with no jump and never
        // off the aspect.
        let start = Vec2::new(480.0, 270.0);
        let a = 16.0 / 9.0;
        let mut last = start;
        for i in 1..=200 {
            let pulled = Vec2::new(480.0 + i as f32 * 2.0, 270.0);
            let out = resize_fit(pulled, start, Some(ResizeDirection::East), a, Vec2::ZERO, FAR);
            assert!((out.x - last.x - 2.0).abs() < 1e-3, "step {i}: {last:?} -> {out:?}");
            assert!((out.x / out.y - a).abs() < 1e-4);
            last = out;
        }
        // A corner drag with a drifting pointer: the size is a continuous function of it.
        let mut prev = start;
        for i in 1..=200 {
            let p = Vec2::new(480.0 + i as f32 * 1.5, 270.0 + i as f32 * 0.5);
            let out = resize_fit(p, start, Some(ResizeDirection::SouthEast), a, Vec2::ZERO, FAR);
            assert!((out - prev).length() < 4.0, "step {i}: jump {prev:?} -> {out:?}");
            prev = out;
        }
    }

    #[test]
    fn the_size_stays_within_the_limits_at_the_aspect() {
        let z = sizing();
        let m = Config::default().media;
        let screen = Some(Vec2::new(1920.0, 1080.0));
        for aspect in [21.0 / 9.0, 16.0 / 9.0, 1.0, 9.0 / 16.0, 1.0 / 3.0, 4.0] {
            let min = min_window(aspect, m.pip_min_width, z.min_height());
            let bound = box_limit(m.pip_max_width, m.pip_max_screen_fraction, screen, min);
            let max = max_window(aspect, bound, min);
            assert!(max.x >= min.x - 0.01 && max.y >= min.y - 0.01, "{aspect}: {min:?} {max:?}");
            assert!(max.x <= m.pip_max_width + 0.01 || max.x <= min.x + 0.01);
            assert!((max.x / max.y - aspect).abs() < 1e-3);
            let start = min * 2.0;
            let dirs = [None, Some(ResizeDirection::East), Some(ResizeDirection::North)];
            for current in
                [Vec2::new(10.0, start.y), Vec2::new(start.x, 10.0), Vec2::ONE, Vec2::splat(9000.0)]
            {
                for d in dirs {
                    let out = resize_fit(current, start, d, aspect, min, max);
                    assert!(out.x >= min.x - 0.01 && out.y >= min.y - 0.01, "{aspect} {d:?}: {out:?}");
                    assert!(out.x <= max.x + 0.01 && out.y <= max.y + 0.01, "{aspect} {d:?}: {out:?}");
                    assert!((out.x / out.y - aspect).abs() < 1e-3, "{aspect}: {out:?}");
                }
            }
        }
        // 21:9 is limited by the width, 9:16 by the controls' height, never letterboxed.
        assert_eq!(min_window(21.0 / 9.0, 272.0, 100.0).x, 272.0);
        assert!(min_window(9.0 / 16.0, 272.0, 100.0).y > 272.0);
        assert!(min_window(4.0, 272.0, 100.0).y >= 100.0 - 0.01);
        // The screen bounds the window even where `pip_max_width` allows more.
        let small = box_limit(1280.0, 0.9, Some(Vec2::new(800.0, 600.0)), Vec2::new(272.0, 153.0));
        assert_eq!(small, Vec2::new(720.0, 540.0));
    }

    #[test]
    fn another_aspect_keeps_the_area() {
        let from = Vec2::new(480.0, 270.0);
        let area = from.x * from.y;
        for aspect in [4.0 / 3.0, 1.0, 9.0 / 16.0, 21.0 / 9.0] {
            let to = size_for_area(area, aspect);
            assert!((to.x * to.y - area).abs() < 1.0, "{aspect}: {to:?}");
            assert!((to.x / to.y - aspect).abs() < 1e-4);
        }
        assert_eq!(size_for_area(area, 16.0 / 9.0).round(), from);
        // Clamped into the box, aspect kept.
        let big =
            clamp_size(Vec2::new(3000.0, 1500.0), 2.0, Vec2::new(272.0, 136.0), Vec2::new(1000.0, 900.0));
        assert_eq!(big, Vec2::new(1000.0, 500.0));
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
