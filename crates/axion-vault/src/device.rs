//! The device key: a random 256-bit secret that never leaves this machine, mixed into every
//! vault key so a copied vault is useless elsewhere (see the crate docs for the construction).
//!
//! [`SystemDeviceKey`] keeps it in the TPM where there is one: the secret is sealed under a
//! storage key that only this TPM can recreate, and the sealed blob (`device.tpm`) may sit in
//! a plain file because nothing but this TPM can open it. Without a usable TPM (no device, no
//! permission, a TPM error, or a build without the `tpm` feature) the secret is a file of its
//! own (`device.key`, mode 0600 in a 0700 directory). Both live in `$XDG_DATA_HOME/axion`
//! (or `AXION_DEVICE_KEY_DIR`), outside every profile and account, so a backup or copy of a
//! profile doesn't carry it. A sealed key is never replaced by a file one (that would look like
//! another device to every vault); a key file moves into the TPM, same secret, once the TPM
//! becomes usable (see [`SystemDeviceKey`]).
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
    /// The secret, created on first use. May wait for the TPM (most of a second).
    fn secret(&self) -> Result<DeviceSecret, DeviceKeyError>;

    /// The secret if it is at hand without waiting: loaded before, or a key that needs no
    /// device. `None` while a TPM-sealed key hasn't been loaded (see [`DeviceKey::secret`]).
    fn ready_secret(&self) -> Option<DeviceSecret> {
        self.secret().ok()
    }
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

/// Seals and unseals the device secret: the platform TPM, or a fake in tests.
pub trait Sealer: Send + Sync {
    /// Seals `secret`; the blob opens only with this sealer.
    fn seal(&self, secret: &DeviceSecret) -> Result<Vec<u8>, DeviceKeyError>;
    fn unseal(&self, blob: &[u8]) -> Result<DeviceSecret, DeviceKeyError>;
}

/// This machine's TPM (see `platform::tpm`).
pub struct PlatformTpm;

impl Sealer for PlatformTpm {
    fn seal(&self, secret: &DeviceSecret) -> Result<Vec<u8>, DeviceKeyError> {
        platform::tpm::seal(secret)
    }

    fn unseal(&self, blob: &[u8]) -> Result<DeviceSecret, DeviceKeyError> {
        platform::tpm::unseal(blob)
    }
}

/// The real device key: TPM-sealed where possible, a private file otherwise. Loaded once,
/// then kept in memory for the rest of the process.
///
/// A key file made while the TPM was unusable moves into the TPM once it is usable: the same
/// secret is sealed (so no vault needs re-encrypting), `device.tpm` is written and read back
/// through the TPM, and only when that matches byte for byte is `device.key` overwritten with
/// zeros and removed. On any failure the key file stays in use and the move is retried at the
/// next start.
pub struct SystemDeviceKey {
    dir: PathBuf,
    tpm: Box<dyn Sealer>,
    /// Whether to create or move keys into the TPM (an existing sealed key always needs it).
    use_tpm: bool,
    loaded: Mutex<Option<(DeviceSecret, Backend)>>,
}

impl SystemDeviceKey {
    /// The device key in `dir`, TPM first.
    pub fn new(dir: PathBuf) -> Self {
        Self::with_sealer(dir, Box::new(PlatformTpm))
    }

    /// The device key in `dir`, sealed by `tpm`.
    pub fn with_sealer(dir: PathBuf, tpm: Box<dyn Sealer>) -> Self {
        Self { dir, tpm, use_tpm: true, loaded: Mutex::new(None) }
    }

