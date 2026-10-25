//! Axion's own media controls (design: "In-page player", "Fullscreen player"): a dark glass
//! capsule over the page's video, driven through the media bridge.
//!
//! Inline, they replace the native controls of `<video controls>` elements and show while the
//! video is paused or the pointer is over it. In fullscreen they sit over every video, also
//! ones with a site's own player, and hide after a moment without pointer movement.

use std::time::{Duration, Instant};

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_engine::ViewId;
use axion_media::{Action, Command, Kind, PageMedia, Player, SessionInfo, format_time};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

const ACTION_BASE: u32 = 9500;
/// Smaller videos keep their page's look; controls wouldn't fit.
const MIN_SIZE: Vec2 = Vec2::new(220.0, 124.0);
/// Below this width the time labels and speed button are left out.
const COMPACT_WIDTH: f32 = 440.0;
/// After scrubbing, page reports still carry the old time for a moment; ignore them.
const SCRUB_SETTLE: Duration = Duration::from_millis(600);
const SETTLED: f32 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    SkipBack,
    SkipForward,
    Mute,
    Speed,
    Captions,
    Fullscreen,
}

const CONTROLS: [Control; 7] = [
    Control::PlayPause,
    Control::SkipBack,
    Control::SkipForward,
    Control::Mute,
    Control::Speed,
    Control::Captions,
    Control::Fullscreen,
];

impl Control {
    fn id(self) -> ActionId {
        ActionId(ACTION_BASE + CONTROLS.iter().position(|&c| c == self).unwrap_or(0) as u32)
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        CONTROLS.get(id.0.checked_sub(ACTION_BASE)? as usize).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Hidden,
    Inline,
    Fullscreen,
}

/// What the overlay needs to know each frame.
pub struct Context<'a> {
    pub view: Option<ViewId>,
    pub media: Option<&'a PageMedia>,
    /// The page area in logical pixels.
    pub page: Frame,
    /// Logical → physical pixels.
    pub scale: f32,
    pub pointer: Option<Vec2>,
    /// The page made something fullscreen.
    pub page_fullscreen: bool,
    /// Shown in fullscreen when the page gives no media session title.
    pub page_title: &'a str,
    pub host: &'a str,
}

pub struct PlayerOverlay {
    root: NodeId,
    title_pill: NodeId,
    title: NodeId,
    subtitle: NodeId,
    corner: NodeId,
    bar: NodeId,
    skip_back: NodeId,
    skip_forward: NodeId,
    play_icon: NodeId,
    time: NodeId,
    scrubber: NodeId,
    remaining: NodeId,
    mute_icon: NodeId,
    speed: NodeId,
    speed_label: NodeId,
    captions: NodeId,
    /// Exit fullscreen, in the bar (fullscreen only).
    exit: NodeId,
    fullscreen_icon: NodeId,
    opacity: Animated<f32>,
    mode: Mode,
    /// The layout the nodes have now: mode and whether it's the compact (narrow) one.
    layout: Option<(Mode, bool)>,
    /// The view and player the controls are about.
    target: Option<(ViewId, Player)>,
    last_pointer: Option<Vec2>,
    last_activity: Instant,
    scrubbed_at: Option<Instant>,
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

fn button(
    ui: &mut Ui,
    parent: NodeId,
    control: Control,
    icon: Icon,
    side: f32,
    filled: bool,
) -> (NodeId, NodeId) {
    let mut widget = Widget::container().with_action(control.id()).with_radius(CAPSULE);
    if filled {
        widget = widget.with_background(Background::Fill(Paint::Label, 0.18));
    }
    let node = ui.add(
        parent,
        widget,
        Style { size: square(side), flex_shrink: 0.0, justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let glyph = ui.add(node, Widget::icon(icon, side * 0.5).with_paint(Paint::Label), Style::default());
    (node, glyph)
}

fn label(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    ui.add_label(parent, text, style, Style { flex_shrink: 0.0, ..Default::default() })
}

impl PlayerOverlay {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container(),
            Style { position: Position::Absolute, ..Default::default() },
        );
        ui.set_plane_dark(root, true);

        let title_pill = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(32.0), top: length(28.0), right: auto(), bottom: auto() },
                size: Size { width: auto(), height: length(52.0) },
                max_size: Size { width: percent(0.6), height: auto() },
                padding: Rect {
                    left: length(20.0),
                    right: length(20.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::CENTER,
                ..Default::default()
            },
        );
        let title = ui.add_label(title_pill, "", TextStyle::BodyEmphasis, Style::default());
        let subtitle = ui.add_label(title_pill, "", TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(subtitle) {
            w.set_paint(Paint::Secondary);
        }

        let corner = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                inset: Rect { right: length(18.0), top: length(18.0), left: auto(), bottom: auto() },
                padding: Rect {
                    left: length(4.0),
                    right: length(4.0),
                    top: length(4.0),
                    bottom: length(4.0),
                },
                ..row(2.0)
            },
        );
        let (_, fullscreen_icon) = button(ui, corner, Control::Fullscreen, Icon::Fullscreen, 38.0, false);

