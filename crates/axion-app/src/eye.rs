//! The eye button of password fields: shows or hides what is typed (Safari / iOS style).
//!
//! One shared helper so every password field looks and behaves the same: [`add`] puts the eye at
//! the end of a field's box and makes the field secure; the browser routes clicks on it to
//! [`toggle`] (see `Browser::handle_ui_events`), which keeps the keyboard focus in the field.
//! Fields built afresh start hidden; [`hide`] / [`hide_all`] hide them again (a sheet that
//! reopens, a step that changes).

use std::cell::RefCell;

use axion_compositor::Icon;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, CAPSULE, Paint, Ui, Widget};

/// Action ids of the eyes: a small range of their own.
const BASE: u32 = 13_000;
const COUNT: u32 = 256;
const SIDE: f32 = 28.0;
const ICON: f32 = 18.0;

struct Eye {
    id: u32,
    field: NodeId,
    icon: NodeId,
    shown: bool,
}

thread_local! {
    static EYES: RefCell<Vec<Eye>> = const { RefCell::new(Vec::new()) };
}

/// Makes `field` (inside `holder`, a row) a password field with an eye at the row's end.
pub fn add(ui: &mut Ui, holder: NodeId, field: NodeId) {
    ui.set_secure(field, true);
    EYES.with_borrow_mut(|eyes| {
        // Eyes of widgets that are gone (rebuilt pages) free their ids.
        eyes.retain(|e| ui.widget(e.field).is_some());
        let Some(id) = (0..COUNT).find(|i| eyes.iter().all(|e| e.id != *i)) else { return };
        let button = ui.add(
            holder,
            Widget::container().with_action(ActionId(BASE + id)).with_radius(CAPSULE),
            Style {
                size: Size { width: length(SIDE), height: length(SIDE) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let icon =
            ui.add(button, Widget::icon(Icon::Eye, ICON).with_paint(Paint::Secondary), Style::default());
        eyes.push(Eye { id, field, icon, shown: false });
    });
}

/// Whether `action` is an eye.
pub fn owns(action: ActionId) -> bool {
    action.0 >= BASE && action.0 < BASE + COUNT
}

/// A click on the eye `action`: shows or hides its field. Returns the field, to keep the focus there.
pub fn toggle(ui: &mut Ui, action: ActionId) -> Option<NodeId> {
    EYES.with_borrow_mut(|eyes| {
        let eye = eyes.iter_mut().find(|e| BASE + e.id == action.0)?;
        eye.shown = !eye.shown;
        apply(ui, eye);
        Some(eye.field)
    })
}

/// Hides what `field` shows (nothing if it has no eye).
pub fn hide(ui: &mut Ui, field: NodeId) {
    EYES.with_borrow_mut(|eyes| {
        if let Some(eye) = eyes.iter_mut().find(|e| e.field == field) {
            eye.shown = false;
            apply(ui, eye);
        }
    });
}

fn apply(ui: &mut Ui, eye: &Eye) {
    ui.set_secure(eye.field, !eye.shown);
    ui.set_icon(eye.icon, if eye.shown { Icon::EyeOff } else { Icon::Eye });
}
