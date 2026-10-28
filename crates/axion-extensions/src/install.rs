//! Installing: copying or unpacking an extension into the profile's extension folder.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::manifest::{extension_id_for_key, extension_id_for_path};
use crate::unpack::unzip;
use crate::{ExtensionError, MAX_BYTES, Manifest, Result, io};

/// An extension installed under the profile's extension folder.
#[derive(Clone, Debug)]
pub struct Installed {
    /// The id Chromium will give it.
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Installs the unpacked extension in `source` (a folder with a `manifest.json`) by copying it
/// below `root`. The original may be changed or deleted afterwards without effect.
pub fn install_folder(source: &Path, root: &Path) -> Result<Installed> {
    Manifest::read(source)?;
    let staging = staging_dir(root, source)?;
    let result = (|| {
        let mut total = 0;
        copy_tree(source, &staging, &mut total)?;
        finish(&staging, root)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Installs a `.crx` or `.zip` package by unpacking it below `root`.
pub fn install_file(file: &Path, root: &Path) -> Result<Installed> {
    let meta = std::fs::metadata(file).map_err(io(file))?;
    if meta.len() > MAX_BYTES {
        return Err(ExtensionError::TooLarge(MAX_BYTES / (1024 * 1024)));
    }
    let data = std::fs::read(file).map_err(io(file))?;
    let staging = staging_dir(root, file)?;
    let result = (|| {
        let key = unzip(&data, &staging)?;
        if let Some(key) = key {
            // Chromium would store the key with the installed copy; an unpacked folder keeps
            // the extension's id only if the manifest has it.
            pin_key(&staging.join("manifest.json"), &key)?;
        }
        finish(&staging, root)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Deletes an installed extension's folder. Only folders directly below `root` go.
pub fn remove_folder(dir: &Path, root: &Path) -> Result<()> {
    if dir.parent() != Some(root) {
        return Err(ExtensionError::Package("not an installed extension folder".into()));
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(dir)(e)),
    }
}

/// A new, empty folder below `root` for unpacking.
fn staging_dir(root: &Path, source: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(root).map_err(io(root))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(source.to_string_lossy().as_bytes());
    hasher.update(now.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let name: String = hasher.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
    let dir = root.join(format!("x{name}"));
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    Ok(dir)
}

/// Checks the unpacked extension and works out its id.
fn finish(dir: &Path, _root: &Path) -> Result<Installed> {
    let manifest = Manifest::read(dir)?;
    let id = manifest
        .key
        .as_deref()
        .and_then(extension_id_for_key)
        .unwrap_or_else(|| extension_id_for_path(dir));
    tracing::info!(%id, name = %manifest.name, "extension unpacked");
    Ok(Installed { id, dir: dir.to_owned(), manifest })
}

/// Writes `key` into the manifest's `key` field (if it has none).
fn pin_key(manifest: &Path, key: &str) -> Result<()> {
    let text = std::fs::read_to_string(manifest).map_err(io(manifest))?;
    let mut value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    if let Some(object) = value.as_object_mut()
        && !object.contains_key("key")
    {
        object.insert("key".into(), Value::String(key.to_owned()));
        let out = serde_json::to_string_pretty(&value).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        std::fs::write(manifest, out).map_err(io(manifest))?;
    }
    Ok(())
}

/// Copies the regular files and folders of `from` into `to` (links and special files are skipped).
fn copy_tree(from: &Path, to: &Path, total: &mut u64) -> Result<()> {
    for entry in std::fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let kind = entry.file_type().map_err(io(from))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(io(&target))?;
            copy_tree(&entry.path(), &target, total)?;
        } else if kind.is_file() {
            *total += entry.metadata().map_err(io(from))?.len();
            if *total > MAX_BYTES {
                return Err(ExtensionError::TooLarge(MAX_BYTES / (1024 * 1024)));
            }
            std::fs::copy(entry.path(), &target).map_err(io(&target))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unpack::tests::{make_crx3, make_zip};

    const MANIFEST: &[u8] = br#"{"manifest_version":3,"name":"Test","version":"1.0"}"#;

    #[test]
    fn a_folder_is_copied() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("manifest.json"), MANIFEST).unwrap();
        std::fs::create_dir(source.path().join("js")).unwrap();
        std::fs::write(source.path().join("js/a.js"), "1").unwrap();
        let root = tempfile::tempdir().unwrap();
        let installed = install_folder(source.path(), root.path()).unwrap();
        assert_eq!(installed.manifest.name, "Test");
        assert_eq!(installed.id.len(), 32);
        assert_eq!(installed.dir.parent(), Some(root.path()));
        assert!(installed.dir.join("js/a.js").exists());
        remove_folder(&installed.dir, root.path()).unwrap();
        assert!(!installed.dir.exists());
    }

    #[test]
    fn a_folder_without_manifest_leaves_nothing_behind() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(install_folder(source.path(), root.path()), Err(ExtensionError::NoManifest)));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_crx_keeps_its_id_through_the_key() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("t.crx");
        std::fs::write(&file, make_crx3(&make_zip(&[("manifest.json", MANIFEST)]), b"pub key bytes")).unwrap();
        let installed = install_file(&file, &root.path().join("ext")).unwrap();
        let key = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"pub key bytes");
        assert_eq!(installed.manifest.key.as_deref(), Some(key.as_str()));
        assert_eq!(Some(installed.id), extension_id_for_key(&key));
    }

    #[test]
    fn only_installed_folders_can_be_removed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(remove_folder(outside.path(), root.path()).is_err());
        assert!(outside.path().exists());
    }
}
