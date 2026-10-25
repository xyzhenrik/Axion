//! The card shown over the page area while private browsing is locked.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Color;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};

const UNLOCK: ActionId = ActionId(9000);
const CARD_WIDTH: f32 = 380.0;
const RADIUS: f32 = 28.0;
const ERROR: Color = Color::rgb(0xD0362B);

pub struct LockScreen {
    root: NodeId,
    field: NodeId,
    error: NodeId,
    visible: bool,
}

impl LockScreen {
    pub fn build(ui: &mut Ui) -> Self {
        // Covers the page completely: nothing of it shows or takes input while locked.
        let root = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Fill(Paint::Ground, 1.0)),
            Style {
                position: Position::Absolute,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let card = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                size: Size { width: length(CARD_WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(0.0), height: length(10.0) },
                padding: Rect {
                    left: length(28.0),
                    right: length(28.0),
                    top: length(30.0),
                    bottom: length(26.0),
                },
                ..Default::default()
            },
        );
        ui.add(card, Widget::icon(Icon::Lock, 40.0).with_paint(Paint::Accent), Style::default());
        ui.add_label(card, "Private Browsing is Locked", TextStyle::Title2, Style::default());
        let hint = ui.add_label(
            card,
            "Enter your password to see your private tabs.",
            TextStyle::Footnote,
            Style::default(),
        );
        if let Some(w) = ui.widget_mut(hint) {
            w.set_paint(Paint::Secondary);
        }
        let field_box = ui.add(
            card,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(14.0),
            Style {
                size: Size { width: percent(1.0), height: length(44.0) },
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(10.0),
                    bottom: length(0.0),
                },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
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
        let error = ui.add_label(card, "Wrong password", TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(error) {
            w.set_paint(Paint::Fixed(ERROR));
        }
        ui.set_visible(error, false);
        let button = ui.add(
            card,
            Widget::container()
                .with_action(UNLOCK)
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Accent, 1.0)),
            Style {
                size: Size { width: percent(1.0), height: length(44.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let label = ui.add_label(button, "Unlock", TextStyle::BodyEmphasis, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
        }
        ui.set_visible(root, false);
        Self { root, field, error, visible: false }
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn is_unlock(action: ActionId) -> bool {
        action == UNLOCK
    }

    /// Shows the card centred in `page` (logical px, with the page's corner radius), or hides it.
    pub fn show(&mut self, ui: &mut Ui, page: Option<Frame>, radius: f32) -> Vec<UiEvent> {
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
                let was_visible = std::mem::replace(&mut self.visible, true);
                ui.set_visible(self.root, true);
                if was_visible { Vec::new() } else { ui.focus(self.field) }
            }
            None => {
                self.visible = false;
                ui.set_visible(self.root, false);
                ui.set_visible(self.error, false);
                ui.set_field_text(self.field, "");
                Vec::new()
            }
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
