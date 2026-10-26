//! Engine-neutral input → CEF browser-host events.

use cef::sys::{cef_event_flags_t, cef_key_event_type_t, cef_mouse_button_type_t};
use cef::*;

use crate::{InputEvent, Modifiers, MouseButton};

/// Tracks held mouse buttons, which CEF expects as flags on every mouse event (for drags).
#[derive(Default)]
pub(super) struct InputState {
    buttons: u32,
}

impl InputState {
    /// Forgets a held button (its release went somewhere else, e.g. ended a drag).
    pub fn release(&mut self, button: MouseButton) {
        self.buttons &= !button_flag(button).0;
    }

    pub fn send(&mut self, host: &BrowserHost, event: &InputEvent) {
        match event {
            InputEvent::MouseMove { pos, modifiers } => {
                host.send_mouse_move_event(Some(&self.mouse(pos.x, pos.y, *modifiers)), 0);
            }
            InputEvent::MouseLeave { modifiers } => {
                host.send_mouse_move_event(Some(&self.mouse(-1.0, -1.0, *modifiers)), 1);
            }
            InputEvent::MouseButton { pos, button, pressed, click_count, modifiers } => {
                let kind = match button {
                    MouseButton::Left => cef_mouse_button_type_t::MBT_LEFT,
                    MouseButton::Middle => cef_mouse_button_type_t::MBT_MIDDLE,
                    MouseButton::Right => cef_mouse_button_type_t::MBT_RIGHT,
                };
                let flag = button_flag(*button);
                if *pressed {
                    self.buttons |= flag.0;
                } else {
                    self.buttons &= !flag.0;
                }
                let event = self.mouse(pos.x, pos.y, *modifiers);
                host.send_mouse_click_event(Some(&event), kind.into(), (!pressed) as _, *click_count as _);
            }
            InputEvent::Wheel { pos, delta, modifiers } => {
                let event = self.mouse(pos.x, pos.y, *modifiers);
                host.send_mouse_wheel_event(Some(&event), delta.x.round() as _, delta.y.round() as _);
            }
            InputEvent::Key { pressed, key_code, native_code, modifiers } => {
                let kind = if *pressed {
                    cef_key_event_type_t::KEYEVENT_RAWKEYDOWN
                } else {
                    cef_key_event_type_t::KEYEVENT_KEYUP
                };
                host.send_key_event(Some(&KeyEvent {
                    type_: kind.into(),
                    modifiers: self.flags(*modifiers),
                    windows_key_code: *key_code,
                    native_key_code: *native_code,
                    ..Default::default()
                }));
            }
            InputEvent::Text { text, modifiers } => {
                for unit in text.encode_utf16() {
                    host.send_key_event(Some(&KeyEvent {
                        type_: cef_key_event_type_t::KEYEVENT_CHAR.into(),
                        modifiers: self.flags(*modifiers),
                        windows_key_code: unit as i32,
                        character: unit,
                        unmodified_character: unit,
                        ..Default::default()
                    }));
                }
            }
        }
    }

    pub fn mouse(&self, x: f32, y: f32, modifiers: Modifiers) -> MouseEvent {
        MouseEvent { x: x.round() as i32, y: y.round() as i32, modifiers: self.flags(modifiers) }
    }

    fn flags(&self, m: Modifiers) -> u32 {
        let mut flags = self.buttons;
        for (on, flag) in [
            (m.shift, cef_event_flags_t::EVENTFLAG_SHIFT_DOWN),
            (m.ctrl, cef_event_flags_t::EVENTFLAG_CONTROL_DOWN),
            (m.alt, cef_event_flags_t::EVENTFLAG_ALT_DOWN),
            (m.logo, cef_event_flags_t::EVENTFLAG_COMMAND_DOWN),
        ] {
            if on {
                flags |= flag.0;
            }
        }
        flags
    }
}

fn button_flag(button: MouseButton) -> cef_event_flags_t {
    match button {
        MouseButton::Left => cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON,
        MouseButton::Middle => cef_event_flags_t::EVENTFLAG_MIDDLE_MOUSE_BUTTON,
        MouseButton::Right => cef_event_flags_t::EVENTFLAG_RIGHT_MOUSE_BUTTON,
    }
}
