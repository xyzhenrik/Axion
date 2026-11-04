//! Updating a tarball / portable installation: the unpacked `Axion-<v>-linux-x86_64` folder
//! (what `tools/install.sh` copies to `~/.local/opt/axion`, or any folder the tarball was
//! extracted to).
//!
//! The update is two phases so the running browser is never disturbed:
//!
//! 1. [`stage_tarball`] (while Axion runs): download the release's tarball next to the
//!    installation, verify its checksum, unpack it into `<dir>.new-<version>` (same file
//!    system) with path-traversal protection and a size bound, and check the tree.
//! 2. [`apply_staged`] (at "Restart", right before the new process starts): `rename` the
//!    installation to `<dir>.old-<oldversion>` and the staged tree to `<dir>`; if the second
//!    rename fails the first is undone. The old tree stays as the backup; older backups go.
//!
//! Nothing outside `<dir>` and its siblings `<dir>.new-*` / `<dir>.old-*` is touched; settings
//! and browsing data live elsewhere. The desktop entry and icons heal themselves at the next
//! start (`desktop_integration.rs`).

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::UpdateError;
use crate::http::Http;
use crate::install::{Progress, Step, published_checksum};
use crate::release::Release;

/// The marker file in the root of a tarball installation.
pub const MARKER: &str = "axion-install.json";

/// `axion-install.json`: written by `tools/package.sh` into the tarball and by this module
/// after an update. Tarballs of 0.2.0 / 0.3.0 have none; the layout check covers them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub kind: String,
    pub version: String,
    #[serde(default)]
    pub installed_at: u64,
}

pub fn read_marker(dir: &Path) -> Option<Marker> {
    serde_json::from_slice(&std::fs::read(dir.join(MARKER)).ok()?).ok()
}

fn write_marker(dir: &Path, version: &str) -> Result<(), UpdateError> {
    let installed_at =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let marker = Marker { kind: "tarball".into(), version: version.to_owned(), installed_at };
    let text = serde_json::to_vec_pretty(&marker).map_err(|e| UpdateError::Parse(e.to_string()))?;
    let path = dir.join(MARKER);
    std::fs::write(&path, text).map_err(|e| UpdateError::io_path("writing", &path, e))
}

/// Whether `dir` looks like an unpacked Axion tarball: the executable and either the marker or
/// the launcher of the 0.2/0.3 layout.
pub fn is_tarball_dir(dir: &Path) -> bool {
    dir.join("usr/bin/axion").is_file() && (dir.join(MARKER).is_file() || dir.join("AppRun").is_file())
}

/// The installation folder `exe` lives in (`<dir>/usr/bin/axion`, or any file below `<dir>`).
pub fn detect_dir(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().skip(1).find(|dir| dir.parent().is_some() && is_tarball_dir(dir)).map(Path::to_path_buf)
}

/// The version recorded in the installation, if it has a marker.
pub fn installed_version(dir: &Path) -> Option<String> {
    read_marker(dir).map(|m| m.version)
}

/// Whether Axion may replace `dir`: it and its parent must be writable (the swap renames
/// inside the parent). A `/opt` install by root, or a read-only mount, is not.
pub fn check_writable(dir: &Path) -> Result<(), UpdateError> {
    let parent = dir.parent().ok_or(UpdateError::NotSelfUpdating)?;
    for place in [parent, dir] {
        let probe = place.join(format!(".axion-write-test-{}", std::process::id()));
        match std::fs::File::create(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
            }
            Err(_) => return Err(UpdateError::NotWritable(place.to_path_buf())),
        }
    }
    Ok(())
}

fn sibling(dir: &Path, suffix: &str) -> Result<PathBuf, UpdateError> {
    let name = dir.file_name().ok_or(UpdateError::NotSelfUpdating)?;
    let mut name = name.to_owned();
    name.push(suffix);
    Ok(dir.with_file_name(name))
}

/// A version as part of a folder name.
fn file_safe(version: &str) -> String {
    version
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+') { c } else { '_' })
        .collect()
}

/// A tree that was downloaded, unpacked and checked, waiting for [`apply_staged`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    /// The installation it will replace.
    pub dir: PathBuf,
    /// `<dir>.new-<version>`.
    pub staging: PathBuf,
    pub version: String,
}

