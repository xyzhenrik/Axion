//! Translates winit input into CEF browser-host events (coordinates in logical pixels).

use std::time::{Duration, Instant};

use cef::sys::{cef_event_flags_t, cef_key_event_type_t, cef_mouse_button_type_t};
use cef::*;
use glam::Vec2;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;

/// Pixels per wheel notch.
const LINE_SCROLL: f32 = 100.0;
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f32 = 4.0;
/// X11/XKB keycodes are evdev scancodes offset by 8; Chromium expects those on Linux.
const XKB_KEYCODE_OFFSET: u32 = 8;

#[derive(Default)]
pub struct InputForwarder {
    modifiers: ModifiersState,
    buttons: u32,
    pos: Vec2,
    last_click: Option<(Instant, Vec2, MouseButton, i32)>,
}

impl InputForwarder {
    pub fn handle(&mut self, browser: &Browser, event: &WindowEvent, scale: f32) {
        let Some(host) = browser.host() else { return };
        match event {
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pos = Vec2::new(position.x as f32, position.y as f32) / scale;
                host.send_mouse_move_event(Some(&self.mouse_event()), 0);
            }
            WindowEvent::CursorLeft { .. } => host.send_mouse_move_event(Some(&self.mouse_event()), 1),
            WindowEvent::MouseInput { state, button, .. } => match button {
                MouseButton::Back if state.is_pressed() => browser.go_back(),
                MouseButton::Forward if state.is_pressed() => browser.go_forward(),
                MouseButton::Left | MouseButton::Middle | MouseButton::Right => {
                    let (kind, flag) = match button {
                        MouseButton::Left => (
                            cef_mouse_button_type_t::MBT_LEFT,
                            cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON,
                        ),
                        MouseButton::Middle => (
                            cef_mouse_button_type_t::MBT_MIDDLE,
                            cef_event_flags_t::EVENTFLAG_MIDDLE_MOUSE_BUTTON,
                        ),
                        _ => (
                            cef_mouse_button_type_t::MBT_RIGHT,
                            cef_event_flags_t::EVENTFLAG_RIGHT_MOUSE_BUTTON,
                        ),
                    };
                    let up = *state == ElementState::Released;
                    let count = if up { self.click_count() } else { self.register_click(*button) };
                    if up {
                        self.buttons &= !flag.0;
                    } else {
                        self.buttons |= flag.0;
                        host.set_focus(1);
                    }
                    host.send_mouse_click_event(Some(&self.mouse_event()), kind.into(), up as _, count);
                }
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(*x, *y) * LINE_SCROLL,
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32) / scale,
                };
                host.send_mouse_wheel_event(Some(&self.mouse_event()), d.x as i32, d.y as i32);
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let pressed = event.state.is_pressed();
                if pressed && self.browser_shortcut(browser, event.physical_key) {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let native = event.physical_key.to_scancode().map_or(0, |s| s + XKB_KEYCODE_OFFSET) as i32;
                if let Some(vk) = windows_key_code(code) {
                    let kind = if pressed {
                        cef_key_event_type_t::KEYEVENT_RAWKEYDOWN
                    } else {
                        cef_key_event_type_t::KEYEVENT_KEYUP
                    };
                    host.send_key_event(Some(&KeyEvent {
                        type_: kind.into(),
                        modifiers: self.flags(),
                        windows_key_code: vk,
                        native_key_code: native,
                        ..Default::default()
                    }));
                }
                // Text input, unless it's a shortcut chord.
                let chord = self.modifiers.control_key() || self.modifiers.super_key();
                if pressed && !chord {
                    let text = match code {
                        KeyCode::Enter | KeyCode::NumpadEnter => Some("\r"),
                        _ => event.text.as_deref(),
                    };
                    for unit in text.unwrap_or_default().encode_utf16() {
                        host.send_key_event(Some(&KeyEvent {
                            type_: cef_key_event_type_t::KEYEVENT_CHAR.into(),
                            modifiers: self.flags(),
                            windows_key_code: unit as i32,
                            native_key_code: native,
                            character: unit,
                            unmodified_character: unit,
                            ..Default::default()
                        }));
                    }
                }
            }
            WindowEvent::Focused(focused) => host.set_focus(*focused as _),
            _ => {}
        }
    }

    /// Alt+←/→ history navigation and F5 reload, handled by the browser instead of the page.
    fn browser_shortcut(&self, browser: &Browser, key: PhysicalKey) -> bool {
        match key {
            PhysicalKey::Code(KeyCode::ArrowLeft) if self.modifiers.alt_key() => browser.go_back(),
            PhysicalKey::Code(KeyCode::ArrowRight) if self.modifiers.alt_key() => browser.go_forward(),
            PhysicalKey::Code(KeyCode::F5) => browser.reload(),
            _ => return false,
        }
        true
    }

    fn register_click(&mut self, button: MouseButton) -> i32 {
        let now = Instant::now();
        let count = match self.last_click {
            Some((t, p, b, n))
                if b == button
                    && now.duration_since(t) < DOUBLE_CLICK_TIME
                    && p.distance(self.pos) < DOUBLE_CLICK_DISTANCE =>
            {
                n + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, self.pos, button, count));
        count
    }

    fn click_count(&self) -> i32 {
        self.last_click.map_or(1, |(_, _, _, n)| n)
    }

    fn flags(&self) -> u32 {
        let m = self.modifiers;
        let mut flags = self.buttons;
        for (on, flag) in [
            (m.shift_key(), cef_event_flags_t::EVENTFLAG_SHIFT_DOWN),
            (m.control_key(), cef_event_flags_t::EVENTFLAG_CONTROL_DOWN),
            (m.alt_key(), cef_event_flags_t::EVENTFLAG_ALT_DOWN),
            (m.super_key(), cef_event_flags_t::EVENTFLAG_COMMAND_DOWN),
        ] {
            if on {
                flags |= flag.0;
            }
        }
        flags
    }

    fn mouse_event(&self) -> MouseEvent {
        MouseEvent { x: self.pos.x as i32, y: self.pos.y as i32, modifiers: self.flags() }
    }
}

/// Windows virtual-key code for a physical key, as Chromium expects in key events.
fn windows_key_code(code: KeyCode) -> Option<i32> {
    use KeyCode::*;
    let letters = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ,
        KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    if let Some(i) = letters.iter().position(|&k| k == code) {
        return Some(0x41 + i as i32);
    }
    let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    if let Some(i) = digits.iter().position(|&k| k == code) {
        return Some(0x30 + i as i32);
    }
    let functions = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(i) = functions.iter().position(|&k| k == code) {
        return Some(0x70 + i as i32);
    }
    Some(match code {
        Backspace => 0x08,
        Tab => 0x09,
        Enter | NumpadEnter => 0x0D,
        ShiftLeft | ShiftRight => 0x10,
        ControlLeft | ControlRight => 0x11,
        AltLeft | AltRight => 0x12,
        CapsLock => 0x14,
        Escape => 0x1B,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Insert => 0x2D,
        Delete => 0x2E,
        SuperLeft => 0x5B,
        SuperRight => 0x5C,
        Semicolon => 0xBA,
        Equal => 0xBB,
        Comma => 0xBC,
        Minus => 0xBD,
        Period => 0xBE,
        Slash => 0xBF,
        Backquote => 0xC0,
        BracketLeft => 0xDB,
        Backslash => 0xDC,
        BracketRight => 0xDD,
        Quote => 0xDE,
        IntlBackslash => 0xE2,
        _ => return None,
    })
}