    /// The device key in `dir`, never using the TPM to create one or to move a key file into
    /// (an existing sealed key still needs it).
    pub fn file_only(dir: PathBuf) -> Self {
        Self { dir, tpm: Box::new(PlatformTpm), use_tpm: false, loaded: Mutex::new(None) }
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
            return Ok((self.tpm.unseal(&blob)?, Backend::Tpm));
        }
        if plain.exists() {
            let secret = read_plain(&plain)?;
            if self.use_tpm {
                match self.move_into_tpm(&secret, &sealed, &plain) {
                    Ok(()) => {
                        tracing::info!("device key: moved from the key file into the TPM");
                        return Ok((secret, Backend::Tpm));
                    }
                    Err(e) => tracing::warn!(
                        "device key: can't move the key file into the TPM ({e}); keeping the file, \
                         trying again next start"
                    ),
                }
            }
            return Ok((secret, Backend::File));
        }
        platform::create_private_dir(&self.dir)?;
        if self.use_tpm {
            match self.create_sealed(&sealed) {
                Ok(secret) => return Ok((secret, Backend::Tpm)),
                Err(e) => tracing::info!("device key: no usable TPM ({e}); using a key file"),
            }
            // Another process may have sealed one meanwhile.
            if sealed.exists() {
                let blob = std::fs::read(&sealed)?;
                return Ok((self.tpm.unseal(&blob)?, Backend::Tpm));
            }
        }
        let secret: DeviceSecret = Zeroizing::new(crypto::random_bytes());
        if !platform::write_private_new(&plain, secret.as_ref())? {
            // Lost a race with another process: use its key.
            return Ok((read_plain(&plain)?, Backend::File));
        }
        Ok((secret, Backend::File))
    }

    /// Seals a new secret and stores the blob, after checking it unseals.
    fn create_sealed(&self, path: &Path) -> Result<DeviceSecret, DeviceKeyError> {
        let secret: DeviceSecret = Zeroizing::new(crypto::random_bytes());
        let blob = self.tpm.seal(&secret)?;
        if *self.tpm.unseal(&blob)? != *secret {
            return Err(DeviceKeyError::Tpm("the sealed key doesn't read back".into()));
        }
        if !platform::write_private_new(path, &blob)? {
            // Another process sealed one first: use that.
            return self.tpm.unseal(&std::fs::read(path)?);
        }
        Ok(secret)
    }

    /// Seals the key file's `secret` into `sealed`, verifies it from disk and only then wipes
    /// the key file. On failure `sealed` is gone again and the key file untouched.
    fn move_into_tpm(
        &self,
        secret: &DeviceSecret,
        sealed: &Path,
        plain: &Path,
    ) -> Result<(), DeviceKeyError> {
        let blob = self.tpm.seal(secret)?;
        if !platform::write_private_new(sealed, &blob)? {
            return Err(DeviceKeyError::Tpm("another sealed key appeared meanwhile".into()));
        }
        let verified = std::fs::read(sealed)
            .map_err(DeviceKeyError::from)
            .and_then(|blob| self.tpm.unseal(&blob))
            .and_then(|back| {
                if *back == **secret {
                    Ok(())
                } else {
                    Err(DeviceKeyError::Tpm("the sealed key doesn't match the key file".into()))
                }
            });
        if let Err(e) = verified {
            // Never leave a sealed key that isn't this device's key: it would win next time.
            let _ = std::fs::remove_file(sealed);
            return Err(e);
        }
        // The sealed key now wins; a failure here only leaves a stale (zeroed) file behind.
        if let Err(e) = platform::wipe_and_remove(plain) {
            tracing::warn!("device key: sealed, but the old key file couldn't be removed: {e}");
        }
        Ok(())
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

    fn ready_secret(&self) -> Option<DeviceSecret> {
        // Busy: another thread is loading it right now (the TPM): not ready either.
        self.loaded.try_lock().ok()?.as_ref().map(|(secret, _)| secret.clone())
    }
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

    /// A pretend TPM: the blob is the secret XOR 0x5a behind a tag.
    #[derive(Clone, Copy)]
    enum FakeTpm {
        Works,
        SealFails,
        /// Unseals a different secret than was sealed.
        Mismatch,
    }

    impl Sealer for FakeTpm {
        fn seal(&self, secret: &DeviceSecret) -> Result<Vec<u8>, DeviceKeyError> {
            if let FakeTpm::SealFails = self {
                return Err(DeviceKeyError::Tpm("test: no access".into()));
            }
            Ok(b"FAKE".iter().copied().chain(secret.iter().map(|b| b ^ 0x5a)).collect())
        }

        fn unseal(&self, blob: &[u8]) -> Result<DeviceSecret, DeviceKeyError> {
            let body = blob.strip_prefix(b"FAKE".as_slice()).ok_or(DeviceKeyError::Damaged)?;
            let mut secret: [u8; KEY_LEN] = body.try_into().map_err(|_| DeviceKeyError::Damaged)?;
            secret.iter_mut().for_each(|b| *b ^= 0x5a);
            if let FakeTpm::Mismatch = self {
                secret[0] ^= 1;
            }
            Ok(Zeroizing::new(secret))
        }
    }

    fn fake(dir: &Path, tpm: FakeTpm) -> SystemDeviceKey {
        SystemDeviceKey::with_sealer(dir.to_owned(), Box::new(tpm))
    }

    /// A key file from a start without TPM access.
    fn key_file(name: &str) -> (PathBuf, DeviceSecret) {
        let dir = scratch(name);
        let secret = SystemDeviceKey::file_only(dir.clone()).secret().unwrap();
        assert!(dir.join(PLAIN_FILE).exists());
        (dir, secret)
    }

    #[test]
    fn a_key_file_moves_into_the_tpm() {
        let (dir, secret) = key_file("move");
        let moved = fake(&dir, FakeTpm::Works);
        assert_eq!(*moved.secret().unwrap(), *secret, "the same secret: no vault re-encryption");
        assert_eq!(moved.backend(), Some(Backend::Tpm));
        assert!(!dir.join(PLAIN_FILE).exists(), "the key file is gone");
        assert!(dir.join(SEALED_FILE).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(SEALED_FILE)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Next start: the sealed key, same secret. Not at hand before it was loaded.
        let next = fake(&dir, FakeTpm::Works);
        assert!(next.ready_secret().is_none());
        assert_eq!(*next.secret().unwrap(), *secret);
        assert_eq!(next.ready_secret().as_deref(), Some(&*secret));
        assert_eq!(next.backend(), Some(Backend::Tpm));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_seal_keeps_the_key_file() {
        let (dir, secret) = key_file("seal-fails");
        let raw = std::fs::read(dir.join(PLAIN_FILE)).unwrap();
        let key = fake(&dir, FakeTpm::SealFails);
        assert_eq!(*key.secret().unwrap(), *secret);
        assert_eq!(key.backend(), Some(Backend::File));
        assert_eq!(std::fs::read(dir.join(PLAIN_FILE)).unwrap(), raw);
        assert!(!dir.join(SEALED_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mismatch_keeps_the_key_file_and_retries_later() {
        let (dir, secret) = key_file("mismatch");
        let raw = std::fs::read(dir.join(PLAIN_FILE)).unwrap();
        let key = fake(&dir, FakeTpm::Mismatch);
        assert_eq!(*key.secret().unwrap(), *secret);
        assert_eq!(key.backend(), Some(Backend::File));
        assert_eq!(std::fs::read(dir.join(PLAIN_FILE)).unwrap(), raw);
        assert!(!dir.join(SEALED_FILE).exists(), "no wrong sealed key left behind");
        // The next start, with a working TPM, moves it.
        let next = fake(&dir, FakeTpm::Works);
        assert_eq!(*next.secret().unwrap(), *secret);
        assert_eq!(next.backend(), Some(Backend::Tpm));
        assert!(!dir.join(PLAIN_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
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
