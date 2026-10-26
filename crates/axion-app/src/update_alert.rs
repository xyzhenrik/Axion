//! The updater's glass alert: asks before anything is changed ("Update Axion to 0.2.0?"),
//! then shows what happens (a progress bar for downloads, a log panel for the commands of a
//! source update) and ends with Relaunch or the reason it failed. One alert whose content is
//! rebuilt for every state (see [`AlertView`]); the texts and states come from `update.rs`.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

const BACKDROP: ActionId = ActionId(9200);
const PRIMARY: ActionId = ActionId(9201);
const SECONDARY: ActionId = ActionId(9202);

const WIDTH: f32 = 440.0;
const RADIUS: f32 = 24.0;
const START_SCALE: f32 = 0.88;
const BACKDROP_DIM: f32 = 0.28;
const ERROR: Color = Color::rgb(0xD0362B);
const ICON_SIZE: f32 = 30.0;
const BADGE: f32 = 56.0;
/// The log panel shows this many of the newest output lines, in a panel this tall at least
/// (so it does not grow line by line at the start).
const LOG_LINES: usize = 10;
const LOG_MIN_HEIGHT: f32 = 178.0;
const BAR_HEIGHT: f32 = 6.0;

/// A click in the alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertAction {
    Primary,
    Secondary,
    /// A click on the dimmed window around the alert.
    Backdrop,
}

/// How a button looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Plain,
}

/// What the alert shows now.
#[derive(Clone, Debug, Default)]
pub struct AlertView {
    pub icon: Option<Icon>,
    pub title: String,
    /// Paragraphs under the title.
    pub lines: Vec<String>,
    /// Output of the commands being run (the panel shows when `show_log`).
    pub log: Vec<String>,
    pub show_log: bool,
    /// A bar from 0 to 1, for a download of known size.
    pub progress: Option<f32>,
    /// A line under the bar or log ("Downloading… 12 of 45 MB").
    pub status: Option<String>,
    pub primary: Option<(String, Tone)>,
    pub secondary: Option<String>,
    /// The title is an error (red icon).
    pub error: bool,
    /// Escape and a click outside close the alert.
    pub dismissable: bool,
}

pub struct UpdateAlert {
    root: NodeId,
    alert: NodeId,
    open: bool,
    appear: Animated<f32>,
    view: AlertView,
    dirty: bool,
}

fn column_gap(gap: f32) -> Size<LengthPercentage> {
    Size { width: length(0.0), height: length(gap) }
}

