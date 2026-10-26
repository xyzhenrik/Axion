//! Private browsing while locked (Safari, iPadOS 26): the page area shows a grey lock,
//! “Private Browsing” is locked and a blue Unlock button. Unlock brings up a glass alert
//! card (scaling up and fading in) that asks for the account password — where Safari asks for
//! Touch ID.
//!
//! Two planes: the cover, an opaque card over the page (above the chrome and its popovers, so
//! nothing of a locked page shows), and the alert, the topmost plane of the window. The alert
//! also comes up on its own (no cover) when something needs the private tabs unlocked from
//! the normal tabs (a tab dragged into the private ones).

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::{tr, tr_fmt};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

const SHOW_ALERT: ActionId = ActionId(9000);
const UNLOCK: ActionId = ActionId(9001);
const CANCEL: ActionId = ActionId(9002);
const REVEAL: ActionId = ActionId(9003);
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
    /// The eye in the field: shows or hides the typed password.
    eye_icon: NodeId,
    revealed: bool,
    /// The alert's line that names the account.
    text: NodeId,
    error: NodeId,
    /// The cover is up (private tabs shown and locked).
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

/// Places a plane over `frame` (logical px).
fn place(ui: &mut Ui, node: NodeId, frame: Frame) {
    let mut style = ui.style(node);
    style.inset =
        Rect { left: length(frame.origin.x), right: auto(), top: length(frame.origin.y), bottom: auto() };
    style.size = Size { width: length(frame.size.x), height: length(frame.size.y) };
    ui.set_style(node, style);
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
        secondary(ui, root, tr("“Private Browsing” is locked"), TextStyle::Title2);
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
        let label = ui.add_label(unlock, tr("Unlock"), TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
        }

        // The alert, centred over the page: its own plane (see `planes`).
        let alert_layer = ui.add(
            ui.root(),
            Widget::container(),
            Style {
                position: Position::Absolute,
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
        ui.add_label(alert, tr("Unlock “Private Browsing”"), TextStyle::BodyEmphasis, Style::default());
        let text = secondary(ui, alert, "", TextStyle::Footnote);
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
            tr("Password"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        ui.set_secure(field, true);
        // The eye (Safari/iOS style): lets the user check what they typed.
        let eye = ui.add(
            field_box,
            Widget::container().with_action(REVEAL).with_radius(CAPSULE),
            Style {
                size: Size { width: length(28.0), height: length(28.0) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let eye_icon =
            ui.add(eye, Widget::icon(Icon::Eye, 18.0).with_paint(Paint::Secondary), Style::default());
        let error = ui.add_label(alert, tr("Wrong password"), TextStyle::Footnote, Style::default());
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
        capsule_button(ui, buttons, CANCEL, tr("Cancel"), false);
        capsule_button(ui, buttons, UNLOCK, tr("Unlock"), true);
        ui.set_visible(alert_layer, false);
        ui.set_visible(root, false);
        Self {
            root,
            alert_layer,
            alert,
            field,
            eye_icon,
            revealed: false,
            text,
            error,
            visible: false,
            alert_open: false,
            appear: Animated::new(0.0, Spring::SNAPPY),
        }
    }

    /// The cover and the alert plane, for the window to put the cover above the chrome and
    /// the alert above everything.
    pub fn planes(&self) -> (NodeId, NodeId) {
        (self.root, self.alert_layer)
    }

    /// Names the account whose password unlocks.
    pub fn set_account(&self, ui: &mut Ui, name: &str) {
        let text = tr_fmt!("Enter the password of “{name}”.", name = name);
        ui.set_text(self.text, &text);
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn is_unlock(action: ActionId) -> bool {
        action == UNLOCK
    }

    pub fn is_cancel(action: ActionId) -> bool {
        action == CANCEL
    }

    pub fn is_lock_action(action: ActionId) -> bool {
        [SHOW_ALERT, UNLOCK, CANCEL, REVEAL].contains(&action)
    }

    pub fn is_alert_open(&self) -> bool {
        self.alert_open
    }

    /// Shows the locked view in `page` (logical px, with the page's corner radius), or hides it
    /// (and its alert).
    pub fn show(&mut self, ui: &mut Ui, page: Option<Frame>, radius: f32) {
        match page {
            Some(page) => {
                if let Some(w) = ui.widget_mut(self.root) {
                    w.radius = radius;
                }
                place(ui, self.root, page);
                place(ui, self.alert_layer, page);
                self.visible = true;
                ui.set_visible(self.root, true);
            }
            None => {
                self.visible = false;
                self.hide_alert(ui);
                ui.set_visible(self.root, false);
            }
        }
    }

    /// The alert alone, centred in `page` (unlocking from the normal tabs).
    pub fn show_alert_over(&mut self, ui: &mut Ui, config: &Config, page: Frame) -> Vec<UiEvent> {
        place(ui, self.alert_layer, page);
        self.open_alert(ui, config)
    }

    /// A click on the locked view's buttons. Returns the events of focusing the password field.
    pub fn action(&mut self, ui: &mut Ui, config: &Config, action: ActionId) -> Vec<UiEvent> {
        match action {
            SHOW_ALERT => self.open_alert(ui, config),
            CANCEL => {
                self.close_alert(ui);
                Vec::new()
            }
            REVEAL => {
                self.set_revealed(ui, !self.revealed);
                // Keep typing: the click didn't take the focus away.
                ui.focus(self.field)
            }
            _ => Vec::new(),
        }
    }

    fn set_revealed(&mut self, ui: &mut Ui, on: bool) {
        self.revealed = on;
        ui.set_secure(self.field, !on);
        ui.set_icon(self.eye_icon, if on { Icon::EyeOff } else { Icon::Eye });
    }

    /// Whether `p` (logical px) is on the alert card.
    pub fn alert_contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.alert_open && ui.frame(self.alert).contains(p)
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
        self.set_revealed(ui, false);
        self.appear.target = 0.0;
        ui.set_field_text(self.field, "");
        ui.set_visible(self.error, false);
        if !self.visible {
            // On its own it goes at once (nothing to fade back to).
            self.hide_alert(ui);
        }
    }

    fn hide_alert(&mut self, ui: &mut Ui) {
        self.alert_open = false;
        self.set_revealed(ui, false);
        self.appear.snap(0.0);
        ui.set_visible(self.alert_layer, false);
        ui.set_visible(self.error, false);
        ui.set_field_text(self.field, "");
    }

    /// Advances the alert's appear / disappear animation. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !ui.is_visible(self.alert_layer) {
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
        self.show_error(ui, tr("Wrong password"));
    }

    /// Shows why unlocking failed (and clears the field).
    pub fn show_error(&mut self, ui: &mut Ui, message: &str) {
        ui.set_text(self.error, message);
        ui.set_visible(self.error, true);
        ui.set_field_text(self.field, "");
    }
}
