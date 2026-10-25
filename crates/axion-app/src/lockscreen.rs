//! Private browsing while locked (Safari, iPadOS 26): the page area shows a grey lock,
//! "“Private Browsing” is locked" and a blue Unlock button. Unlock brings up a glass alert
//! card (scaling up and fading in) that asks for the password — where Safari asks for Touch ID.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

const SHOW_ALERT: ActionId = ActionId(9000);
const UNLOCK: ActionId = ActionId(9001);
const CANCEL: ActionId = ActionId(9002);
const ALERT_WIDTH: f32 = 320.0;
const ALERT_RADIUS: f32 = 22.0;
/// The alert grows from this size as it appears.
const ALERT_START_SCALE: f32 = 0.88;
const ERROR: Color = Color::rgb(0xD0362B);

pub struct LockScreen {
    root: NodeId,
    alert_layer: NodeId,
    alert: NodeId,
    field: NodeId,
    error: NodeId,
    visible: bool,
    alert_open: bool,
    appear: Animated<f32>,
}

fn capsule_button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) -> NodeId {
    let background =
        if primary { Background::Fill(Paint::Accent, 1.0) } else { Background::Fill(Paint::Label, 0.08) };
    let button = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            size: Size { width: auto(), height: length(40.0) },
            padding: Rect { left: length(22.0), right: length(22.0), top: length(0.0), bottom: length(0.0) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(button, text, TextStyle::BodyEmphasis, Style::default());
    if primary && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
    }
    button
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

impl LockScreen {
    /// The Unlock button of the locked view (opens the alert).
    pub const SHOW_ALERT: ActionId = SHOW_ALERT;

    pub fn build(ui: &mut Ui) -> Self {
        // Covers the page completely: nothing of it shows or takes input while locked.
        let root = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Fill(Paint::Ground, 1.0)),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(0.0), height: length(14.0) },
                ..Default::default()
            },
        );
        ui.add(root, Widget::icon(Icon::Lock, 34.0).with_paint(Paint::Secondary), Style::default());
        secondary(ui, root, "“Private Browsing” is locked", TextStyle::Title2);
        let unlock = ui.add(
            root,
            Widget::container()
                .with_action(SHOW_ALERT)
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Accent, 1.0)),
            Style {
                size: Size { width: auto(), height: length(34.0) },
                padding: Rect {
                    left: length(18.0),
                    right: length(18.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let label = ui.add_label(unlock, "Unlock", TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
        }

        // The alert, centred over the locked view.
        let alert_layer = ui.add(
            root,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let alert = ui.add(
            alert_layer,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_radius(ALERT_RADIUS),
            Style {
                size: Size { width: length(ALERT_WIDTH), height: auto() },
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
        ui.add(alert, Widget::icon(Icon::Lock, 30.0).with_paint(Paint::Accent), Style::default());
        ui.add_label(alert, "Unlock “Private Browsing”", TextStyle::BodyEmphasis, Style::default());
        secondary(ui, alert, "Enter your password to see your private tabs.", TextStyle::Footnote);
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
            "Password",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        ui.set_secure(field, true);
        let error = ui.add_label(alert, "Wrong password", TextStyle::Footnote, Style::default());
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
        capsule_button(ui, buttons, UNLOCK, "Unlock", true);
        ui.set_visible(alert_layer, false);
        ui.set_visible(root, false);
        Self {
            root,
            alert_layer,
            alert,
            field,
            error,
            visible: false,
            alert_open: false,
            appear: Animated::new(0.0, Spring::SNAPPY),
        }
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn is_unlock(action: ActionId) -> bool {
        action == UNLOCK
    }

    pub fn is_lock_action(action: ActionId) -> bool {
        [SHOW_ALERT, UNLOCK, CANCEL].contains(&action)
    }

    pub fn is_alert_open(&self) -> bool {
        self.alert_open
    }

    /// Shows the locked view in `page` (logical px, with the page's corner radius), or hides it.
    pub fn show(&mut self, ui: &mut Ui, page: Option<Frame>, radius: f32) {
        match page {
            Some(page) => {
                if let Some(w) = ui.widget_mut(self.root) {
                    w.radius = radius;
                }
                let mut style = ui.style(self.root);
                style.inset = Rect {
                    left: length(page.origin.x),
                    right: auto(),
                    top: length(page.origin.y),
                    bottom: auto(),
                };
                style.size = Size { width: length(page.size.x), height: length(page.size.y) };
                ui.set_style(self.root, style);
                self.visible = true;
                ui.set_visible(self.root, true);
            }
            None => {
                self.visible = false;
                self.alert_open = false;
                self.appear.snap(0.0);
                ui.set_visible(self.root, false);
                ui.set_visible(self.alert_layer, false);
                ui.set_visible(self.error, false);
                ui.set_field_text(self.field, "");
            }
        }
    }

    /// A click on the locked view's buttons. Returns the events of focusing the password field.
    pub fn action(&mut self, ui: &mut Ui, config: &Config, action: ActionId) -> Vec<UiEvent> {
        match action {
            SHOW_ALERT => self.open_alert(ui, config),
            CANCEL => {
                self.close_alert(ui);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn open_alert(&mut self, ui: &mut Ui, config: &Config) -> Vec<UiEvent> {
        self.alert_open = true;
        self.appear.spring = if config.glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(config.motion.snappy.response * config.motion.speed, config.motion.snappy.damping)
        };
        self.appear.target = 1.0;
        ui.set_visible(self.alert_layer, true);
        ui.set_visible(self.error, false);
        ui.focus(self.field)
    }

    pub fn close_alert(&mut self, ui: &mut Ui) {
        self.alert_open = false;
        self.appear.target = 0.0;
        ui.set_field_text(self.field, "");
        ui.set_visible(self.error, false);
    }

    /// Advances the alert's appear / disappear animation. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !self.visible {
            return;
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if !self.alert_open && k < 0.01 {
            ui.set_visible(self.alert_layer, false);
            return;
        }
        ui.set_opacity(self.alert_layer, k.min(1.0));
        let frame = ui.frame(self.alert);
        if frame.size.x > 0.0 {
            let scale = ALERT_START_SCALE + (1.0 - ALERT_START_SCALE) * k;
            let size = frame.size * scale;
            let origin = frame.origin + (frame.size - size) / 2.0;
            ui.set_visual_frame(self.alert, Some(Frame { origin, size: size.max(Vec2::ONE) }));
        }
    }

    /// The password typed so far.
    pub fn password(&self, ui: &Ui) -> String {
        ui.field_text(self.field).unwrap_or_default().to_owned()
    }

    pub fn wrong_password(&mut self, ui: &mut Ui) {
        ui.set_visible(self.error, true);
        ui.set_field_text(self.field, "");
    }
}
