//! Axion's own media controls, laid out like Safari's player on iPadOS 26 (see
//! docs/reference/safari-ipados26.md): dark glass circles for close / fullscreen and
//! Picture-in-Picture at the top left, a volume capsule at the top right, skip back, play/pause
//! and skip forward in the middle, the progress capsule along the bottom and a speed button
//! (with its menu) above it. Driven through the media bridge.
//!
//! They only take over in fullscreen: inline, pages keep their own controls. When a video goes
//! fullscreen (the page's button, double-click, F), Axion's player covers it — a site's own
//! player UI is hidden around the video ("takeover"). Trusted sites (`media.trusted_player_sites`:
//! YouTube, Twitch…) keep their player also in fullscreen. The controls hide after a moment
//! without pointer movement.

use std::time::{Duration, Instant};

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_engine::ViewId;
use axion_i18n::tr;
use axion_media::{Action, Command, Kind, PageMedia, Player, format_time};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Align, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

const ACTION_BASE: u32 = 9500;
const SPEED_CHOICE_BASE: u32 = ACTION_BASE + 50;
/// Icon size relative to its button.
const ICON_RATIO: f32 = 0.5;
/// After scrubbing, page reports still carry the old time for a moment; ignore them.
const SCRUB_SETTLE: Duration = Duration::from_millis(600);
/// The progress bar's grab area is never shorter than this (logical px).
pub const MIN_HIT_HEIGHT: f32 = 24.0;
const SETTLED: f32 = 0.01;
const SPEED_MENU_WIDTH: f32 = 200.0;
const SPEED_ROW: f32 = 34.0;
/// Space between neighbouring glass circles: wide enough that they don't flow into each other.
const CIRCLE_GAP: f32 = 16.0;

/// Sizes (logical px), measured on Safari's fullscreen player.
#[derive(Clone, Copy)]
struct Metrics {
    margin: f32,
    corner: f32,
    volume: Vec2,
    skip: f32,
    play: f32,
    centre_gap: f32,
    bar_height: f32,
    small: f32,
}

const FULLSCREEN: Metrics = Metrics {
    margin: 18.0,
    corner: 38.0,
    volume: Vec2::new(184.0, 38.0),
    skip: 56.0,
    play: 82.0,
    centre_gap: 46.0,
    bar_height: 40.0,
    small: 32.0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    SkipBack,
    SkipForward,
    Mute,
    Speed,
    Captions,
    /// Close: leave fullscreen.
    Fullscreen,
    PictureInPicture,
    /// Save the playing video to the Playlist.
    Playlist,
    /// An entry of the speed menu (index into `media.playback_speeds`).
    SpeedChoice(usize),
}

const CONTROLS: [Control; 9] = [
    Control::PlayPause,
    Control::SkipBack,
    Control::SkipForward,
    Control::Mute,
    Control::Speed,
    Control::Captions,
    Control::Fullscreen,
    Control::PictureInPicture,
    Control::Playlist,
];

impl Control {
    fn id(self) -> ActionId {
        match self {
            Control::SpeedChoice(i) => ActionId(SPEED_CHOICE_BASE + i as u32),
            simple => ActionId(ACTION_BASE + CONTROLS.iter().position(|&c| c == simple).unwrap_or(0) as u32),
        }
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        if let Some(i) = id.0.checked_sub(SPEED_CHOICE_BASE).filter(|&i| i < 32) {
            return Some(Control::SpeedChoice(i as usize));
        }
        CONTROLS.get(id.0.checked_sub(ACTION_BASE)? as usize).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Hidden,
    Fullscreen,
}

/// What the overlay needs to know each frame.
pub struct Context<'a> {
    pub view: Option<ViewId>,
    pub media: Option<&'a PageMedia>,
    /// The page area in logical pixels.
    pub page: Frame,
    pub pointer: Option<Vec2>,
    /// The page made something fullscreen.
    pub page_fullscreen: bool,
}

/// What the browser should do after a click on the controls.
pub enum Outcome {
    Command(ViewId, Command),
    PictureInPicture(ViewId, Player),
    /// Add this video to the Playlist.
    AddToPlaylist(ViewId, Player),
    Nothing,
}

