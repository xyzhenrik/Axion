//! Translating winit input for the keymap, the UI and the page.

use std::time::{Duration, Instant};

use axion_config::{Chord, Key, Modifiers};
use axion_engine::{CursorKind, InputEvent};
use glam::Vec2;
use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::{self, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::CursorIcon;

const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f32 = 4.0;
/// X11/XKB keycodes are evdev scancodes offset by 8; Chromium expects those on Linux.
const XKB_KEYCODE_OFFSET: u32 = 8;

pub fn modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        ctrl: state.control_key(),
        shift: state.shift_key(),
        alt: state.alt_key(),
        logo: state.super_key(),
    }
}

pub fn engine_modifiers(m: Modifiers) -> axion_engine::Modifiers {
    axion_engine::Modifiers { shift: m.shift, ctrl: m.ctrl, alt: m.alt, logo: m.logo }
}

/// The key as the keymap and UI see it: a lower-case character or a canonical name.
pub fn key(event: &KeyEvent) -> Option<Key> {
    // The key without Shift/AltGr applied, so Ctrl+Shift+\ is "\" and not "|".
    match event.key_without_modifiers() {
        keyboard::Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(Key::Char(c.to_ascii_lowercase())),
                _ => None,
            }
        }
        keyboard::Key::Named(NamedKey::Space) => Some(Key::Named("Space".into())),
        keyboard::Key::Named(named) => Key::parse(&format!("{named:?}")),
        _ => None,
    }
}

pub fn chord(event: &KeyEvent, modifiers: Modifiers) -> Option<Chord> {
    Some(Chord { modifiers, key: key(event)? })
}

/// Turns winit clicks into click counts (double/triple click) for the page.
#[derive(Default)]
pub struct ClickCounter {
    last: Option<(Instant, Vec2, MouseButton, u32)>,
}

impl ClickCounter {
    pub fn press(&mut self, button: MouseButton, pos: Vec2) -> u32 {
        let now = Instant::now();
        let count = match self.last {
            Some((t, p, b, n))
                if b == button
                    && now.duration_since(t) < DOUBLE_CLICK_TIME
                    && p.distance(pos) < DOUBLE_CLICK_DISTANCE =>
            {
                n + 1
            }
            _ => 1,
        };
        self.last = Some((now, pos, button, count));
        count
    }

    pub fn current(&self) -> u32 {
        self.last.map_or(1, |(_, _, _, n)| n)
    }
}

/// A physical key event for the page (key down/up with Windows key code).
pub fn page_key(event: &KeyEvent, modifiers: Modifiers) -> Option<InputEvent> {
    let PhysicalKey::Code(code) = event.physical_key else { return None };
    let key_code = windows_key_code(code)?;
    let native_code = event.physical_key.to_scancode().map_or(0, |s| s + XKB_KEYCODE_OFFSET) as i32;
    Some(InputEvent::Key {
        pressed: event.state.is_pressed(),
        key_code,
        native_code,
        modifiers: engine_modifiers(modifiers),
    })
}

/// Text a key press types into the page, unless it's a shortcut chord.
pub fn page_text(event: &KeyEvent, modifiers: Modifiers) -> Option<InputEvent> {
    if !event.state.is_pressed() || modifiers.ctrl || modifiers.logo {
        return None;
    }
    let text = match event.physical_key {
        PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => "\r".to_owned(),
        _ => event.text.as_ref()?.to_string(),
    };
    Some(InputEvent::Text { text, modifiers: engine_modifiers(modifiers) })
}

pub fn cursor_icon(kind: CursorKind) -> Option<CursorIcon> {
    Some(match kind {
        CursorKind::Default => CursorIcon::Default,
        CursorKind::Pointer => CursorIcon::Pointer,
        CursorKind::Text => CursorIcon::Text,
        CursorKind::Crosshair => CursorIcon::Crosshair,
        CursorKind::Wait => CursorIcon::Wait,
        CursorKind::Progress => CursorIcon::Progress,
        CursorKind::Help => CursorIcon::Help,
        CursorKind::Move => CursorIcon::Move,
        CursorKind::Grab => CursorIcon::Grab,
        CursorKind::Grabbing => CursorIcon::Grabbing,
        CursorKind::NotAllowed => CursorIcon::NotAllowed,
        CursorKind::ResizeHorizontal => CursorIcon::EwResize,
        CursorKind::ResizeVertical => CursorIcon::NsResize,
        CursorKind::Hidden => return None,
    })
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
    let numpad = [Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9];
    if let Some(i) = numpad.iter().position(|&k| k == code) {
        return Some(0x60 + i as i32);
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
        NumpadMultiply => 0x6A,
        NumpadAdd => 0x6B,
        NumpadSubtract => 0x6D,
        NumpadDecimal => 0x6E,
        NumpadDivide => 0x6F,
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
