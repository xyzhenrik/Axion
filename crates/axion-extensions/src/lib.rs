//! Chrome extensions for Axion.
//!
//! Chromium (through CEF) runs the extensions; this crate does what Axion needs around it:
//! read a Manifest V3 `manifest.json` (name, icon, popup, options page and what the extension
//! may access, so the user can see it), work out the extension id Chromium will give it, unpack a
//! `.crx` / `.zip` and keep installed extensions in a folder of the profile.
//!
//! Extensions are loaded unpacked (`--load-extension`), so every extension lives as a plain
//! folder under `<profile>/extensions/`. See `docs/features/extensions.md` for the limits.

mod install;
mod manifest;
mod unpack;

pub use install::{Installed, install_file, install_folder, remove_folder};
pub use manifest::{Manifest, Permissions, extension_id_for_key, extension_id_for_path, is_broad_host};
pub use unpack::{crx_public_key, unzip};

use std::path::PathBuf;

/// Why an extension could not be read or installed.
#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("there is no manifest.json in this folder")]
    NoManifest,
    #[error("manifest.json is not valid: {0}")]
    Manifest(String),
    #[error("this extension uses Manifest V{0}; only Manifest V3 extensions are supported")]
    UnsupportedManifest(u32),
    #[error("not a valid extension package: {0}")]
    Package(String),
    #[error("the extension is larger than {0} MB")]
    TooLarge(u64),
}

pub type Result<T> = std::result::Result<T, ExtensionError>;

/// The most an extension may take unpacked (all files together).
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;

fn io(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> ExtensionError + '_ {
    move |source| ExtensionError::Io { path: path.to_owned(), source }
}
