//! Platform specifics: private files (owner-only permissions where the OS has them) and the
//! TPM (Linux, `tpm` feature; elsewhere a stub that always reports "no TPM").

use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(all(target_os = "linux", feature = "tpm"))]
#[path = "tpm_linux.rs"]
pub mod tpm;

#[cfg(not(all(target_os = "linux", feature = "tpm")))]
pub mod tpm {
    use crate::device::{DeviceKeyError, DeviceSecret};

    const NO_TPM: &str = "this build has no TPM support";

    pub fn seal(_secret: &DeviceSecret) -> Result<Vec<u8>, DeviceKeyError> {
        Err(DeviceKeyError::Tpm(NO_TPM.into()))
    }

    pub fn unseal(_blob: &[u8]) -> Result<DeviceSecret, DeviceKeyError> {
        Err(DeviceKeyError::Tpm(NO_TPM.into()))
    }
}

/// Creates `dir` (and its parents) and makes it owner-only.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dir)?.permissions();
        if perms.mode() & 0o077 != 0 {
            perms.set_mode(0o700);
            std::fs::set_permissions(dir, perms)?;
        }
    }
    Ok(())
}

/// Writes an owner-only temporary file next to `path` and returns its path.
fn write_temp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nonce: [u8; 8] = crate::crypto::random_bytes();
    let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let temp = path.with_file_name(format!(".{name}.{suffix}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options.open(&temp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(temp)
}

/// Creates `path` with `bytes`, owner-only, all at once (never a half-written file). Returns
/// `false`, writing nothing, when `path` already exists.
pub fn write_private_new(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let temp = write_temp(path, bytes)?;
    // A hard link appears atomically and fails if the name is taken.
    let linked = std::fs::hard_link(&temp, path);
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Writes `path` (replacing it) with `bytes`, owner-only, all at once.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = write_temp(path, bytes)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Warns when a secret file is readable by others (it still works).
pub fn warn_if_shared(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            tracing::warn!(path = %path.display(), "the device key file is readable by other users");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
