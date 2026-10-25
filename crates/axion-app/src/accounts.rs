//! Accounts in the browser: the "Add Account" sheet, and switching by relaunching.
//!
//! Chromium allows one root cache path per process, so switching accounts (for now) saves the
//! session, shuts Chromium down and starts Axion again with `--account=ID` in the same place.
//! [`crate::browser::Browser::switch_account`] is the one entry point, so switching in-process
//! can replace this later.

use std::path::PathBuf;

use anyhow::{Context, Result};
use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

const CREATE: ActionId = ActionId(9100);
const CANCEL: ActionId = ActionId(9101);
const WIDTH: f32 = 340.0;
const RADIUS: f32 = 22.0;
/// The sheet grows from this size as it appears.
const START_SCALE: f32 = 0.88;
const BACKDROP_DIM: f32 = 0.28;
const ERROR: Color = Color::rgb(0xD0362B);

/// A click in the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetAction {
    Create,
    Cancel,
}

/// A small glass sheet asking for the new account's name (a password can be set later).
pub struct AddAccountSheet {
    root: NodeId,
    alert: NodeId,
    field: NodeId,
    error: NodeId,
    open: bool,
    appear: Animated<f32>,
}

fn capsule_button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) {
    let background =
        if primary { Background::Fill(Paint::Accent, 1.0) } else { Background::Fill(Paint::Label, 0.08) };
    let button = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            size: Size { width: auto(), height: length(40.0) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(button, text, TextStyle::BodyEmphasis, Style::default());
    if primary && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
    }
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str) {
    let label = ui.add_label(parent, text, TextStyle::Footnote, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
}

impl AddAccountSheet {
    pub fn build(ui: &mut Ui) -> Self {
        // The dimmed window behind it; a click there cancels.
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(CANCEL),
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
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(8.0) },
                padding: Rect {
                    left: length(20.0),
                    right: length(20.0),
                    top: length(20.0),
                    bottom: length(18.0),
                },
                ..Default::default()
            },
        );
        ui.add(alert, Widget::icon(Icon::PersonAdd, 30.0).with_paint(Paint::Accent), Style::default());
        ui.add_label(alert, "Add Account", TextStyle::BodyEmphasis, Style::default());
        secondary(ui, alert, "Its own tabs, history, passwords and settings.");
        secondary(ui, alert, "A password can be set later in Account Settings.");
        let field_box = ui.add(
            alert,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(12.0),
            Style {
                size: Size { width: percent(1.0), height: length(40.0) },
                margin: Rect { left: length(0.0), right: length(0.0), top: length(6.0), bottom: length(0.0) },
                padding: Rect {
                    left: length(12.0),
                    right: length(12.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let field = ui.add_text_field(
            field_box,
            "Name",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let error = ui.add_label(alert, "Enter a name.", TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(error) {
            w.set_paint(Paint::Fixed(ERROR));
        }
        ui.set_visible(error, false);
        let buttons = ui.add(
            alert,
            Widget::container(),
            Style {
                margin: Rect { left: length(0.0), right: length(0.0), top: length(8.0), bottom: length(0.0) },
                gap: Size { width: length(10.0), height: length(0.0) },
                ..Default::default()
            },
        );
        capsule_button(ui, buttons, CANCEL, "Cancel", false);
        capsule_button(ui, buttons, CREATE, "Add and Switch", true);
        ui.set_visible(root, false);
        Self { root, alert, field, error, open: false, appear: Animated::new(0.0, Spring::SNAPPY) }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn action(&self, id: ActionId) -> Option<SheetAction> {
        match id {
            CREATE => Some(SheetAction::Create),
            CANCEL => Some(SheetAction::Cancel),
            _ => None,
        }
    }

    /// Whether `p` (logical px) is on the sheet itself (the backdrop's cancel also fires for
    /// clicks on the sheet's background).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.alert).contains(p)
    }

    /// Opens with an empty name field focused; returns the focus events.
    pub fn open(&mut self, ui: &mut Ui, config: &Config) -> Vec<UiEvent> {
        self.open = true;
        self.appear.spring = if config.glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(config.motion.snappy.response * config.motion.speed, config.motion.snappy.damping)
        };
        self.appear.snap(0.0);
        self.appear.target = 1.0;
        ui.set_field_text(self.field, "");
        ui.set_visible(self.error, false);
        ui.set_visible(self.root, true);
        ui.focus(self.field)
    }

    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
        self.appear.target = 0.0;
        let _ = ui.blur();
    }

    /// The name typed, trimmed; shows an error while it is empty.
    pub fn name(&self, ui: &mut Ui) -> Option<String> {
        let name = ui.field_text(self.field).unwrap_or_default().trim().to_owned();
        ui.set_visible(self.error, name.is_empty());
        (!name.is_empty()).then_some(name)
    }

    /// Capture aid: types a name.
    pub fn set_name(&self, ui: &mut Ui, name: &str) {
        ui.set_field_text(self.field, name);
    }

    /// Advances the appear / disappear animation. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
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

/// Starting Axion again in another account, where this window was.
#[derive(Clone, Debug, PartialEq)]
pub struct Relaunch {
    pub account: String,
    /// The window's inner size (logical px).
    pub size: (f64, f64),
    /// The window's outer position (physical px), where the platform tells it (X11).
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    /// Capture aid: the relaunched window takes this screenshot.
    pub capture: Option<PathBuf>,
}

impl Relaunch {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![
            format!("--account={}", self.account),
            format!("--size={:.0}x{:.0}", self.size.0, self.size.1),
        ];
        if let Some((x, y)) = self.position {
            args.push(format!("--position={x},{y}"));
        }
        if self.maximized {
            args.push("--maximized".to_owned());
        }
        if let Some(path) = &self.capture {
            args.push(format!("--capture={}", path.display()));
        }
        args
    }

    /// Starts the new process (call once this one's Chromium has shut down). The environment
    /// (`AXION_MONITOR`, `AXION_PROFILE_DIR` …) carries over.
    pub fn spawn(&self) -> Result<()> {
        let exe = std::env::current_exe().context("finding Axion's executable")?;
        tracing::info!(account = %self.account, "starting Axion in the other account");
        std::process::Command::new(&exe)
            .args(self.args())
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_arguments() {
        let relaunch = Relaunch {
            account: "1a2b3c4d".into(),
            size: (1440.0, 900.0),
            position: Some((10, -20)),
            maximized: true,
            capture: Some(PathBuf::from("/tmp/x.png")),
        };
        assert_eq!(
            relaunch.args(),
            [
                "--account=1a2b3c4d",
                "--size=1440x900",
                "--position=10,-20",
                "--maximized",
                "--capture=/tmp/x.png"
            ]
        );
        let plain = Relaunch { position: None, maximized: false, capture: None, ..relaunch };
        assert_eq!(plain.args(), ["--account=1a2b3c4d", "--size=1440x900"]);
    }
}
