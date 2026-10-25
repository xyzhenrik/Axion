//! The device key: a random 256-bit secret that never leaves this machine, mixed into every
//! vault key so a copied vault is useless elsewhere (see the crate docs for the construction).
//!
//! [`SystemDeviceKey`] keeps it in the TPM where there is one: the secret is sealed under a
//! storage key that only this TPM can recreate, and the sealed blob (`device.tpm`) may sit in
//! a plain file because nothing but this TPM can open it. Without a usable TPM (no device, no
//! permission, a TPM error, or a build without the `tpm` feature) the secret is a file of its
//! own (`device.key`, mode 0600 in a 0700 directory). Both live in `$XDG_DATA_HOME/axion`
//! (or `AXION_DEVICE_KEY_DIR`), outside every profile and account, so a backup or copy of a
//! profile doesn't carry it. Once created, a device key keeps its backend: a sealed key is never
//! silently replaced by a file one (that would look like another device to every vault).
//!
//! The secret is never logged; only the backend is.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::crypto::{self, KEY_LEN};
use crate::platform;

/// The device secret, wiped from memory when dropped.
pub type DeviceSecret = Zeroizing<[u8; KEY_LEN]>;

/// The environment variable that moves the device key's directory (tests, throwaway runs).
pub const DIR_ENV: &str = "AXION_DEVICE_KEY_DIR";
/// The file holding a TPM-sealed device key.
pub const SEALED_FILE: &str = "device.tpm";
/// The file holding a device key without a TPM.
pub const PLAIN_FILE: &str = "device.key";

#[derive(Debug, thiserror::Error)]
pub enum DeviceKeyError {
    #[error("no place for the device key (no home directory)")]
    NoHome,
    #[error("the device key file can't be used: {0}")]
    Io(#[from] io::Error),
    #[error("the device key file is damaged")]
    Damaged,
    /// The TPM is unusable, or can't open the sealed key.
    #[error("TPM: {0}")]
    Tpm(String),
}

/// Where the vault gets the device secret from.
pub trait DeviceKey: Send + Sync {
    /// The secret, created on first use.
    fn secret(&self) -> Result<DeviceSecret, DeviceKeyError>;
}

/// A fixed secret held in memory: for tests, and to simulate several devices.
pub struct MemoryDeviceKey(DeviceSecret);

impl MemoryDeviceKey {
    pub fn new(secret: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(secret))
    }

    /// A fresh random device.
    pub fn random() -> Self {
        Self::new(crypto::random_bytes())
    }
}

impl DeviceKey for MemoryDeviceKey {
    fn secret(&self) -> Result<DeviceSecret, DeviceKeyError> {
        Ok(self.0.clone())
    }
}

/// No device key at all (no home directory): every vault reports it as unavailable.
pub struct NoDeviceKey;

impl DeviceKey for NoDeviceKey {
    fn secret(&self) -> Result<DeviceSecret, DeviceKeyError> {
        Err(DeviceKeyError::NoHome)
    }
}

/// Which store a [`SystemDeviceKey`] uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Tpm,
    File,
}

/// The real device key: TPM-sealed where possible, a private file otherwise. Loaded once,
/// then kept in memory for the rest of the process.
pub struct SystemDeviceKey {
    dir: PathBuf,
    use_tpm: bool,
    loaded: Mutex<Option<(DeviceSecret, Backend)>>,
}

impl SystemDeviceKey {
    /// The device key in `dir`, TPM first.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, use_tpm: true, loaded: Mutex::new(None) }
    }

    /// The device key in `dir`, never using the TPM to create one (an existing sealed key
    /// still needs it).
    pub fn file_only(dir: PathBuf) -> Self {
        Self { dir, use_tpm: false, loaded: Mutex::new(None) }
    }

    /// The device key in its usual place: `AXION_DEVICE_KEY_DIR`, else `$XDG_DATA_HOME/axion`.
    pub fn system() -> Result<Self, DeviceKeyError> {
        Ok(Self::new(default_dir()?))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The backend in use, once the key was loaded.
    pub fn backend(&self) -> Option<Backend> {
        self.loaded.lock().ok()?.as_ref().map(|(_, b)| *b)
    }

    fn load(&self) -> Result<(DeviceSecret, Backend), DeviceKeyError> {
        let sealed = self.dir.join(SEALED_FILE);
        let plain = self.dir.join(PLAIN_FILE);
        if sealed.exists() {
            let blob = std::fs::read(&sealed)?;
            return Ok((platform::tpm::unseal(&blob)?, Backend::Tpm));
        }
        if plain.exists() {
            return Ok((read_plain(&plain)?, Backend::File));
        }
        platform::create_private_dir(&self.dir)?;
        if self.use_tpm {
            match create_sealed(&sealed) {
                Ok(secret) => return Ok((secret, Backend::Tpm)),
                Err(e) => tracing::info!("device key: no usable TPM ({e}); using a key file"),
            }
            // Another process may have sealed one meanwhile.
            if sealed.exists() {
                let blob = std::fs::read(&sealed)?;
                return Ok((platform::tpm::unseal(&blob)?, Backend::Tpm));
            }
        }
        let secret: DeviceSecret = Zeroizing::new(crypto::random_bytes());
        if !platform::write_private_new(&plain, secret.as_ref())? {
            // Lost a race with another process: use its key.
            return Ok((read_plain(&plain)?, Backend::File));
        }
        Ok((secret, Backend::File))
    }
}

