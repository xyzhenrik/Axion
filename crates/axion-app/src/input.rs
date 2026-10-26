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

/// The events a key press or release sends to the page.
pub fn page_key_events(event: &KeyEvent, modifiers: Modifiers) -> Vec<InputEvent> {
    key_events(event.physical_key, event.text.as_deref(), event.state.is_pressed(), modifiers)
}

/// The events a key sends to the page: the key itself (down/up, with its Windows key code)
/// and, on a press, the text it types, unless it's a shortcut chord.
fn key_events(
    physical: PhysicalKey,
    text: Option<&str>,
    pressed: bool,
    modifiers: Modifiers,
) -> Vec<InputEvent> {
    let mut events = Vec::with_capacity(2);
    let key = match physical {
        PhysicalKey::Code(code) => windows_key_code(code),
        PhysicalKey::Unidentified(_) => None,
    };
    let native_code = physical.to_scancode().map_or(0, |s| s + XKB_KEYCODE_OFFSET) as i32;
    if let Some(key_code) = key {
        events.push(InputEvent::Key {
            pressed,
            key_code,
            native_code,
            modifiers: engine_modifiers(modifiers),
        });
    }
    if pressed && !modifiers.ctrl && !modifiers.logo {
        let text = match physical {
            PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => Some("\r"),
            _ => text,
        };
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            events.push(InputEvent::Text {
                text: text.to_owned(),
                key_code: key.unwrap_or(0),
                native_code,
                modifiers: engine_modifiers(modifiers),
            });
        }
    }
    events
}

/// The key events of typing `text` on a US layout, as a keyboard sends them (a test aid).
pub fn typed_keys(text: &str) -> Vec<InputEvent> {
    let shift = Modifiers { shift: true, ..Default::default() };
    let mut events = Vec::new();
    for c in text.chars() {
        let Some((code, shifted)) = us_key(c) else { continue };
        let modifiers = if shifted { shift } else { Modifiers::default() };
        if shifted {
            events.extend(key_events(PhysicalKey::Code(KeyCode::ShiftLeft), None, true, shift));
        }
        let typed = c.to_string();
        events.extend(key_events(PhysicalKey::Code(code), Some(&typed), true, modifiers));
        events.extend(key_events(PhysicalKey::Code(code), None, false, modifiers));
        if shifted {
            events.extend(key_events(
                PhysicalKey::Code(KeyCode::ShiftLeft),
                None,
                false,
                Modifiers::default(),
            ));
        }
    }
    events
}

/// The key that types `c` on a US layout, and whether Shift is held for it.
fn us_key(c: char) -> Option<(KeyCode, bool)> {
    use KeyCode::*;
    let letters = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ,
        KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    Some(match c {
        'a'..='z' => (letters[c as usize - 'a' as usize], false),
        'A'..='Z' => (letters[c as usize - 'A' as usize], true),
        '0'..='9' => (digits[c as usize - '0' as usize], false),
        '.' => (Period, false),
        ',' => (Comma, false),
        '-' => (Minus, false),
        '/' => (Slash, false),
        ';' => (Semicolon, false),
        '\'' => (Quote, false),
        ' ' => (Space, false),
        '@' => (Digit2, true),
        '#' => (Digit3, true),
        '$' => (Digit4, true),
        '%' => (Digit5, true),
        '&' => (Digit7, true),
        '(' => (Digit9, true),
        ')' => (Digit0, true),
        '!' => (Digit1, true),
        '\x08' => (Backspace, false),
        _ => return None,
    })
}

/// The key events of Ctrl+<letter> as a keyboard sends them to the page (a test aid).
pub fn ctrl_keys(letter: char) -> Vec<InputEvent> {
    // XKB keycode of left Control, and of the letters on a QWERTY layout.
    const CONTROL: i32 = 37;
    const QWERTY: &str = "qwertyuiop    asdfghjkl     zxcvbnm";
    const FIRST_LETTER_KEYCODE: i32 = 24;
    let letter = letter.to_ascii_lowercase();
    let Some(offset) = QWERTY.find(letter).filter(|_| letter.is_ascii_lowercase()) else { return Vec::new() };
    let native = FIRST_LETTER_KEYCODE + offset as i32;
    let key_code = letter.to_ascii_uppercase() as i32;
    let ctrl = axion_engine::Modifiers { ctrl: true, ..Default::default() };
    let none = axion_engine::Modifiers::default();
    vec![
        InputEvent::Key { pressed: true, key_code: 0x11, native_code: CONTROL, modifiers: ctrl },
        InputEvent::Key { pressed: true, key_code, native_code: native, modifiers: ctrl },
        InputEvent::Key { pressed: false, key_code, native_code: native, modifiers: ctrl },
        InputEvent::Key { pressed: false, key_code: 0x11, native_code: CONTROL, modifiers: none },
    ]
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
        CursorKind::Copy => CursorIcon::Copy,
        CursorKind::Alias => CursorIcon::Alias,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, text: Option<&str>, modifiers: Modifiers) -> Vec<InputEvent> {
        key_events(PhysicalKey::Code(code), text, true, modifiers)
    }

    #[test]
    fn typed_character_carries_its_key() {
        let events = press(KeyCode::Period, Some("."), Modifiers::default());
        let [
            InputEvent::Key { pressed: true, key_code: 0xBE, native_code, .. },
            InputEvent::Text { text, key_code, native_code: text_native, .. },
        ] = &events[..]
        else {
            panic!("expected key then text, got {events:?}");
        };
        assert_eq!(text, ".");
        // Not the character's own code (0x2E, the Delete key).
        assert_eq!(*key_code, 0xBE);
        assert_eq!(text_native, native_code);
    }

    #[test]
    fn release_and_chords_type_nothing() {
        let release = key_events(PhysicalKey::Code(KeyCode::KeyA), Some("a"), false, Modifiers::default());
        assert!(matches!(&release[..], [InputEvent::Key { pressed: false, key_code: 0x41, .. }]));
        let ctrl = press(KeyCode::KeyV, Some("v"), Modifiers { ctrl: true, ..Default::default() });
        assert!(matches!(&ctrl[..], [InputEvent::Key { pressed: true, .. }]));
    }

    #[test]
    fn enter_types_carriage_return() {
        let events = press(KeyCode::Enter, Some("\n"), Modifiers::default());
        assert!(matches!(
            &events[..],
            [InputEvent::Key { .. }, InputEvent::Text { text, key_code: 0x0D, .. }] if text == "\r"
        ));
    }

    #[test]
    fn typing_holds_shift_around_shifted_characters() {
        let kinds: Vec<String> = typed_keys("a@")
            .iter()
            .map(|e| match e {
                InputEvent::Key { pressed, key_code, .. } => {
                    format!("{}{key_code:x}", if *pressed { "+" } else { "-" })
                }
                InputEvent::Text { text, .. } => text.clone(),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(kinds, ["+41", "a", "-41", "+10", "+32", "@", "-32", "-10"]);
    }
}