/// Downloads, verifies and unpacks the release's tarball for `arch` next to `dir`.
/// `max_extracted` bounds the unpacked size (bytes). Leaves nothing behind on failure.
pub fn stage_tarball(
    release: &Release,
    dir: &Path,
    arch: &str,
    http: &Http,
    max_extracted: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Staged, UpdateError> {
    check_writable(dir)?;
    let asset = release.tarball_asset(arch).ok_or(UpdateError::NoAsset)?;
    let expected = published_checksum(release, &asset.name, http)?;
    let version = release.version.to_string();
    let staging = sibling(dir, &format!(".new-{}", file_safe(&version)))?;
    let archive = sibling(dir, ".download.tar.gz")?;
    remove_leftovers(dir);

    let result = (|| {
        progress(Progress::Step(Step::Downloading));
        http.download(&asset.url, &archive, cancel, &mut |done, total| {
            progress(Progress::Download { done, total });
        })?;
        progress(Progress::Step(Step::Verifying));
        if let Some(expected) = &expected {
            crate::checksum::verify_file(&archive, expected)?;
        }
        progress(Progress::Step(Step::Extracting));
        extract_tar_gz(&archive, &staging, max_extracted, cancel, progress)?;
        check_staged(&staging, &version)
    })();
    let _ = std::fs::remove_file(&archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result?;
    Ok(Staged { dir: dir.to_path_buf(), staging, version })
}

/// Half-finished staging trees and downloads of earlier attempts.
fn remove_leftovers(dir: &Path) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else { return };
    let prefix = format!("{}.new-", name.to_string_lossy());
    let download = format!("{}.download.tar.gz", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with(&prefix) {
            let _ = std::fs::remove_dir_all(entry.path());
        } else if file == download {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The staged tree must be an Axion tree of the release's version.
fn check_staged(staging: &Path, version: &str) -> Result<(), UpdateError> {
    let exe = staging.join("usr/bin/axion");
    let meta = std::fs::metadata(&exe)
        .map_err(|_| UpdateError::BadArchive("the archive has no usr/bin/axion".into()))?;
    if !meta.is_file() || !is_executable(&meta) {
        return Err(UpdateError::BadArchive("usr/bin/axion is not an executable file".into()));
    }
    if !staging.join("AppRun").is_file() && !staging.join(MARKER).is_file() {
        return Err(UpdateError::BadArchive("the archive has neither AppRun nor a marker".into()));
    }
    if let Some(marker) = read_marker(staging)
        && marker.version != version
    {
        return Err(UpdateError::BadArchive(format!(
            "the archive contains Axion {} but the release is {version}",
            marker.version
        )));
    }
    write_marker(staging, version)
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    true
}

// ---- extraction ----

/// Fails with `FileTooLarge` once more than `left` bytes were read.
struct Limited<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            return Err(io::Error::new(io::ErrorKind::FileTooLarge, "the archive expands beyond the limit"));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// Counts the compressed bytes read (for the progress bar).
struct Counting<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn bad(context: &str, e: io::Error) -> UpdateError {
    match e.kind() {
        io::ErrorKind::FileTooLarge => {
            UpdateError::BadArchive("it expands to more than the allowed size".into())
        }
        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => {
            UpdateError::BadArchive(format!("{context}: {e} (truncated or damaged)"))
        }
        _ => UpdateError::io(context.to_owned(), e),
    }
}

/// The entry's path below the archive's single top-level folder, or `None` for the folder
/// itself. Absolute paths and `..` are refused.
fn strip_top(path: &Path) -> Result<Option<PathBuf>, UpdateError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            _ => return Err(UpdateError::BadArchive(format!("unsafe path {}", path.display()))),
        }
    }
    if parts.len() <= 1 {
        return Ok(None);
    }
    Ok(Some(parts[1..].iter().collect()))
}

/// A link target may only climb out of the link's folder up to the tree's root, and only at
/// its start (`../lib`), never after a name (`a/../..`).
fn check_link(rel: &Path, target: &Path) -> Result<(), UpdateError> {
    let unsafe_link =
        || UpdateError::BadArchive(format!("unsafe link {} -> {}", rel.display(), target.display()));
    let mut depth = rel.components().count() as i64 - 1;
    let mut named = false;
    for component in target.components() {
        match component {
            Component::ParentDir => {
                if named {
                    return Err(unsafe_link());
                }
                depth -= 1;
                if depth < 0 {
                    return Err(unsafe_link());
                }
            }
            Component::Normal(_) => named = true,
            Component::CurDir => {}
            _ => return Err(unsafe_link()),
        }
    }
    Ok(())
}

/// No folder on the way to `target` (below `root`) may be a link: nothing is written through one.
fn check_no_link_ancestor(root: &Path, target: &Path) -> Result<(), UpdateError> {
    let mut current = target.parent();
    while let Some(dir) = current {
        if dir == root {
            break;
        }
        if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(UpdateError::BadArchive(format!(
                "{} would be written through a link",
                target.display()
            )));
        }
        current = dir.parent();
    }
    Ok(())
}

