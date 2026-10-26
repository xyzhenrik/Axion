//! Downloads and browsing data, as the browser sees them.

use std::path::{Path, PathBuf};

/// Where a download stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
    InProgress,
    Paused,
    Complete,
    Cancelled,
    Failed,
}

impl DownloadState {
    /// No more updates will come.
    pub fn is_finished(self) -> bool {
        matches!(self, DownloadState::Complete | DownloadState::Cancelled | DownloadState::Failed)
    }
}

/// A download's latest state (sent with every update).
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadInfo {
    /// Unique for the engine's lifetime.
    pub id: u32,
    pub url: String,
    /// The address the download was started with (before redirects).
    pub original_url: String,
    /// Where the file is (or will be) saved; empty until decided.
    pub path: PathBuf,
    pub file_name: String,
    pub mime_type: String,
    pub received_bytes: i64,
    /// -1 while unknown.
    pub total_bytes: i64,
    /// Bytes per second.
    pub speed: i64,
    pub state: DownloadState,
}

/// Where downloads go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadOptions {
    pub directory: PathBuf,
    /// Ask where to save each file (the engine's save dialog).
    pub ask: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadAction {
    Cancel,
    Pause,
    Resume,
}

/// What [`crate::Engine::clear_data`] removes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClearData {
    /// The whole HTTP cache (cached images and files).
    pub cache: bool,
    /// All cookies and site data …
    pub all_site_data: bool,
    /// … or only those of these sites (hosts), e.g. the sites visited in a time range.
    pub sites: Vec<String>,
}

/// `name` in `dir`, or `name (1)`, `name (2)` … if a file of that name exists.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name = sanitize_file_name(name);
    let candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    (1..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(candidate)
}

/// A file name safe to create: no path separators or control characters, never empty.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    if cleaned.is_empty() { "download".into() } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("axion-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a.zip"));
        std::fs::write(dir.join("a.zip"), b"").unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a (1).zip"));
        std::fs::write(dir.join("a (1).zip"), b"").unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a (2).zip"));
        std::fs::write(dir.join("README"), b"").unwrap();
        assert_eq!(unique_path(&dir, "README"), dir.join("README (1)"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(sanitize_file_name("../etc/passwd"), "_etc_passwd");
        assert_eq!(sanitize_file_name("  "), "download");
    }
}
