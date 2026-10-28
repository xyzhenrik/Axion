//! What is remembered of the window between runs: where it was, how big, whether it was
//! maximized or fullscreen, and which sidebar panels were open (`window.toml` in the
//! profile directory; whether and how it is used is `[window]` in the settings).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// File name inside the account's profile directory.
pub const WINDOW_STATE_FILE: &str = "window.toml";

/// The remembered window. Every field is optional/defaulted: a missing or damaged file just
/// means the window opens as it would the first time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// Inner size in logical pixels (the restored size when maximized).
    pub width: f64,
    pub height: f64,
    /// Outer position in physical pixels, where the platform reports one (not on Wayland).
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// Name of the monitor the window was on.
    pub monitor: Option<String>,
    pub maximized: bool,
    pub fullscreen: bool,
    /// The docked sidebar was open (`None`: no opinion, use `layout.sidebar_visible`).
    pub sidebar_open: Option<bool>,
    pub tabs_section_open: Option<bool>,
    pub saved_section_open: Option<bool>,
    pub private_section_open: Option<bool>,
    /// The Picture in Picture window's size (logical px) as the user last made it: its area is
    /// kept for the next video, whatever its shape.
    pub pip_width: Option<f64>,
    pub pip_height: Option<f64>,
}

impl WindowState {
    pub fn path(profile: &Path) -> PathBuf {
        profile.join(WINDOW_STATE_FILE)
    }

    /// Reads the file; a missing or unreadable one gives the defaults (logged if damaged).
    pub fn load(profile: &Path) -> Self {
        let path = Self::path(profile);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("{} is not valid ({e}); ignoring it", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the file atomically (temp file, then rename).
    pub fn save(&self, profile: &Path) -> Result<(), ConfigError> {
        let path = Self::path(profile);
        let write_err = |source| ConfigError::Write { path: path.clone(), source };
        let text = toml::to_string_pretty(self)?;
        std::fs::create_dir_all(profile).map_err(write_err)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(write_err)?;
        std::fs::rename(&tmp, &path).map_err(write_err)
    }

    /// Whether a size was ever remembered.
    pub fn has_size(&self) -> bool {
        self.width >= 1.0 && self.height >= 1.0
    }
}

/// A monitor's rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Whether a window whose outer rectangle is `(x, y, width, height)` (physical px) shows at
/// least `min_visible` pixels both ways on one of `monitors`. With no monitor known the
/// position is trusted.
pub fn is_visible(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    min_visible: u32,
    monitors: &[MonitorRect],
) -> bool {
    if monitors.is_empty() {
        return true;
    }
    let need = i64::from(min_visible).min(i64::from(width)).min(i64::from(height));
    monitors.iter().any(|m| {
        let left = i64::from(x).max(i64::from(m.x));
        let right = (i64::from(x) + i64::from(width)).min(i64::from(m.x) + i64::from(m.width));
        let top = i64::from(y).max(i64::from(m.y));
        let bottom = (i64::from(y) + i64::from(height)).min(i64::from(m.y) + i64::from(m.height));
        right - left >= need && bottom - top >= need
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("axion-window-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn round_trips() {
        let d = dir("rt");
        let state = WindowState {
            width: 1200.0,
            height: 800.0,
            x: Some(-30),
            y: Some(40),
            monitor: Some("HDMI-A-1".into()),
            maximized: true,
            sidebar_open: Some(false),
            pip_width: Some(640.0),
            pip_height: Some(360.0),
            ..WindowState::default()
        };
        assert!(state.save(&d).is_ok());
        assert_eq!(WindowState::load(&d), state);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_or_damaged_file_is_default() {
        let d = dir("bad");
        assert_eq!(WindowState::load(&d), WindowState::default());
        assert!(std::fs::create_dir_all(&d).is_ok());
        assert!(std::fs::write(WindowState::path(&d), "width = [").is_ok());
        assert!(!WindowState::load(&d).has_size());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn visibility_needs_overlap_with_a_monitor() {
        let m = [MonitorRect { x: 0, y: 0, width: 1920, height: 1080 }];
        assert!(is_visible(100, 100, 800, 600, 120, &m));
        assert!(is_visible(1800, 900, 800, 600, 120, &m));
        assert!(!is_visible(1850, 100, 800, 600, 120, &m));
        // Second monitor unplugged.
        assert!(!is_visible(2500, 0, 800, 600, 120, &m));
        assert!(is_visible(2500, 0, 800, 600, 120, &[]));
        assert!(!is_visible(-790, 0, 800, 600, 120, &m));
    }
}