fn button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, tone: Tone) {
    let background = match tone {
        Tone::Accent => Background::TintedGlass(GlassLevel::Thin, Paint::Accent),
        Tone::Plain => Background::Fill(Paint::Label, 0.08),
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            flex_basis: length(0.0),
            min_size: Size { width: length(0.0), height: length(40.0) },
            padding: Rect { left: length(12.0), right: length(12.0), top: length(0.0), bottom: length(0.0) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(node, text, TextStyle::BodyEmphasis, Style::default());
    ui.set_label_lines(label, 1);
    if tone == Tone::Accent
        && let Some(w) = ui.widget_mut(label)
    {
        w.set_paint(Paint::OnAccent);
    }
}

fn paragraph(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, Style::default());
    ui.set_label_lines(label, 6);
    if style == TextStyle::Footnote
        && let Some(w) = ui.widget_mut(label)
    {
        w.set_paint(Paint::Secondary);
    }
    label
}

impl UpdateAlert {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(BACKDROP),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let alert = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                max_size: Size { width: percent(0.94), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: column_gap(8.0),
                padding: Rect {
                    left: length(22.0),
                    right: length(22.0),
                    top: length(22.0),
                    bottom: length(20.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        Self {
            root,
            alert,
            open: false,
            appear: Animated::new(0.0, Spring::SNAPPY),
            view: AlertView::default(),
            dirty: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether Escape and clicks outside may close the alert now.
    pub fn is_dismissable(&self) -> bool {
        self.open && self.view.dismissable
    }

    pub fn action(&self, id: ActionId) -> Option<AlertAction> {
        match id {
            PRIMARY => Some(AlertAction::Primary),
            SECONDARY => Some(AlertAction::Secondary),
            BACKDROP => Some(AlertAction::Backdrop),
            _ => None,
        }
    }

    /// Whether `p` (logical px) is on the alert itself (the backdrop's action also fires for
    /// clicks on the alert's background).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.alert).contains(p)
    }

    /// Shows `view`: opens the alert, or replaces its content while it is open.
    pub fn show(&mut self, ui: &mut Ui, config: &Config, view: AlertView) {
        self.view = view;
        self.dirty = true;
        if !self.open {
            self.open = true;
            self.appear.spring = if config.glass.reduce_motion {
                Spring::new(0.15, 1.0)
            } else {
                Spring::new(
                    config.motion.snappy.response * config.motion.speed,
                    config.glass.damping(config.motion.snappy.damping),
                )
            };
            self.appear.snap(0.0);
            self.appear.target = 1.0;
            ui.set_visible(self.root, true);
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.appear.target = 0.0;
    }

    fn rebuild(&mut self, ui: &mut Ui) {
        self.dirty = false;
        let view = self.view.clone();
        ui.clear_children(self.alert);
        let alert = self.alert;
        if let Some(icon) = view.icon {
            let badge = ui.add(
                alert,
                Widget::container()
                    .with_background(Background::Fill(
                        if view.error { Paint::Fixed(ERROR) } else { Paint::Accent },
                        0.16,
                    ))
                    .with_radius(CAPSULE),
                Style {
                    size: Size { width: length(BADGE), height: length(BADGE) },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            );
            ui.add(
                badge,
                Widget::icon(icon, ICON_SIZE).with_paint(if view.error {
                    Paint::Fixed(ERROR)
                } else {
                    Paint::Accent
                }),
                Style::default(),
            );
        }
        paragraph(ui, alert, &view.title, TextStyle::BodyEmphasis);
        for line in &view.lines {
            paragraph(ui, alert, line, TextStyle::Footnote);
        }
        if view.show_log {
            let panel = ui.add(
                alert,
                Widget::container().with_background(Background::Fill(Paint::Label, 0.06)).with_radius(12.0),
                Style {
                    flex_direction: FlexDirection::Column,
                    min_size: Size { width: length(0.0), height: length(LOG_MIN_HEIGHT) },
                    justify_content: JustifyContent::FLEX_END,
                    padding: Rect {
                        left: length(12.0),
                        right: length(12.0),
                        top: length(10.0),
                        bottom: length(10.0),
                    },
                    margin: Rect {
                        left: length(0.0),
                        right: length(0.0),
                        top: length(4.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            let start = view.log.len().saturating_sub(LOG_LINES);
            for line in &view.log[start..] {
                let label = ui.add_label(
                    panel,
                    line,
                    TextStyle::Footnote,
                    Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
                );
                ui.set_label_lines(label, 1);
            }
        }
        if let Some(progress) = view.progress {
            let track = ui.add(
                alert,
                Widget::container()
                    .with_background(Background::Fill(Paint::Label, 0.10))
                    .with_radius(CAPSULE),
                Style {
                    size: Size { width: percent(1.0), height: length(BAR_HEIGHT) },
                    flex_shrink: 0.0,
                    margin: Rect {
                        left: length(0.0),
                        right: length(0.0),
                        top: length(4.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            ui.add(
                track,
                Widget::container()
                    .with_background(Background::Fill(Paint::Accent, 1.0))
                    .with_radius(CAPSULE),
                Style {
                    size: Size { width: percent(progress.clamp(0.0, 1.0)), height: length(BAR_HEIGHT) },
                    ..Default::default()
                },
            );
        }
        if let Some(status) = &view.status {
            paragraph(ui, alert, status, TextStyle::Footnote);
        }
        if view.primary.is_some() || view.secondary.is_some() {
            let buttons = ui.add(
                alert,
                Widget::container(),
                Style {
                    gap: Size { width: length(10.0), height: length(0.0) },
                    margin: Rect {
                        left: length(0.0),
                        right: length(0.0),
                        top: length(8.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            if let Some(text) = &view.secondary {
                button(ui, buttons, SECONDARY, text, Tone::Plain);
            }
            if let Some((text, tone)) = &view.primary {
                button(ui, buttons, PRIMARY, text, *tone);
            }
        }
    }

    /// Rebuilds the content after a change and advances the appear / disappear animation.
    /// Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
        }
        if self.dirty {
            self.rebuild(ui);
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if !self.open && k < 0.01 {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_opacity(self.root, k.min(1.0));
        let frame = ui.frame(self.alert);
        if frame.size.x > 0.0 {
            let scale = START_SCALE + (1.0 - START_SCALE) * k;
            let size = frame.size * scale;
            let origin = frame.origin + (frame.size - size) / 2.0;
            ui.set_visual_frame(self.alert, Some(Frame { origin, size: size.max(Vec2::ONE) }));
        }
    }
}