impl DeviceKey for SystemDeviceKey {
    fn secret(&self) -> Result<DeviceSecret, DeviceKeyError> {
        let mut loaded = self.loaded.lock().map_err(|_| DeviceKeyError::Tpm("poisoned lock".into()))?;
        if let Some((secret, _)) = loaded.as_ref() {
            return Ok(secret.clone());
        }
        let (secret, backend) = self.load()?;
        match backend {
            Backend::Tpm => tracing::info!("device key: sealed by the TPM"),
            Backend::File => tracing::info!("device key: key file (no TPM)"),
        }
        *loaded = Some((secret.clone(), backend));
        Ok(secret)
    }
}

/// Seals a new secret with the TPM and stores the blob, after checking it unseals.
fn create_sealed(path: &Path) -> Result<DeviceSecret, DeviceKeyError> {
    let secret: DeviceSecret = Zeroizing::new(crypto::random_bytes());
    let blob = platform::tpm::seal(&secret)?;
    if *platform::tpm::unseal(&blob)? != *secret {
        return Err(DeviceKeyError::Tpm("the sealed key doesn't read back".into()));
    }
    if !platform::write_private_new(path, &blob)? {
        // Another process sealed one first: use that.
        return platform::tpm::unseal(&std::fs::read(path)?);
    }
    Ok(secret)
}

fn read_plain(path: &Path) -> Result<DeviceSecret, DeviceKeyError> {
    platform::warn_if_shared(path);
    let bytes = Zeroizing::new(std::fs::read(path)?);
    let array: [u8; KEY_LEN] = bytes.as_slice().try_into().map_err(|_| DeviceKeyError::Damaged)?;
    Ok(Zeroizing::new(array))
}

/// `AXION_DEVICE_KEY_DIR`, else `axion` in the user's data directory (`~/.local/share/axion`).
pub fn default_dir() -> Result<PathBuf, DeviceKeyError> {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let dirs = directories::BaseDirs::new().ok_or(DeviceKeyError::NoHome)?;
    Ok(dirs.data_dir().join("axion"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axion-device-key-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn file_key_is_created_once_and_kept() {
        let dir = scratch("file");
        let a = SystemDeviceKey::file_only(dir.clone()).secret().unwrap();
        assert_eq!(SystemDeviceKey::file_only(dir.clone()).backend(), None);
        let b = SystemDeviceKey::file_only(dir.clone());
        assert_eq!(*b.secret().unwrap(), *a);
        assert_eq!(b.backend(), Some(Backend::File));
        assert!(dir.join(PLAIN_FILE).exists() && !dir.join(SEALED_FILE).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.join(PLAIN_FILE)), 0o600);
            assert_eq!(mode(&dir), 0o700);
        }
        // Another directory is another device.
        let other = scratch("file-other");
        assert_ne!(*SystemDeviceKey::file_only(other.clone()).secret().unwrap(), *a);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn damaged_key_file_is_an_error_not_a_new_key() {
        let dir = scratch("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(PLAIN_FILE), b"short").unwrap();
        assert!(matches!(SystemDeviceKey::file_only(dir.clone()).secret(), Err(DeviceKeyError::Damaged)));
        assert_eq!(std::fs::read(dir.join(PLAIN_FILE)).unwrap(), b"short");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sealed_key_is_never_replaced_by_a_file() {
        let dir = scratch("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SEALED_FILE), b"not a sealed blob").unwrap();
        assert!(SystemDeviceKey::file_only(dir.clone()).secret().is_err());
        assert!(!dir.join(PLAIN_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The real TPM: seals a device key in a scratch directory, reads it back, and checks a
    /// second instance unseals the same secret. Needs read/write access to `/dev/tpmrm0`
    /// (the `tss` group). Run with
    /// `cargo test -p axion-vault -- --ignored real_tpm --nocapture`.
    #[test]
    #[ignore = "needs a TPM 2.0 and access to /dev/tpmrm0"]
    fn real_tpm() {
        // Straight to the TPM first, for a clear reason when it isn't usable.
        let probe: DeviceSecret = Zeroizing::new([0x5a; KEY_LEN]);
        let blob = platform::tpm::seal(&probe).unwrap_or_else(|e| panic!("sealing failed: {e}"));
        let opened = platform::tpm::unseal(&blob).unwrap_or_else(|e| panic!("unsealing failed: {e}"));
        assert_eq!(*opened, *probe);
        let mut flipped = blob.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert!(platform::tpm::unseal(&flipped).is_err(), "a changed blob must not unseal");

        let dir = scratch("tpm");
        let first = SystemDeviceKey::new(dir.clone());
        let secret = first.secret().unwrap();
        assert_eq!(first.backend(), Some(Backend::Tpm), "fell back to a key file: no usable TPM");
        assert!(dir.join(SEALED_FILE).exists() && !dir.join(PLAIN_FILE).exists());
        let blob = std::fs::read(dir.join(SEALED_FILE)).unwrap();
        assert!(!blob.windows(KEY_LEN).any(|w| w == secret.as_slice()), "the blob holds the secret in clear");
        let second = SystemDeviceKey::new(dir.clone());
        assert_eq!(*second.secret().unwrap(), *secret);
        assert_eq!(second.backend(), Some(Backend::Tpm));
        // A vault made with it opens with it, and not with another device's key.
        let mut v = crate::Vault::in_memory(std::sync::Arc::new(second)).unwrap();
        v.setup_with("test-only-pass", crate::KdfParams::TEST).unwrap();
        v.lock();
        v.unlock("test-only-pass").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