        // Thick glass: the controls must stay legible over any video.
        let bar = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                padding: Rect {
                    left: length(10.0),
                    right: length(10.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..row(4.0)
            },
        );
        let (skip_back, _) = button(ui, bar, Control::SkipBack, Icon::SkipBack, 48.0, false);
        let (_, play_icon) = button(ui, bar, Control::PlayPause, Icon::Pause, 42.0, true);
        let (skip_forward, _) = button(ui, bar, Control::SkipForward, Icon::SkipForward, 48.0, false);
        let time = label(ui, bar, "0:00", TextStyle::Footnote);
        let scrubber = ui.add(
            bar,
            Widget::slider(0.0),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(40.0), height: auto() },
                margin: Rect { left: length(8.0), right: length(8.0), top: length(0.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        let remaining = label(ui, bar, "-0:00", TextStyle::Footnote);
        let (_, mute_icon) = button(ui, bar, Control::Mute, Icon::Volume, 42.0, false);
        let speed = ui.add(
            bar,
            Widget::container().with_action(Control::Speed.id()).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(36.0) },
                min_size: Size { width: length(42.0), height: auto() },
                padding: Rect {
                    left: length(8.0),
                    right: length(8.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                justify_content: JustifyContent::CENTER,
                flex_shrink: 0.0,
                ..row(0.0)
            },
        );
        let speed_label = ui.add_label(speed, "1×", TextStyle::Callout, Style::default());
        let (captions, _) = button(ui, bar, Control::Captions, Icon::Captions, 42.0, false);
        let (exit, _) = button(ui, bar, Control::Fullscreen, Icon::ExitFullscreen, 42.0, false);
        ui.set_visible(root, false);
        Self {
            root,
            title_pill,
            title,
            subtitle,
            corner,
            bar,
            skip_back,
            skip_forward,
            play_icon,
            time,
            scrubber,
            remaining,
            mute_icon,
            speed,
            speed_label,
            captions,
            exit,
            fullscreen_icon,
            opacity: Animated::new(0.0, Spring::SNAPPY),
            mode: Mode::Hidden,
            layout: None,
            target: None,
            last_pointer: None,
            last_activity: Instant::now(),
            scrubbed_at: None,
            takeover: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The scrubber node, whose value changes seek.
    pub fn scrubber(&self) -> NodeId {
        self.scrubber
    }

    /// The controls are showing (fullscreen ones may have hidden themselves).
    pub fn is_shown(&self) -> bool {
        self.mode != Mode::Hidden && self.opacity.target > 0.5
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
            self.fade(ui, false, dt);
            return;
        };
        let layout = (mode, mode != Mode::Fullscreen && rect.size.x < COMPACT_WIDTH);
        if self.layout != Some(layout) {
            self.layout = Some(layout);
            self.apply_layout(ui, layout.0, layout.1);
        }
        self.place(ui, rect);
        self.show_state(ui, &player, cx);
        let pointer_inside = cx.pointer.is_some_and(|p| rect.contains(p));
        let recently_active =
            now.duration_since(self.last_activity).as_secs_f32() < config.media.hide_controls_after_seconds;
        let scrubbing = ui.dragging() == Some(self.scrubber);
        let visible = scrubbing
            || !player.is_playing()
            || match mode {
                Mode::Fullscreen => recently_active,
                _ => pointer_inside && recently_active,
            };
        self.target = Some((view, player));
        self.fade(ui, visible, dt);
    }

    fn choose(&self, config: &Config, cx: &Context<'_>) -> (Mode, Option<Player>, Option<Frame>) {
        let Some(media) = cx.media else { return (Mode::Hidden, None, None) };
        if cx.page_fullscreen {
            if !config.media.own_fullscreen_controls {
                return (Mode::Hidden, None, None);
            }
            let player = media
                .players
                .iter()
                .filter(|p| p.kind == Kind::Video && p.fullscreen)
                .max_by(|a, b| a.area().total_cmp(&b.area()));
            return (Mode::Fullscreen, player.cloned(), Some(cx.page));
        }
        if !config.media.own_controls {
            return (Mode::Hidden, None, None);
        }
        let dpr = if media.dpr > 0.0 { media.dpr } else { cx.scale };
        let to_logical = |p: &Player| {
            let k = dpr / cx.scale;
            Frame::new(cx.page.origin.x + p.x * k, cx.page.origin.y + p.y * k, p.w * k, p.h * k)
        };
        let candidates: Vec<(&Player, Frame)> = media
            .players
            .iter()
            .filter(|p| p.kind == Kind::Video && p.native)
            .map(|p| (p, clip(to_logical(p), cx.page)))
            .filter(|(_, r)| r.size.x >= MIN_SIZE.x && r.size.y >= MIN_SIZE.y)
            .collect();
        let under_pointer = cx.pointer.and_then(|p| candidates.iter().find(|(_, r)| r.contains(p)));
        let chosen = under_pointer
            .or_else(|| candidates.iter().find(|(p, _)| p.is_playing()))
            .or_else(|| candidates.first());
        match chosen {
            Some((p, r)) => (Mode::Inline, Some((*p).clone()), Some(*r)),
            None => (Mode::Hidden, None, None),
        }
    }

    fn apply_layout(&self, ui: &mut Ui, mode: Mode, compact: bool) {
        let fullscreen = mode == Mode::Fullscreen;
        let (side, bottom, height) = if fullscreen { (64.0, 40.0, 72.0) } else { (18.0, 18.0, 58.0) };
        let mut style = ui.style(self.bar);
        style.inset = Rect { left: length(side), right: length(side), bottom: length(bottom), top: auto() };
        style.size = Size { width: auto(), height: length(height) };
        ui.set_style(self.bar, style);
        ui.set_visible(self.title_pill, fullscreen);
        ui.set_visible(self.corner, !fullscreen);
        ui.set_visible(self.skip_back, fullscreen);
        ui.set_visible(self.skip_forward, fullscreen);
        ui.set_visible(self.exit, fullscreen);
        ui.set_visible(self.time, !compact);
        ui.set_visible(self.remaining, !compact);
        ui.set_visible(self.speed, !compact);
        ui.set_icon(self.fullscreen_icon, if fullscreen { Icon::ExitFullscreen } else { Icon::Fullscreen });
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

    fn show_state(&mut self, ui: &mut Ui, player: &Player, cx: &Context<'_>) {
        ui.set_icon(self.play_icon, if player.is_playing() { Icon::Pause } else { Icon::Play });
        ui.set_icon(
            self.mute_icon,
            if player.muted || player.volume == 0.0 { Icon::Muted } else { Icon::Volume },
        );
        ui.set_text(self.time, &format_time(player.time));
        let remaining = if player.live {
            "Live".to_owned()
        } else {
            format!("-{}", format_time(player.duration - player.time))
        };
        ui.set_text(self.remaining, &remaining);
        ui.set_text(self.speed_label, &format_rate(player.rate));
        ui.set_visible(self.captions, !player.captions.is_empty());
        let scrubbing = ui.dragging() == Some(self.scrubber)
            || self.scrubbed_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
        if !scrubbing && player.duration > 0.0 {
            ui.set_value(self.scrubber, (player.time / player.duration) as f32);
        }
        if self.mode == Mode::Fullscreen {
            let session = cx.media.and_then(|m| m.session.clone()).unwrap_or_default();
            let SessionInfo { title, artist, .. } = session;
            let title = if title.is_empty() { cx.page_title.to_owned() } else { title };
            let subtitle =
                if artist.is_empty() { cx.host.to_owned() } else { format!("{artist} · {}", cx.host) };
            ui.set_text(self.title, &title);
            ui.set_text(self.subtitle, &subtitle);
        }
    }

    fn fade(&mut self, ui: &mut Ui, visible: bool, dt: f32) {
        self.opacity.target = if visible { 1.0 } else { 0.0 };
        self.opacity.step(dt);
        let shown = self.opacity.value > SETTLED || visible;
        ui.set_visible(self.root, shown);
        ui.set_opacity(self.root, self.opacity.value.clamp(0.0, 1.0));
    }

    /// The bridge command for a click on one of the controls.
    pub fn command(&self, control: Control, config: &Config) -> Option<(ViewId, Command)> {
        let (view, player) = self.target.as_ref()?;
        let action = match control {
            Control::PlayPause => Action::Toggle,
            Control::SkipBack => Action::Skip(-f64::from(config.media.skip_seconds)),
            Control::SkipForward => Action::Skip(f64::from(config.media.skip_seconds)),
            Control::Mute => Action::Mute(!player.muted),
            Control::Speed => Action::Rate(next_rate(&config.media.playback_speeds, player.rate)),
            Control::Captions => Action::Caption(next_caption(player)),
            Control::Fullscreen => Action::Fullscreen(self.mode != Mode::Fullscreen),
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
        if player.native || self.takeover.as_deref() == Some(player.id.as_str()) {
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

/// The part of `r` inside `bounds`.
fn clip(r: Frame, bounds: Frame) -> Frame {
    let min = r.origin.max(bounds.origin);
    let max = (r.origin + r.size).min(bounds.origin + bounds.size);
    Frame { origin: min, size: (max - min).max(Vec2::ZERO) }
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
        for c in CONTROLS {
            assert_eq!(Control::from_id(c.id()), Some(c));
        }
    }
}
