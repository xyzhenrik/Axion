//! A small file of named secrets encrypted with the device key alone, no master password.
//!
//! The network connections keep their secrets (proxy password, WireGuard keys) here, so
//! connecting never depends on the password vault being unlocked. The file is protected the
//! way the vault is bound to the device: the key is derived from the device secret (TPM-sealed
//! where there is a TPM, else a private key file, both outside the profile), so a copy of the
//! file is useless on another machine, and without the TPM nothing here can be read.
//!
//! ```text
//! key   = HKDF-SHA256(device_secret, "axion-vault/v1/sealed-store")
//! entry = XChaCha20-Poly1305(key, random nonce, value, aad = name)
//! ```
//!
//! Each entry's name is authenticated, so an entry can't be moved to another name. The file is
//! JSON (`{"version":1,"entries":{name:{"nonce":hex,"data":hex}}}`), written owner-only and
//! atomically. Names are not secret; values are. Nothing secret is ever logged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::crypto::{self, KEY_LEN, Key};
use crate::device::{DeviceKey, DeviceKeyError};
use crate::platform;

const KEY_INFO: &[u8] = b"axion-vault/v1/sealed-store";
const FORMAT: u32 = 1;
/// Largest file read (a damaged or hostile file can't exhaust memory).
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SealedError {
    #[error("the device key isn't available: {0}")]
    Device(#[from] DeviceKeyError),
    #[error("the secrets file can't be used: {0}")]
    Io(#[from] std::io::Error),
    /// Not valid JSON, or too large.
    #[error("the secrets file is damaged")]
    Damaged,
    /// Wrong device, or the entry was changed.
    #[error("the secret \"{0}\" can't be decrypted (another device, or the file was changed)")]
    Decrypt(String),
    #[error("encrypting failed")]
    Encrypt,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    version: u32,
    entries: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    nonce: String,
    data: String,
}

/// Named secrets in a file, encrypted with the device key.
pub struct SealedStore {
    path: PathBuf,
    device: Arc<dyn DeviceKey>,
    /// Serialises read-modify-write cycles within the process.
    lock: Mutex<()>,
}

impl SealedStore {
    pub fn new(path: PathBuf, device: Arc<dyn DeviceKey>) -> Self {
        Self { path, device, lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the device key (may take most of a second with a TPM) so later calls are fast.
    pub fn warm_up(&self) -> Result<(), SealedError> {
        self.device.secret().map(|_| ()).map_err(Into::into)
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn key(&self) -> Result<Key, SealedError> {
        let secret = self.device.secret()?;
        let mut key = Zeroizing::new([0; KEY_LEN]);
        Hkdf::<Sha256>::new(None, secret.as_ref())
            .expand(KEY_INFO, key.as_mut())
            .map_err(|_| SealedError::Encrypt)?;
        Ok(key)
    }

    fn read(&self) -> Result<File, SealedError> {
        match std::fs::metadata(&self.path) {
            Ok(m) if m.len() > MAX_FILE_BYTES => return Err(SealedError::Damaged),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(File::default()),
            Err(e) => return Err(e.into()),
        }
        let bytes = std::fs::read(&self.path)?;
        let file: File = serde_json::from_slice(&bytes).map_err(|_| SealedError::Damaged)?;
        if file.version != FORMAT {
            return Err(SealedError::Damaged);
        }
        Ok(file)
    }

    fn write(&self, file: &File) -> Result<(), SealedError> {
        if let Some(dir) = self.path.parent() {
            platform::create_private_dir(dir)?;
        }
        let bytes = serde_json::to_vec(file).map_err(|_| SealedError::Damaged)?;
        platform::write_private(&self.path, &bytes)?;
        Ok(())
    }

    /// The names that have a value (no device key needed).
    pub fn names(&self) -> Result<Vec<String>, SealedError> {
        let _g = self.guard();
        Ok(self.read()?.entries.into_keys().collect())
    }

    /// The value of `name`, `None` when there is none.
    pub fn get(&self, name: &str) -> Result<Option<Zeroizing<String>>, SealedError> {
        let _g = self.guard();
        let file = self.read()?;
        let Some(entry) = file.entries.get(name) else { return Ok(None) };
        let key = self.key()?;
        open_entry(&key, name, entry).map(Some)
    }

    /// Every value, for a cache that is filled once. An entry that can't be decrypted is
    /// skipped (logged by name only) so one bad entry doesn't hide the others.
    pub fn all(&self) -> Result<Vec<(String, Zeroizing<String>)>, SealedError> {
        let _g = self.guard();
        let file = self.read()?;
        if file.entries.is_empty() {
            return Ok(Vec::new());
        }
        let key = self.key()?;
        let mut out = Vec::new();
        for (name, entry) in &file.entries {
            match open_entry(&key, name, entry) {
                Ok(v) => out.push((name.clone(), v)),
                Err(e) => tracing::warn!("sealed store: {e}"),
            }
        }
        Ok(out)
    }

    /// Stores `value` under `name`, replacing an earlier one.
    pub fn set(&self, name: &str, value: &str) -> Result<(), SealedError> {
        let _g = self.guard();
        let key = self.key()?;
        let mut file = self.read()?;
        file.version = FORMAT;
        let (nonce, data) =
            crypto::seal(&key, value.as_bytes(), name.as_bytes()).map_err(|_| SealedError::Encrypt)?;
        file.entries.insert(name.to_owned(), Entry { nonce: hex(&nonce), data: hex(&data) });
        self.write(&file)
    }

    /// Removes `name` (nothing happens if it is missing).
    pub fn remove(&self, name: &str) -> Result<(), SealedError> {
        let _g = self.guard();
        let mut file = self.read()?;
        if file.entries.remove(name).is_some() {
            self.write(&file)?;
        }
        Ok(())
    }
}

fn open_entry(key: &Key, name: &str, entry: &Entry) -> Result<Zeroizing<String>, SealedError> {
    let bad = || SealedError::Decrypt(name.to_owned());
    let nonce = unhex(&entry.nonce).ok_or_else(bad)?;
    let data = unhex(&entry.data).ok_or_else(bad)?;
    let plain = crypto::open(key, &nonce, &data, name.as_bytes()).map_err(|_| bad())?;
    String::from_utf8(plain.to_vec()).map(Zeroizing::new).map_err(|_| bad())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{MemoryDeviceKey, SystemDeviceKey};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axion-sealed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn store(dir: &Path, device: MemoryDeviceKey) -> SealedStore {
        SealedStore::new(dir.join("net").join("secrets.json"), Arc::new(device))
    }

    #[test]
    fn round_trip_and_remove() {
        let dir = scratch("round");
        let s = store(&dir, MemoryDeviceKey::new([3; KEY_LEN]));
        assert!(s.get("c-1").unwrap().is_none());
        s.set("c-1", "{\"password\":\"pw\"}").unwrap();
        s.set("c-2", "two").unwrap();
        assert_eq!(s.get("c-1").unwrap().unwrap().as_str(), "{\"password\":\"pw\"}");
        assert_eq!(s.names().unwrap(), ["c-1", "c-2"]);
        assert_eq!(s.all().unwrap().len(), 2);
        s.set("c-1", "new").unwrap();
        assert_eq!(s.get("c-1").unwrap().unwrap().as_str(), "new");
        s.remove("c-1").unwrap();
        s.remove("missing").unwrap();
        assert!(s.get("c-1").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_holds_no_plaintext_and_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("plain");
        let s = store(&dir, MemoryDeviceKey::new([4; KEY_LEN]));
        s.set("c-1", "hunter2-hunter2").unwrap();
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(!text.contains("hunter2"));
        let mode = std::fs::metadata(s.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "owner only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_device_cannot_read_it() {
        let dir = scratch("other");
        store(&dir, MemoryDeviceKey::new([5; KEY_LEN])).set("c-1", "x").unwrap();
        let other = store(&dir, MemoryDeviceKey::new([6; KEY_LEN]));
        assert!(matches!(other.get("c-1"), Err(SealedError::Decrypt(_))));
        assert!(other.all().unwrap().is_empty(), "undecryptable entries are skipped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_cannot_be_moved_to_another_name() {
        let dir = scratch("move");
        let s = store(&dir, MemoryDeviceKey::new([7; KEY_LEN]));
        s.set("a", "secret-a").unwrap();
        let mut file = s.read().unwrap();
        let entry = file.entries.remove("a").unwrap();
        file.entries.insert("b".into(), entry);
        s.write(&file).unwrap();
        assert!(matches!(s.get("b"), Err(SealedError::Decrypt(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_file_is_an_error_not_a_panic() {
        let dir = scratch("damaged");
        let s = store(&dir, MemoryDeviceKey::new([8; KEY_LEN]));
        s.set("a", "x").unwrap();
        std::fs::write(s.path(), b"not json").unwrap();
        assert!(matches!(s.names(), Err(SealedError::Damaged)));
        assert!(matches!(s.set("b", "y"), Err(SealedError::Damaged)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn works_with_a_throwaway_device_key_dir() {
        let dir = scratch("system");
        let keys = dir.join("keys");
        // File-only: never the TPM, never the real key directory.
        let device = Arc::new(SystemDeviceKey::file_only(keys.clone()));
        let s = SealedStore::new(dir.join("secrets.json"), device);
        s.warm_up().unwrap();
        s.set("c-1", "kept").unwrap();
        // A new handle with the same key directory (a restart) reads it.
        let again = SealedStore::new(dir.join("secrets.json"), Arc::new(SystemDeviceKey::file_only(keys)));
        assert_eq!(again.get("c-1").unwrap().unwrap().as_str(), "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
