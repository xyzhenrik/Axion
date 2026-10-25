//! The desktop's light/dark preference, read from the XDG settings portal
//! (`org.freedesktop.appearance` `color-scheme`) and followed live. winit's own theme
//! detection doesn't work on most Linux desktops, so `theme = "auto"` relies on this.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedValue, Value};

const UNKNOWN: u8 = 0;
const DARK: u8 = 1;
const LIGHT: u8 = 2;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

pub struct DesktopTheme {
    state: Arc<AtomicU8>,
}

impl DesktopTheme {
    /// Reads the preference and keeps following it on a background thread.
    pub fn start() -> Self {
        let state = Arc::new(AtomicU8::new(UNKNOWN));
        let shared = Arc::clone(&state);
        let spawned = std::thread::Builder::new().name("desktop-theme".into()).spawn(move || {
            if let Err(e) = follow(&shared) {
                tracing::debug!("desktop light/dark preference unavailable: {e}");
            }
        });
        if let Err(e) = spawned {
            tracing::debug!("could not watch the desktop theme: {e}");
        }
        Self { state }
    }

    /// `Some(true)` when the desktop prefers dark, `Some(false)` for light, `None` while
    /// unknown (no portal, or no preference).
    pub fn dark(&self) -> Option<bool> {
        match self.state.load(Ordering::Relaxed) {
            DARK => Some(true),
            LIGHT => Some(false),
            _ => None,
        }
    }
}

/// The portal's value: 1 prefers dark, 2 prefers light, 0 no preference.
fn state_of(value: &Value<'_>) -> u8 {
    let mut value = value;
    // `Read` (the older method) wraps the value in another variant.
    while let Value::Value(inner) = value {
        value = inner;
    }
    match value {
        Value::U32(1) => DARK,
        Value::U32(2) => LIGHT,
        _ => UNKNOWN,
    }
}

fn follow(state: &AtomicU8) -> zbus::Result<()> {
    let connection = Connection::session()?;
    let read = connection
        .call_method(Some(PORTAL), PATH, Some(SETTINGS), "ReadOne", &(NAMESPACE, KEY))
        .or_else(|_| connection.call_method(Some(PORTAL), PATH, Some(SETTINGS), "Read", &(NAMESPACE, KEY)))?;
    let value: OwnedValue = read.body().deserialize()?;
    state.store(state_of(&value), Ordering::Relaxed);

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SETTINGS)?
        .member("SettingChanged")?
        .path(PATH)?
        .build();
    for message in MessageIterator::for_match_rule(rule, &connection, None)? {
        let Ok(message) = message else { continue };
        let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, OwnedValue)>() else {
            continue;
        };
        if namespace == NAMESPACE && key == KEY {
            state.store(state_of(&value), Ordering::Relaxed);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_values() {
        assert_eq!(state_of(&Value::U32(1)), DARK);
        assert_eq!(state_of(&Value::U32(2)), LIGHT);
        assert_eq!(state_of(&Value::U32(0)), UNKNOWN);
        assert_eq!(state_of(&Value::Value(Box::new(Value::U32(1)))), DARK);
    }
}