pub struct PlayerOverlay {
    root: NodeId,
    top_left: NodeId,
    first_icon: NodeId,
    playlist_button: NodeId,
    playlist_icon: NodeId,
    volume_capsule: NodeId,
    volume: NodeId,
    mute_icon: NodeId,
    centre: NodeId,
    skip_back: NodeId,
    play: NodeId,
    play_icon: NodeId,
    skip_forward: NodeId,
    side_buttons: NodeId,
    captions: NodeId,
    speed_menu: NodeId,
    speed_menu_open: bool,
    bar: NodeId,
    time: NodeId,
    scrubber: NodeId,
    remaining: NodeId,
    opacity: Animated<f32>,
    mode: Mode,
    /// The nodes have been laid out (once, on first use).
    laid_out: bool,
    /// The view and player the controls are about.
    target: Option<(ViewId, Player)>,
    last_pointer: Option<Vec2>,
    last_activity: Instant,
    scrubbed_at: Option<Instant>,
    volume_set_at: Option<Instant>,
    /// The site player whose video was asked to cover the site's own fullscreen UI.
    takeover: Option<String>,
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn square(side: f32) -> Size<Dimension> {
    Size { width: length(side), height: length(side) }
}

fn absolute(
    left: LengthPercentageAuto,
    top: LengthPercentageAuto,
    right: LengthPercentageAuto,
    bottom: LengthPercentageAuto,
) -> Style {
    Style { position: Position::Absolute, inset: Rect { left, right, top, bottom }, ..Default::default() }
}

/// A dark glass circle with an icon (and, for the skip buttons, the seconds inside it). Pure
/// glass at any size: no highlight fill under the pointer, and pressed it becomes a lens of
/// its own that swells and follows the pointer, like the other glass buttons.
fn circle(
    ui: &mut Ui,
    parent: NodeId,
    control: Control,
    icon: Icon,
    side: f32,
    text: Option<&str>,
) -> (NodeId, NodeId) {
    let node = ui.add(
        parent,
        Widget::container()
            .with_action(control.id())
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thick))
            .pressed_whole()
            .without_hover_fill(),
        Style { size: square(side), flex_shrink: 0.0, justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let glyph_side = side * ICON_RATIO;
    let glyph = ui.add(node, Widget::icon(icon, glyph_side).with_paint(Paint::Label), Style::default());
    if let Some(text) = text {
        // Drawn over the icon: same width, pulled back over it with a negative margin.
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
    (node, glyph)
}

fn time_label(ui: &mut Ui, parent: NodeId, text: &str) -> NodeId {
    let label =
        ui.add_label(parent, text, TextStyle::Caption, Style { flex_shrink: 0.0, ..Default::default() });
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

impl PlayerOverlay {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container(),
            Style { position: Position::Absolute, ..Default::default() },
        );
        ui.set_plane_dark(root, true);

        // Top left: close / fullscreen and Picture-in-Picture.
        let top_left = ui.add(root, Widget::container(), Style { ..row(CIRCLE_GAP) });
        let (_, first_icon) =
            circle(ui, top_left, Control::Fullscreen, Icon::Fullscreen, FULLSCREEN.corner, None);
        circle(ui, top_left, Control::PictureInPicture, Icon::PictureInPicture, FULLSCREEN.corner, None);
        let (playlist_button, playlist_icon) =
            circle(ui, top_left, Control::Playlist, Icon::PlaylistAdd, FULLSCREEN.corner, None);
        ui.set_visible(playlist_button, false);

        // Top right: volume.
        let volume_capsule = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(CAPSULE),
            Style {
                padding: Rect {
                    left: length(14.0),
                    right: length(4.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..row(6.0)
            },
        );
        let volume = ui.add(
            volume_capsule,
            Widget::line_slider(1.0),
            Style {
                flex_grow: 1.0,
                min_size: Size { width: length(40.0), height: auto() },
                ..Default::default()
            },
        );
        let mute = ui.add(
            volume_capsule,
            Widget::container().with_action(Control::Mute.id()).with_radius(CAPSULE),
            Style {
                size: square(30.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        let mute_icon =
            ui.add(mute, Widget::icon(Icon::Volume, 18.0).with_paint(Paint::Label), Style::default());

        // Centre: skip back, play/pause, skip forward. The row covers the video but takes no
        // clicks itself (no background, no action).
        let centre = ui.add(
            root,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: percent(1.0) },
                justify_content: JustifyContent::CENTER,
                ..row(FULLSCREEN.centre_gap)
            },
        );
        let (skip_back, _) =
            circle(ui, centre, Control::SkipBack, Icon::SkipBack, FULLSCREEN.skip, Some("10"));
        let (play, play_icon) = circle(ui, centre, Control::PlayPause, Icon::Pause, FULLSCREEN.play, None);
        let (skip_forward, _) =
            circle(ui, centre, Control::SkipForward, Icon::SkipForward, FULLSCREEN.skip, Some("10"));

        // Above the bar at the right: captions and speed.
        let side_buttons = ui.add(root, Widget::container(), Style { ..row(CIRCLE_GAP) });
        let (captions, _) =
            circle(ui, side_buttons, Control::Captions, Icon::Captions, FULLSCREEN.small, None);
        circle(ui, side_buttons, Control::Speed, Icon::Gauge, FULLSCREEN.small, None);
        let speed_menu = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(18.0),
            Style {
                size: Size { width: length(SPEED_MENU_WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(6.0),
                    right: length(6.0),
                    top: length(8.0),
                    bottom: length(6.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(speed_menu, false);

        // Bottom: elapsed, progress, remaining in one capsule.
        let bar = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(CAPSULE),
            Style {
                padding: Rect {
                    left: length(16.0),
                    right: length(16.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..row(10.0)
            },
        );
        let time = time_label(ui, bar, "0:00");
        let scrubber = ui.add(
            bar,
            Widget::line_slider(0.0),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(40.0), height: auto() },
                ..Default::default()
            },
        );
        let remaining = time_label(ui, bar, "-0:00");
        ui.set_visible(root, false);
        Self {
            root,
            top_left,
            first_icon,
            playlist_button,
            playlist_icon,
            volume_capsule,
            volume,
            mute_icon,
            centre,
            skip_back,
            play,
            play_icon,
            skip_forward,
            side_buttons,
            captions,
            speed_menu,
            speed_menu_open: false,
            bar,
            time,
            scrubber,
            remaining,
            opacity: Animated::new(0.0, Spring::SNAPPY),
            mode: Mode::Hidden,
            laid_out: false,
            target: None,
            last_pointer: None,
            last_activity: Instant::now(),
            scrubbed_at: None,
            volume_set_at: None,
            takeover: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The video the controls are about right now.
    pub fn target(&self) -> Option<(ViewId, &Player)> {
        self.target.as_ref().map(|(view, player)| (*view, player))
    }

    /// The Playlist button: hidden (`None`; private tabs), add, or already saved.
    pub fn set_playlist_button(&self, ui: &mut Ui, state: Option<bool>) {
        ui.set_visible(self.playlist_button, state.is_some());
        if let Some(added) = state {
            ui.set_icon(self.playlist_icon, if added { Icon::Check } else { Icon::PlaylistAdd });
        }
    }

    /// The scrubber node, whose value changes seek.
    pub fn scrubber(&self) -> NodeId {
        self.scrubber
    }

    /// The volume slider node.
    pub fn volume(&self) -> NodeId {
        self.volume
    }

    /// The controls are showing (fullscreen ones may have hidden themselves).
    pub fn is_shown(&self) -> bool {
        self.mode != Mode::Hidden && self.opacity.target > 0.5
    }

    /// The pointer moved: fullscreen controls that had faded out are there again at once (not
    /// only with the next frame), so a press right after the move hits them.
    pub fn reveal(&mut self, ui: &mut Ui) {
        if self.mode == Mode::Fullscreen && self.target.is_some() && !ui.is_visible(self.root) {
            self.wake();
            ui.set_visible(self.root, true);
            ui.layout();
        }
    }

    /// Something the user did that should bring the controls back (key press, click).
    pub fn wake(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Recomputes which video the controls are for, where they go and whether they show.
    pub fn update(&mut self, ui: &mut Ui, config: &Config, cx: &Context<'_>, dt: f32) {
        let now = Instant::now();
        if cx.pointer != self.last_pointer {
            self.last_pointer = cx.pointer;
            self.last_activity = now;
        }
        let (mode, player, rect) = self.choose(config, cx);
        self.mode = mode;
        let (Some(player), Some(view), Some(rect)) = (player, cx.view, rect) else {
            self.target = None;
            self.close_speed_menu(ui);
            self.fade(ui, false, dt);
            return;
        };
        if !self.laid_out {
            self.laid_out = true;
            self.apply_layout(ui, config);
        }
        self.place(ui, rect);
        self.show_state(ui, &player);
        let recently_active =
            now.duration_since(self.last_activity).as_secs_f32() < config.media.hide_controls_after_seconds;
        let held = ui.dragging().is_some_and(|n| n == self.scrubber || n == self.volume);
        let visible = held || self.speed_menu_open || !player.is_playing() || recently_active;
        self.target = Some((view, player));
        if !visible {
            self.close_speed_menu(ui);
        }
        self.fade(ui, visible, dt);
    }
    /// The fullscreen video Axion's player is for, if the page made one fullscreen.
    fn choose(&self, config: &Config, cx: &Context<'_>) -> (Mode, Option<Player>, Option<Frame>) {
        let Some(media) = cx.media else { return (Mode::Hidden, None, None) };
        if !cx.page_fullscreen || !config.media.fullscreen_player {
            return (Mode::Hidden, None, None);
        }
        let player = media
            .players
            .iter()
            .filter(|p| p.kind == Kind::Video && p.fullscreen && !p.trusted)
            .max_by(|a, b| a.area().total_cmp(&b.area()));
        match player {
            Some(p) => (Mode::Fullscreen, Some(p.clone()), Some(cx.page)),
            None => (Mode::Hidden, None, None),
        }
    }

    fn apply_layout(&self, ui: &mut Ui, config: &Config) {
        let m = FULLSCREEN;
        let set = |ui: &mut Ui, node: NodeId, f: &dyn Fn(&mut Style)| {
            let mut style = ui.style(node);
            f(&mut style);
            ui.set_style(node, style);
        };
        set(ui, self.top_left, &|st| {
            *st = Style { ..absolute(length(m.margin), length(m.margin), auto(), auto()) };
            st.flex_direction = FlexDirection::Row;
            st.align_items = AlignItems::CENTER;
            st.gap = Size { width: length(CIRCLE_GAP), height: length(0.0) };
        });
        for node in ui.children(self.top_left) {
            set(ui, node, &|st| st.size = square(m.corner));
        }
        set(ui, self.volume_capsule, &|st| {
            st.position = Position::Absolute;
            st.inset = Rect { left: auto(), top: length(m.margin), right: length(m.margin), bottom: auto() };
            st.size = Size { width: length(m.volume.x), height: length(m.volume.y) };
        });
        set(ui, self.centre, &|st| {
            st.position = Position::Absolute;
            st.inset = Rect { left: length(0.0), top: length(0.0), right: auto(), bottom: auto() };
            st.gap = Size { width: length(m.centre_gap), height: length(0.0) };
        });
        for node in [self.skip_back, self.skip_forward] {
            set(ui, node, &|st| st.size = square(m.skip));
        }
        set(ui, self.play, &|st| st.size = square(m.play));
        set(ui, self.bar, &|st| {
            st.position = Position::Absolute;
            st.inset = Rect {
                left: length(m.margin),
                top: auto(),
                right: length(m.margin),
                bottom: length(m.margin),
            };
            st.size = Size { width: auto(), height: length(m.bar_height) };
        });
        set(ui, self.side_buttons, &|st| {
            st.position = Position::Absolute;
            st.inset = Rect {
                left: auto(),
                top: auto(),
                right: length(m.margin),
                bottom: length(m.margin + m.bar_height + CIRCLE_GAP),
            };
        });
        for node in ui.children(self.side_buttons) {
            set(ui, node, &|st| st.size = square(m.small));
        }
        set(ui, self.speed_menu, &|st| {
            st.position = Position::Absolute;
            st.inset = Rect {
                left: auto(),
                top: auto(),
                right: length(m.margin),
                bottom: length(m.margin + m.bar_height + m.small + 2.0 * CIRCLE_GAP),
            };
        });
        // The progress bar: a thick track, and a tall area that grabs the pointer.
        let media = &config.media;
        if let Some(w) = ui.widget_mut(self.scrubber) {
            w.track = Some(media.progress_thickness.max(1.0));
        }
        set(ui, self.scrubber, &|st| {
            st.min_size.height = length(media.progress_hit_height.max(MIN_HIT_HEIGHT));
        });
        ui.set_icon(self.first_icon, Icon::Close);
    }

    fn place(&self, ui: &mut Ui, rect: Frame) {
        let mut style = ui.style(self.root);
        style.inset =
            Rect { left: length(rect.origin.x), top: length(rect.origin.y), right: auto(), bottom: auto() };
        style.size = Size { width: length(rect.size.x), height: length(rect.size.y) };
        if style != ui.style(self.root) {
            ui.set_style(self.root, style);
        }
    }

    fn show_state(&mut self, ui: &mut Ui, player: &Player) {
        ui.set_icon(self.play_icon, if player.is_playing() { Icon::Pause } else { Icon::Play });
        let silent = player.muted || player.volume == 0.0;
        ui.set_icon(self.mute_icon, if silent { Icon::Muted } else { Icon::Volume });
        ui.set_text(self.time, &format_time(player.time));
        let remaining = if player.live {
            tr("Live").to_owned()
        } else {
            format!("−{}", format_time(player.duration - player.time))
        };
        ui.set_text(self.remaining, &remaining);
        ui.set_visible(self.captions, !player.captions.is_empty());
        let scrubbing = ui.dragging() == Some(self.scrubber)
            || self.scrubbed_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
        if !scrubbing && player.duration > 0.0 {
            ui.set_value(self.scrubber, (player.time / player.duration) as f32);
        }
        let adjusting = ui.dragging() == Some(self.volume)
            || self.volume_set_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
        if !adjusting {
            ui.set_value(self.volume, if player.muted { 0.0 } else { player.volume as f32 });
        }
    }

    fn fade(&mut self, ui: &mut Ui, visible: bool, dt: f32) {
        self.opacity.target = if visible { 1.0 } else { 0.0 };
        self.opacity.step(dt);
        let shown = self.opacity.value > SETTLED || visible;
        ui.set_visible(self.root, shown);
        ui.set_opacity(self.root, self.opacity.value.clamp(0.0, 1.0));
    }

    fn close_speed_menu(&mut self, ui: &mut Ui) {
        if self.speed_menu_open {
            self.speed_menu_open = false;
            ui.set_visible(self.speed_menu, false);
        }
    }

    /// Opens the speed menu ("Playback Speed", fastest first, the current one checked).
    fn toggle_speed_menu(&mut self, ui: &mut Ui, config: &Config) {
        if self.speed_menu_open {
            self.close_speed_menu(ui);
            return;
        }
        let Some((_, player)) = &self.target else { return };
        let rate = player.rate;
        ui.clear_children(self.speed_menu);
        let header = ui.add_label(
            self.speed_menu,
            tr("Playback Speed"),
            TextStyle::Footnote,
            Style {
                margin: Rect {
                    left: length(12.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(4.0),
                },
                ..Default::default()
            },
        );
        if let Some(w) = ui.widget_mut(header) {
            w.set_paint(Paint::Secondary);
        }
        let mut speeds: Vec<(usize, f32)> =
            config.media.playback_speeds.iter().copied().enumerate().collect();
        speeds.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (i, speed) in speeds {
            let row_node = ui.add(
                self.speed_menu,
                Widget::container().with_action(Control::SpeedChoice(i).id()).with_radius(10.0),
                Style {
                    size: Size { width: auto(), height: length(SPEED_ROW) },
                    padding: Rect {
                        left: length(8.0),
                        right: length(12.0),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    ..row(6.0)
                },
            );
            let slot = ui.add(
                row_node,
                Widget::container(),
                Style { size: square(16.0), flex_shrink: 0.0, ..Default::default() },
            );
            if (f64::from(speed) - rate).abs() < 0.01 {
                ui.add(slot, Widget::icon(Icon::Check, 16.0).with_paint(Paint::Label), Style::default());
            }
            ui.add_label(row_node, &format_rate(f64::from(speed)), TextStyle::Body, Style::default());
        }
        ui.set_visible(self.speed_menu, true);
        self.speed_menu_open = true;
    }

    /// What a click on one of the controls does.
    pub fn click(&mut self, ui: &mut Ui, control: Control, config: &Config) -> Outcome {
        self.wake();
        match control {
            Control::Speed => {
                self.toggle_speed_menu(ui, config);
                return Outcome::Nothing;
            }
            Control::PictureInPicture => {
                self.close_speed_menu(ui);
                return match &self.target {
                    Some((view, player)) => Outcome::PictureInPicture(*view, player.clone()),
                    None => Outcome::Nothing,
                };
            }
            Control::Playlist => {
                return match &self.target {
                    Some((view, player)) => Outcome::AddToPlaylist(*view, player.clone()),
                    None => Outcome::Nothing,
                };
            }
            Control::SpeedChoice(_) => self.close_speed_menu(ui),
            _ => {}
        }
        match self.command(control, config) {
            Some((view, command)) => Outcome::Command(view, command),
            None => Outcome::Nothing,
        }
    }

    /// The volume slider moved: the commands that set it (see [`volume_actions`]).
    pub fn volume_command(&mut self, value: f32) -> Option<(ViewId, Vec<Command>)> {
        let (view, player) = self.target.as_ref()?;
        self.volume_set_at = Some(Instant::now());
        let commands = volume_actions(player, value).into_iter().map(|a| Command::new(&player.id, a));
        Some((*view, commands.collect()))
    }

    /// The bridge command for a click on one of the controls.
    fn command(&self, control: Control, config: &Config) -> Option<(ViewId, Command)> {
        let (view, player) = self.target.as_ref()?;
        let action = match control {
            Control::PlayPause => Action::Toggle,
            Control::SkipBack => Action::Skip(-f64::from(config.media.skip_seconds)),
            Control::SkipForward => Action::Skip(f64::from(config.media.skip_seconds)),
            Control::Mute => Action::Mute(!player.muted),
            Control::Speed => Action::Rate(next_rate(&config.media.playback_speeds, player.rate)),
            Control::Captions => Action::Caption(next_caption(player)),
            Control::Fullscreen => Action::Fullscreen(self.mode != Mode::Fullscreen),
            Control::SpeedChoice(i) => Action::Rate(f64::from(*config.media.playback_speeds.get(i)?)),
            Control::PictureInPicture | Control::Playlist => return None,
        };
        Some((*view, Command::new(&player.id, action)))
    }

    /// A media key (only meaningful while fullscreen).
    pub fn key_command(&self, action: axion_config::Action, config: &Config) -> Option<(ViewId, Command)> {
        use axion_config::Action as K;
        let (view, player) = self.target.as_ref()?;
        let step = f64::from(config.media.volume_step);
        let speeds = &config.media.playback_speeds;
        let action = match action {
            K::MediaPlayPause => Action::Toggle,
            K::MediaSeekBack => Action::Skip(-f64::from(config.media.skip_seconds)),
            K::MediaSeekForward => Action::Skip(f64::from(config.media.skip_seconds)),
            K::MediaVolumeUp => Action::Volume((player.volume + step).min(1.0)),
            K::MediaVolumeDown => Action::Volume((player.volume - step).max(0.0)),
            K::MediaMute => Action::Mute(!player.muted),
            K::MediaFullscreen => Action::Fullscreen(self.mode != Mode::Fullscreen),
            K::MediaCaptions => Action::Caption(next_caption(player)),
            K::MediaSlower => Action::Rate(step_rate(speeds, player.rate, false)),
            K::MediaFaster => Action::Rate(step_rate(speeds, player.rate, true)),
            _ => return None,
        };
        Some((*view, Command::new(&player.id, action)))
    }

    /// In a fullscreen made by a site's own player, the video once asks to cover that player's
    /// UI so only Axion's controls show.
    pub fn takeover_command(&mut self) -> Option<(ViewId, Command)> {
        if self.mode != Mode::Fullscreen {
            self.takeover = None;
            return None;
        }
        let (view, player) = self.target.as_ref()?;
        if player.native || player.trusted || self.takeover.as_deref() == Some(player.id.as_str()) {
            return None;
        }
        self.takeover = Some(player.id.clone());
        Some((*view, Command::new(&player.id, Action::Takeover(true))))
    }

    /// The scrubber moved: seek there.
    pub fn seek_command(&mut self, value: f32) -> Option<(ViewId, Command)> {
        let (view, player) = self.target.as_ref()?;
        if player.duration <= 0.0 {
            return None;
        }
        self.scrubbed_at = Some(Instant::now());
        Some((*view, Command::new(&player.id, Action::Seek(f64::from(value) * player.duration))))
    }
}

/// What a volume slider at `value` (0–1) sends for `player`: the volume, and — as the slider
/// shows 0 while muted — taking it up from there also unmutes.
pub fn volume_actions(player: &Player, value: f32) -> Vec<Action> {
    let value = f64::from(value.clamp(0.0, 1.0));
    let mut actions = vec![Action::Volume(value)];
    if player.muted && value > 0.0 {
        actions.push(Action::Mute(false));
    }
    actions
}

/// 1 → "1×", 1.25 → "1.25×".
fn format_rate(rate: f64) -> String {
    let text = format!("{rate:.2}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
}

/// The speed after `rate` in the configured cycle.
fn next_rate(speeds: &[f32], rate: f64) -> f64 {
    let i = speeds.iter().position(|&s| (f64::from(s) - rate).abs() < 0.01);
    let next = i.map_or(0, |i| (i + 1) % speeds.len().max(1));
    speeds.get(next).map_or(1.0, |&s| f64::from(s))
}

/// The next faster (or slower) of the configured speeds.
fn step_rate(speeds: &[f32], rate: f64, faster: bool) -> f64 {
    let mut sorted: Vec<f64> = speeds.iter().map(|&s| f64::from(s)).collect();
    sorted.sort_by(f64::total_cmp);
    let pick = if faster {
        sorted.iter().find(|&&s| s > rate + 0.01)
    } else {
        sorted.iter().rev().find(|&&s| s < rate - 0.01)
    };
    pick.copied().unwrap_or(rate)
}

/// Off → first track → … → last → off.
fn next_caption(player: &Player) -> i32 {
    let pos = player.captions.iter().position(|c| c.index == player.caption);
    match pos {
        None => player.captions.first().map_or(-1, |c| c.index),
        Some(i) => player.captions.get(i + 1).map_or(-1, |c| c.index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axion_media::Caption;

    #[test]
    fn the_volume_slider_sets_the_volume_and_unmutes() {
        let mut p = Player { volume: 0.5, ..Player::default() };
        assert_eq!(volume_actions(&p, 0.3), vec![Action::Volume(f64::from(0.3_f32))]);
        // Muted, the slider shows 0: taking it up unmutes as well; 0 itself stays muted.
        p.muted = true;
        assert_eq!(volume_actions(&p, 0.3), vec![Action::Volume(f64::from(0.3_f32)), Action::Mute(false)]);
        assert_eq!(volume_actions(&p, 0.0), vec![Action::Volume(0.0)]);
        // Out of range is clamped.
        assert_eq!(volume_actions(&Player::default(), 4.0), vec![Action::Volume(1.0)]);
    }

    #[test]
    fn rates() {
        let speeds = [1.0, 1.25, 1.5, 2.0, 0.5, 0.75];
        assert_eq!(next_rate(&speeds, 1.0), 1.25);
        assert_eq!(next_rate(&speeds, 0.75), 1.0);
        assert_eq!(next_rate(&speeds, 3.0), 1.0);
        assert_eq!(step_rate(&speeds, 1.0, true), 1.25);
        assert_eq!(step_rate(&speeds, 0.5, false), 0.5);
        assert_eq!(format_rate(1.0), "1×");
        assert_eq!(format_rate(1.25), "1.25×");
        assert_eq!(format_rate(1.5), "1.5×");
    }

    #[test]
    fn captions_cycle_through_off() {
        let mut p = Player {
            captions: vec![
                Caption { index: 2, label: "en".into() },
                Caption { index: 5, label: "de".into() },
            ],
            caption: -1,
            ..Default::default()
        };
        assert_eq!(next_caption(&p), 2);
        p.caption = 2;
        assert_eq!(next_caption(&p), 5);
        p.caption = 5;
        assert_eq!(next_caption(&p), -1);
    }

    #[test]
    fn control_ids_round_trip() {
        for c in CONTROLS.into_iter().chain([Control::SpeedChoice(0), Control::SpeedChoice(5)]) {
            assert_eq!(Control::from_id(c.id()), Some(c));
        }
    }
}