/// Unpacks the gzip'd tar `archive` into `dest` (stripping its top-level folder), refusing
/// anything that is not a file, folder or contained symlink. Permissions are kept.
pub fn extract_tar_gz(
    archive: &Path,
    dest: &Path,
    max_bytes: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), UpdateError> {
    let file = std::fs::File::open(archive).map_err(|e| UpdateError::io_path("opening", archive, e))?;
    let total = file.metadata().map(|m| m.len()).ok();
    let count = Arc::new(AtomicU64::new(0));
    let counting = Counting { inner: io::BufReader::new(file), count: Arc::clone(&count) };
    let mut limited = Limited { inner: flate2::read::GzDecoder::new(counting), left: max_bytes };
    std::fs::create_dir_all(dest).map_err(|e| UpdateError::io_path("creating", dest, e))?;
    {
        let mut tar = tar::Archive::new(&mut limited);
        tar.set_preserve_permissions(true);
        tar.set_unpack_xattrs(false);
        let mut last_reported = 0u64;
        for entry in tar.entries().map_err(|e| bad("reading the archive", e))? {
            if cancel.load(Ordering::Relaxed) {
                return Err(UpdateError::Cancelled);
            }
            let mut entry = entry.map_err(|e| bad("reading the archive", e))?;
            let kind = entry.header().entry_type();
            if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() || kind.is_gnu_longname() {
                continue;
            }
            let path = entry.path().map_err(|e| bad("reading the archive", e))?.into_owned();
            let Some(rel) = strip_top(&path)? else { continue };
            let target = dest.join(&rel);
            check_no_link_ancestor(dest, &target)?;
            if kind.is_dir() {
                std::fs::create_dir_all(&target).map_err(|e| UpdateError::io_path("creating", &target, e))?;
                entry.unpack(&target).map_err(|e| bad("unpacking", e))?;
            } else if kind.is_file() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| UpdateError::io_path("creating", parent, e))?;
                }
                entry.unpack(&target).map_err(|e| bad("unpacking", e))?;
            } else if kind.is_symlink() {
                let link = entry
                    .link_name()
                    .map_err(|e| bad("reading the archive", e))?
                    .ok_or_else(|| UpdateError::BadArchive(format!("link {} has no target", rel.display())))?
                    .into_owned();
                check_link(&rel, &link)?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| UpdateError::io_path("creating", parent, e))?;
                }
                entry.unpack(&target).map_err(|e| bad("unpacking", e))?;
            } else {
                return Err(UpdateError::BadArchive(format!(
                    "{} is not a file, folder or link",
                    rel.display()
                )));
            }
            let done = count.load(Ordering::Relaxed);
            if total.is_some_and(|t| done - last_reported > t / 100) {
                last_reported = done;
                progress(Progress::Download { done, total });
            }
        }
    }
    // The tar end marker comes before the gzip trailer: read on so a truncated or corrupt
    // stream (bad CRC) is noticed.
    io::copy(&mut limited, &mut io::sink()).map_err(|e| bad("reading the archive", e))?;
    progress(Progress::Download { done: total.unwrap_or(0), total });
    Ok(())
}

// ---- the swap ----

/// Swaps the staged tree in: `dir` becomes `<dir>.old-<oldversion>` (the backup), the staging
/// folder becomes `dir`. If the second step fails the first is undone and `dir` is as before.
/// Older `<dir>.old-*` backups are removed afterwards. Returns the backup's path.
pub fn apply_staged(staged: &Staged) -> Result<PathBuf, UpdateError> {
    let dir = &staged.dir;
    let old_version = installed_version(dir).unwrap_or_else(|| "previous".into());
    let backup = sibling(dir, &format!(".old-{}", file_safe(&old_version)))?;
    // The same version twice (a reinstall): the earlier backup makes room.
    if backup.exists() {
        std::fs::remove_dir_all(&backup).map_err(|e| UpdateError::io_path("removing", &backup, e))?;
    }
    std::fs::rename(dir, &backup).map_err(|e| UpdateError::io_path("moving aside", dir, e))?;
    if let Err(e) = std::fs::rename(&staged.staging, dir) {
        // Back to where we were.
        if let Err(undo) = std::fs::rename(&backup, dir) {
            tracing::error!("could not restore {} from {}: {undo}", dir.display(), backup.display());
        }
        return Err(UpdateError::io_path("installing", &staged.staging, e));
    }
    remove_old_backups(dir, &backup);
    Ok(backup)
}

/// Removes `<dir>.old-*` other than `keep`.
fn remove_old_backups(dir: &Path, keep: &Path) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else { return };
    let prefix = format!("{}.old-", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        if entry.path() != keep && entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Throws a staged tree away (the update was abandoned).
pub fn discard_staged(staged: &Staged) {
    let _ = std::fs::remove_dir_all(&staged.staging);
}

#[cfg(test)]
mod tests;
