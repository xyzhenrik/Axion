//! Picture in Picture: the video in its own small, borderless window that stays above all
//! other windows and can be dragged anywhere on the screen. Its controls (close, back to the
//! tab, ±10 s, play/pause, progress) show while the pointer is over it (Safari, iPadOS 26).
//!
//! The picture is the video's tab's frame: cropped to the video while that tab is shown in the
//! browser, and — while another tab is shown — the whole frame of the hidden tab, in which the
//! bridge makes the video cover the page ("takeover").
//!
//! "Always on top": winit sets it on X11. Wayland gives apps no way to ask for it, so on KDE
//! Plasma a small KWin script keeps the window above (see `keep_above_on_kwin`).

use std::sync::Arc;
use std::time::Instant;

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
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

const ACTION_BASE: u32 = 9700;
/// The window's width (logical px); the height follows the video.
const WIDTH: f32 = 480.0;
const MAX_HEIGHT: f32 = 480.0;
const MIN_WIDTH: f32 = 240.0;
const RADIUS: f32 = 16.0;
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
            .with_background(Background::Glass(GlassLevel::Thick)),
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

pub struct PipWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    compositor: Compositor,
    ui: Ui,
    transparent: bool,
    picture: NodeId,
    controls: NodeId,
    play_icon: NodeId,
    progress: NodeId,
    controls_opacity: Animated<f32>,
    hovered: bool,
    last_frame: Instant,
    /// The PiP'd video, while PiP is on.
    pub view: Option<ViewId>,
    pub player: Option<Player>,
    /// The bridge has been asked to let the video cover its (hidden) tab.
    pub takeover: bool,
}

impl PipWindow {
    /// Opens the (hidden) window; [`PipWindow::show`] fills and shows it.
    pub fn new(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &Config,
    ) -> Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(TITLE)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(true)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_min_inner_size(LogicalSize::new(MIN_WIDTH, MIN_WIDTH * 9.0 / 16.0))
            .with_inner_size(LogicalSize::new(WIDTH, WIDTH * 9.0 / 16.0));
        #[cfg(target_os = "linux")]
        let attributes = {
            use winit::platform::wayland::WindowAttributesExtWayland as _;
            attributes.with_name("axion", "axion-pip")
        };
        let window = Arc::new(event_loop.create_window(attributes).context("opening the PiP window")?);
        let surface = instance.create_surface(window.clone()).context("PiP window surface")?;
        let caps = surface.get_capabilities(adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        // Rounded corners need a transparent window; without one the corners stay black.
        let transparent = caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied);
        let alpha_mode =
            if transparent { wgpu::CompositeAlphaMode::PreMultiplied } else { caps.alpha_modes[0] };
        // Never wait for vsync here: the browser window already does, and both render on one
        // thread.
        let present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::Fifo);
        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &surface_config);

        let scale = window.scale_factor() as f32;
        let mut ui = Ui::new(Theme::new(config, true), scale);
        let compositor = Compositor::new(device, queue, format, &mut ui.fonts());
        let root = ui.root();
        let picture = ui.add(
            root,
            Widget::image().with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: percent(1.0), height: percent(1.0) },
                ..Default::default()
            },
        );
        let controls = ui.add(
            root,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(axion_config::Color::rgb(0x000000)), 0.2))
                .with_radius(RADIUS),
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
        ui.set_viewport(Vec2::new(size.width as f32, size.height as f32) / scale, scale);
        Ok(Self {
            window,
            surface,
            surface_config,
            device: device.clone(),
            queue: queue.clone(),
            compositor,
            ui,
            transparent,
            picture,
            controls,
            play_icon,
            progress,
            controls_opacity: Animated::new(0.0, Spring::SNAPPY),
            hovered: false,
            last_frame: Instant::now(),
            view: None,
            player: None,
            takeover: false,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn is_active(&self) -> bool {
        self.view.is_some()
    }

    /// Shows the window for `player` of `view`, sized to the video.
    pub fn show(&mut self, view: ViewId, player: Player) {
        let aspect = player.aspect().max(0.2);
        let height = (WIDTH / aspect).min(MAX_HEIGHT);
        let _ = self.window.request_inner_size(LogicalSize::new(height * aspect, height));
        self.view = Some(view);
        self.player = Some(player);
        self.takeover = false;
        self.window.set_visible(true);
        // Set again once mapped: X11 window managers drop a level set on a hidden window.
        self.window.set_window_level(WindowLevel::AlwaysOnTop);
        keep_above_on_kwin();
    }

    pub fn hide(&mut self) {
        self.view = None;
        self.player = None;
        self.takeover = false;
        self.window.set_visible(false);
    }

    /// The last rendered frame (for `--capture`).
    pub fn read_back(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.compositor.read_back()
    }

    pub fn set_theme(&mut self, config: &Config) {
        self.ui.set_theme(Theme::new(config, true));
    }

    /// Input for the PiP window. Dragging anywhere but on a button moves the window.
    pub fn window_event(&mut self, event: WindowEvent, config: &Config) -> Option<PipOutcome> {
        match event {
            WindowEvent::CloseRequested => {
                let (view, player) = (self.view?, self.player.clone()?);
                return Some(PipOutcome::Close(view, player.id));
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                self.surface_config.width = size.width;
                self.surface_config.height = size.height;
                self.surface.configure(&self.device, &self.surface_config);
                let scale = self.window.scale_factor() as f32;
                self.ui.set_viewport(Vec2::new(size.width as f32, size.height as f32) / scale, scale);
            }
            WindowEvent::CursorEntered { .. } => self.hovered = true,
            WindowEvent::CursorLeft { .. } => {
                self.hovered = false;
                self.ui.handle(UiInput::PointerLeave);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.hovered = true;
                let p = Vec2::new(position.x as f32, position.y as f32) / self.window.scale_factor() as f32;
                self.ui.handle(UiInput::PointerMove(p));
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    let (consumed, _) = self.ui.handle(UiInput::PointerDown);
                    if !consumed || self.ui.pressed_action().is_none() {
                        // Not on a button: the window follows the pointer (the window manager
                        // moves it, so it can go anywhere on the screen).
                        let _ = self.window.drag_window();
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

    fn click(&mut self, control: PipControl, config: &Config) -> Option<PipOutcome> {
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
    pub fn render(&mut self, picture: Option<Picture>) {
        if !self.is_active() {
            return;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        if let Some((image, uv)) = picture {
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

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            _ => return,
        };
        let target = frame.texture.create_view(&Default::default());
        let mut layers = Vec::new();
        self.ui.paint(&mut layers);
        let clear = if self.transparent { [0.0, 0.0, 0.0, 0.0] } else { [0.0, 0.0, 0.0, 1.0] };
        let scene = Scene { clear, layers };
        let size = (self.surface_config.width, self.surface_config.height);
        self.compositor.render(&scene, &mut self.ui.fonts(), &target, size);
        drop(scene);
        self.queue.present(frame);
    }
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
    fn control_ids_round_trip() {
        for c in CONTROLS {
            assert_eq!(PipControl::from_id(c.id()), Some(c));
        }
    }
}
