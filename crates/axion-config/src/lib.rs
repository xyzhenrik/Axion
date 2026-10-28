//! Axion's settings.
//!
//! Everything user-facing is a key here, with defaults taken from the design system
//! (`docs/design/Main.dc.html`, "Foundations"). Each account has its own directory for them
//! (`~/.config/axion/accounts/<id>/`, see `axion-accounts`): `config.toml` for settings and
//! `keymap.toml` for shortcuts. Both are optional and partial — anything missing falls back to
//! the default — and both are reloaded live when edited.

mod color;
mod keymap;
mod schema;
mod store;
mod watch;
mod window_state;

pub use color::{ACCENT_SWATCHES, Color};
pub use keymap::{Action, Chord, Key, Keymap, KeymapConflict, Modifiers};
pub use schema::*;
pub use store::{ConfigPaths, load_config, load_keymap, save_config};
pub use watch::{Changed, ConfigWatcher};
pub use window_state::{MonitorRect, WINDOW_STATE_FILE, WindowState, is_visible};

/// Errors from reading, parsing or writing configuration files.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Read { path: std::path::PathBuf, source: std::io::Error },
    #[error("writing {path}: {source}")]
    Write { path: std::path::PathBuf, source: std::io::Error },
    #[error("{path} is not valid: {source}")]
    Parse { path: std::path::PathBuf, source: toml::de::Error },
    #[error("serializing settings: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid key binding {chord:?}: {reason}")]
    Chord { chord: String, reason: String },
    #[error("unknown action {0:?} in keymap")]
    UnknownAction(String),
    #[error("watching configuration files: {0}")]
    Watch(#[from] notify::Error),
}
