//! Axion's password manager: logins, cards, addresses and secure notes in an encrypted vault.
//!
//! The vault is its own SQLite file in the profile; every item is encrypted on its own with
//! XChaCha20-Poly1305, its id authenticated alongside so ciphertexts can't be swapped.
//!
//! # Keys
//!
//! The vault key needs two factors, the master password and this device's secret
//! ([`device`]: sealed by the TPM, or a private file outside every profile):
//!
//! ```text
//! password_key = Argon2id(password, salt, kdf)              salt: 16 random bytes per key
//! vault_key    = HKDF-SHA256(salt, password_key ‖ device_secret, "axion-vault/v2/vault-key")
//! device_check = HKDF-SHA256(salt, device_secret, "axion-vault/v2/device-check")
//! ```
//!
//! Salt, cost and `device_check` are stored in the vault, with a known value sealed under the
//! vault key. Unlocking first compares `device_check` (a mismatch is
//! [`VaultError::OtherDevice`]: the vault, or a copy of the whole profile, came from another
//! machine) and then the sealed check value (a mismatch is [`VaultError::WrongPassword`]).
//! A copied `vault.sqlite` is useless without the device secret, even with the password, and
//! the device secret alone is useless without the password (Argon2id still guards it).
//!
//! Changing the password draws a new salt and re-encrypts every item under the same device
//! secret. Vaults from before device binding (format 1: `vault_key = password_key`) are
//! upgraded on their next successful unlock, in one transaction; if that fails they stay as
//! they were and still open. `meta.format` holds the version.
//!
//! [`Vault::export_encrypted`] writes a portable copy encrypted with the password alone (see
//! [`export`]), the only way to move the passwords to another device.
//!
//! While unlocked, the key and the decrypted items live only in memory; locking wipes both.
//! Nothing secret is ever logged.

pub mod autofill;
mod crypto;
pub mod device;
pub mod export;
pub mod generator;
pub mod import;
mod item;
mod platform;
pub mod site;

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use crypto::KdfParams;
pub use device::{DeviceKey, DeviceKeyError, MemoryDeviceKey, SystemDeviceKey};
pub use item::{Address, Card, Item, ItemData, Kind, Login, Note};

use crypto::{Key, SALT_LEN};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);
    CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, nonce BLOB NOT NULL, data BLOB NOT NULL);
";
/// Format 1: the key comes from the password alone.
const FORMAT_PASSWORD_ONLY: u32 = 1;
/// Format 2: the key needs the password and the device secret.
const FORMAT_DEVICE_BOUND: u32 = 2;
/// Encrypted at setup; decrypting it proves the password (format 1).
const CHECK_PLAINTEXT_V1: &[u8] = b"axion-vault-v1";
/// The same for format 2.
const CHECK_PLAINTEXT_V2: &[u8] = b"axion-vault-v2";
const CHECK_AAD: &[u8] = b"check";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("wrong password")]
    WrongPassword,
    /// The vault was made on another device (copied file or profile): only an export moves it.
    #[error("this vault belongs to another device; restore it from an Axion export")]
    OtherDevice,
    /// This device's key can't be read (e.g. the TPM refuses).
    #[error("this device's key is unavailable: {0}")]
    DeviceKey(#[from] DeviceKeyError),
    #[error("the vault is locked")]
    Locked,
    #[error("the vault has no master password yet")]
    NotSetUp,
    #[error("the vault already has a master password")]
    AlreadySetUp,
    #[error("the password must not be empty")]
    EmptyPassword,
    #[error("the vault is damaged: {0}")]
    Corrupt(&'static str),
    #[error("no item with that id")]
    NoSuchItem,
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("this isn't an Axion passwords export")]
    NotAnExport,
    #[error("this export was made by a newer Axion (version {0})")]
    UnsupportedExport(u32),
}

pub type Result<T> = std::result::Result<T, VaultError>;

/// What is encrypted for each item.
#[derive(Serialize, Deserialize)]
struct Envelope {
    data: ItemData,
    created: i64,
    modified: i64,
}

pub struct Vault {
    conn: Connection,
    device: Arc<dyn DeviceKey>,
    key: Option<Key>,
    items: Vec<Item>,
    last_activity: Instant,
    /// Domains that share a sign-in (see [`site::Related`]).
    related: site::Related,
    /// Site tags read from the index, for the locked-state check (see [`Vault::site_known`]).
    index: RefCell<Option<Vec<[u8; crypto::SITE_TAG_LEN]>>>,
    /// A background load of the device key is running (see [`Vault::load_device_key_in_background`]).
    loading_device_key: Arc<AtomicBool>,
    /// Tests: make the format upgrade fail just before it commits.
    #[cfg(test)]
    fail_upgrade: bool,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn new_id() -> String {
    crypto::random_bytes::<16>().iter().map(|b| format!("{b:02x}")).collect()
}

impl Vault {
    /// Opens (or creates) the vault file, bound to `device`. It starts locked.
    pub fn open(path: &Path, device: Arc<dyn DeviceKey>) -> Result<Self> {
        Self::init(Connection::open(path)?, device)
    }

    pub fn in_memory(device: Arc<dyn DeviceKey>) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, device)
    }

    fn init(conn: Connection, device: Arc<dyn DeviceKey>) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            device,
            key: None,
            items: Vec::new(),
            last_activity: Instant::now(),
            related: site::Related::default(),
            index: RefCell::new(None),
            loading_device_key: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fail_upgrade: false,
        })
    }

    fn meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Whether a master password has been set.
    pub fn is_set_up(&self) -> bool {
        self.meta("check").ok().flatten().is_some()
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// The key scheme of the vault on disk (1: password only, 2: password and device).
    fn format(&self) -> Result<u32> {
        match self.meta("format")? {
            None => Ok(FORMAT_PASSWORD_ONLY),
            Some(raw) => std::str::from_utf8(&raw)
                .ok()
                .and_then(|v| v.parse().ok())
                .ok_or(VaultError::Corrupt("format")),
        }
    }

    /// Sets the master password of a new vault and unlocks it.
    pub fn setup(&mut self, password: &str) -> Result<()> {
        self.setup_with(password, KdfParams::DEFAULT)
    }

    pub fn setup_with(&mut self, password: &str, params: KdfParams) -> Result<()> {
        if self.is_set_up() {
            return Err(VaultError::AlreadySetUp);
        }
        self.key = Some(self.write_key(password, params)?);
        self.items.clear();
        self.reindex();
        self.touch();
        Ok(())
    }

    /// Derives a new device-bound key with a fresh salt and stores salt, cost, device check,
    /// check value and format.
    fn write_key(&self, password: &str, params: KdfParams) -> Result<Key> {
        if password.is_empty() {
            return Err(VaultError::EmptyPassword);
        }
        let device = self.device.secret()?;
        let salt = crypto::random_bytes::<SALT_LEN>();
        let password_key =
            crypto::derive_key(password, &salt, params).map_err(|_| VaultError::Corrupt("key derivation"))?;
        let key = crypto::combine(&password_key, &device, &salt)
            .map_err(|_| VaultError::Corrupt("key derivation"))?;
        let device_check =
            crypto::device_check(&device, &salt).map_err(|_| VaultError::Corrupt("key derivation"))?;
        let (nonce, check) = crypto::seal(&key, CHECK_PLAINTEXT_V2, CHECK_AAD)
            .map_err(|_| VaultError::Corrupt("encryption"))?;
        let kdf = format!("{},{},{}", params.memory, params.iterations, params.parallelism);
        self.set_meta("salt", &salt)?;
        self.set_meta("kdf", kdf.as_bytes())?;
        self.set_meta("device_check", &device_check)?;
        self.set_meta("check_nonce", &nonce)?;
        self.set_meta("check", &check)?;
        self.set_meta("format", FORMAT_DEVICE_BOUND.to_string().as_bytes())?;
        Ok(key)
    }

    fn kdf_params(&self) -> Result<KdfParams> {
        let raw = self.meta("kdf")?.ok_or(VaultError::NotSetUp)?;
        let text = String::from_utf8(raw).map_err(|_| VaultError::Corrupt("kdf"))?;
        let n: Vec<u32> = text.split(',').filter_map(|v| v.parse().ok()).collect();
        match n[..] {
            [memory, iterations, parallelism] => Ok(KdfParams { memory, iterations, parallelism }),
            _ => Err(VaultError::Corrupt("kdf")),
        }
    }

    /// Derives the key from `password` (and the device secret) and checks it, without
    /// unlocking.
    fn key_for(&self, password: &str) -> Result<Key> {
        let salt = self.meta("salt")?.ok_or(VaultError::NotSetUp)?;
        let (key, expected) = match self.format()? {
            FORMAT_PASSWORD_ONLY => {
                let key = crypto::derive_key(password, &salt, self.kdf_params()?)
                    .map_err(|_| VaultError::Corrupt("key derivation"))?;
                (key, CHECK_PLAINTEXT_V1)
            }
            FORMAT_DEVICE_BOUND => {
                let stored = self.meta("device_check")?.ok_or(VaultError::Corrupt("device check"))?;
                let device = self.device.secret()?;
                let check = crypto::device_check(&device, &salt)
                    .map_err(|_| VaultError::Corrupt("key derivation"))?;
                if stored != check {
                    return Err(VaultError::OtherDevice);
                }
                let password_key = crypto::derive_key(password, &salt, self.kdf_params()?)
                    .map_err(|_| VaultError::Corrupt("key derivation"))?;
                let key = crypto::combine(&password_key, &device, &salt)
                    .map_err(|_| VaultError::Corrupt("key derivation"))?;
                (key, CHECK_PLAINTEXT_V2)
            }
            _ => return Err(VaultError::Corrupt("made by a newer Axion")),
        };
        let nonce = self.meta("check_nonce")?.ok_or(VaultError::Corrupt("check"))?;
        let check = self.meta("check")?.ok_or(VaultError::Corrupt("check"))?;
        match crypto::open(&key, &nonce, &check, CHECK_AAD) {
            Ok(plain) if plain.as_slice() == expected => Ok(key),
            _ => Err(VaultError::WrongPassword),
        }
    }

    /// Checks the master password without changing anything (locked stays locked, unlocked
    /// stays unlocked): for asking again before something sensitive.
    pub fn verify(&self, password: &str) -> Result<()> {
        self.key_for(password).map(|_| ())
    }

    /// Unlocks with the master password and decrypts the items. A vault from before device
    /// binding is upgraded on the way (or, if that fails, opens as it is).
    pub fn unlock(&mut self, password: &str) -> Result<()> {
        let key = self.key_for(password)?;
        self.items = self.read_items(&key)?;
        self.sort();
        let key = if self.format()? == FORMAT_PASSWORD_ONLY {
            match self.upgrade(password) {
                Ok(new) => {
                    tracing::info!("vault upgraded to a device-bound key");
                    new
                }
                Err(e) => {
                    tracing::warn!(
                        "upgrading the vault to a device-bound key failed, it stays as it was: {e}"
                    );
                    key
                }
            }
        } else {
            key
        };
        self.key = Some(key);
        self.last_activity = Instant::now();
        self.reindex();
        Ok(())
    }

    fn read_items(&self, key: &Key) -> Result<Vec<Item>> {
        let mut stmt = self.conn.prepare("SELECT id, nonce, data FROM items")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (id, nonce, data) = row?;
            let plain = crypto::open(key, &nonce, &data, id.as_bytes())
                .map_err(|_| VaultError::Corrupt("an item doesn't decrypt"))?;
            let envelope: Envelope =
                serde_json::from_slice(&plain).map_err(|_| VaultError::Corrupt("an item doesn't parse"))?;
            items.push(Item {
                id,
                data: envelope.data,
                created: envelope.created,
                modified: envelope.modified,
            });
        }
        Ok(items)
    }

    /// Re-encrypts the (decrypted) items under a new device-bound key from `password`, all in
    /// one transaction: on any failure nothing on disk changes.
    fn upgrade(&self, password: &str) -> Result<Key> {
        let tx = self.conn.unchecked_transaction()?;
        let key = self.write_key(password, self.kdf_params()?)?;
        self.rewrite_items(&key)?;
        #[cfg(test)]
        if self.fail_upgrade {
            return Err(VaultError::Corrupt("test: upgrade fails"));
        }
        tx.commit()?;
        Ok(key)
    }

    /// Writes every item in memory again, encrypted with `key`.
    fn rewrite_items(&self, key: &Key) -> Result<()> {
        for item in &self.items {
            let envelope =
                Envelope { data: item.data.clone(), created: item.created, modified: item.modified };
            self.write_with(key, &item.id, &envelope)?;
        }
        Ok(())
    }

    /// Wipes the key and the decrypted items from memory.
    pub fn lock(&mut self) {
        self.key = None;
        self.items.clear();
    }

    /// Something was done with the vault (resets the auto-lock timer).
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Locks after `after` without activity. Returns whether it just locked.
    pub fn lock_if_idle(&mut self, after: Duration) -> bool {
        if self.is_unlocked() && self.last_activity.elapsed() >= after {
            self.lock();
            return true;
        }
        false
    }

    fn key(&self) -> Result<&Key> {
        self.key.as_ref().ok_or(VaultError::Locked)
    }

    fn sort(&mut self) {
        self.items.sort_by_cached_key(|i| (i.data.kind(), i.data.title().to_lowercase(), i.data.subtitle()));
    }

    /// The decrypted items, by kind and title (empty while locked).
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn write(&self, id: &str, envelope: &Envelope) -> Result<()> {
        self.write_with(self.key()?, id, envelope)
    }

    fn write_with(&self, key: &Key, id: &str, envelope: &Envelope) -> Result<()> {
        let plain =
            Zeroizing::new(serde_json::to_vec(envelope).map_err(|_| VaultError::Corrupt("serialize"))?);
        let (nonce, data) =
            crypto::seal(key, &plain, id.as_bytes()).map_err(|_| VaultError::Corrupt("encryption"))?;
        self.conn.execute(
            "INSERT INTO items (id, nonce, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET nonce = excluded.nonce, data = excluded.data",
            params![id, nonce.as_slice(), data],
        )?;
        Ok(())
    }

    /// Adds an item; returns its id.
    pub fn add(&mut self, data: ItemData) -> Result<String> {
        self.key()?;
        let id = new_id();
        let at = now();
        let envelope = Envelope { data, created: at, modified: at };
        self.write(&id, &envelope)?;
        self.items.push(Item { id: id.clone(), data: envelope.data, created: at, modified: at });
        self.sort();
        self.reindex();
        self.touch();
        Ok(id)
    }

    pub fn update(&mut self, id: &str, data: ItemData) -> Result<()> {
        self.key()?;
        let created = self.get(id).ok_or(VaultError::NoSuchItem)?.created;
        let envelope = Envelope { data, created, modified: now() };
        self.write(id, &envelope)?;
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.data = envelope.data;
            item.modified = envelope.modified;
        }
        self.sort();
        self.reindex();
        self.touch();
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.key()?;
        self.conn.execute("DELETE FROM items WHERE id = ?1", [id])?;
        self.items.retain(|i| i.id != id);
        self.reindex();
        self.touch();
        Ok(())
    }

    /// Logins that may be filled into a page at `page_url` (same registrable domain).
    pub fn logins_for(&self, page_url: &str) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|i| matches!(&i.data, ItemData::Login(l) if self.related.same_site(&l.site, page_url)))
            .collect()
    }

    /// Sets which domains share a sign-in (config: `passwords.related_domains`).
    pub fn set_related(&mut self, related: site::Related) {
        self.related = related;
    }

    /// Rewrites the site index from the (unlocked) items: one keyed tag per registrable
    /// domain with a login. Only tags are stored, never domains (see [`crypto::site_tag`]).
    fn reindex(&self) {
        let result = (|| -> Result<()> {
            let device = self.device.secret()?;
            let mut tags: Vec<[u8; crypto::SITE_TAG_LEN]> = Vec::new();
            for item in &self.items {
                if let ItemData::Login(l) = &item.data
                    && let Some(domain) = site::registrable(&l.site)
                {
                    let tag =
                        crypto::site_tag(&device, &domain).map_err(|_| VaultError::Corrupt("site index"))?;
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }
            self.set_meta("site_index", &tags.concat())?;
            *self.index.borrow_mut() = Some(tags);
            Ok(())
        })();
        if let Err(e) = result {
            tracing::warn!("updating the vault's site index failed: {e}");
        }
    }

    /// Whether the device key is loaded, so that [`Vault::site_known`] answers without waiting.
    pub fn device_ready(&self) -> bool {
        self.device.ready_secret().is_some()
    }

    /// Loads the device key on a thread of its own: unsealing it with the TPM takes most of a
    /// second, which the UI thread must not wait for. One load at a time; a failed one may be
    /// started again.
    pub fn load_device_key_in_background(&self) {
        if self.device_ready() || self.loading_device_key.swap(true, Ordering::AcqRel) {
            return;
        }
        let device = Arc::clone(&self.device);
        let loading = Arc::clone(&self.loading_device_key);
        let spawned = std::thread::Builder::new().name("vault-device-key".into()).spawn(move || {
            if let Err(e) = device.secret() {
                tracing::warn!("loading the device key failed: {e}");
            }
            loading.store(false, Ordering::Release);
        });
        if let Err(e) = spawned {
            self.loading_device_key.store(false, Ordering::Release);
            tracing::warn!("loading the device key failed: {e}");
        }
    }

    /// Whether a login is saved for the site of `page_url` (or a related one), answered
    /// without unlocking, from the keyed index. `None` if the vault has no index yet (it is
    /// made on the next unlock) or the device key isn't at hand: it never waits for the TPM
    /// (see [`Vault::load_device_key_in_background`]).
    pub fn site_known(&self, page_url: &str) -> Option<bool> {
        if self.index.borrow().is_none() {
            let raw = self.meta("site_index").ok().flatten()?;
            let tags = raw.as_chunks::<{ crypto::SITE_TAG_LEN }>().0.to_vec();
            *self.index.borrow_mut() = Some(tags);
        }
        let device = self.device.ready_secret()?;
        let index = self.index.borrow();
        let tags = index.as_ref()?;
        Some(
            self.related
                .domains(page_url)
                .iter()
                .any(|d| crypto::site_tag(&device, d).is_ok_and(|t| tags.contains(&t))),
        )
    }

    /// The saved login for this site and user name, if any.
    pub fn login_for(&self, page_url: &str, username: &str) -> Option<&Item> {
        self.logins_for(page_url)
            .into_iter()
            .find(|i| matches!(&i.data, ItemData::Login(l) if l.username == username))
    }

    /// Changes the master password: new salt and key (under the same device secret), every
    /// item re-encrypted.
    pub fn change_password(&mut self, old: &str, new: &str) -> Result<()> {
        self.key_for(old)?;
        if new.is_empty() {
            return Err(VaultError::EmptyPassword);
        }
        if !self.is_unlocked() {
            self.unlock(old)?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let key = self.write_key(new, self.kdf_params()?)?;
        self.rewrite_items(&key)?;
        tx.commit()?;
        self.key = Some(key);
        self.touch();
        Ok(())
    }

    /// Imports a CSV export from another browser or manager. Entries already in the vault
    /// (same site, user and password) are skipped. Returns how many were added.
    pub fn import_csv(&mut self, text: &str) -> Result<usize> {
        self.key()?;
        let mut added = 0;
        for data in import::items_from_csv(text) {
            if self.items.iter().any(|i| i.data == data) {
                continue;
            }
            self.add(data)?;
            added += 1;
        }
        Ok(added)
    }

    /// Writes a portable export (see [`export`]) to `path`: every item, encrypted with the
    /// master password alone, so it can be restored on another device. `password` must be the
    /// master password.
    pub fn export_encrypted(&self, path: &Path, password: &str) -> Result<()> {
        self.key()?;
        self.key_for(password)?;
        let bytes = export::seal(&self.items, password, self.kdf_params()?)?;
        platform::write_private(path, &bytes)?;
        Ok(())
    }

    /// Adds the items of an export (made with `password`) to this unlocked vault, encrypted
    /// for this device. Items already here (same contents) are skipped. Returns how many were
    /// added.
    pub fn import_export(&mut self, bytes: &[u8], password: &str) -> Result<usize> {
        let key = self.key()?.clone();
        let (items, _) = export::open(bytes, password)?;
        let tx = self.conn.unchecked_transaction()?;
        let added = self.insert(&key, items)?;
        tx.commit()?;
        let count = added.len();
        self.items.extend(added);
        self.sort();
        self.reindex();
        self.touch();
        Ok(count)
    }

    /// Sets up this new vault from an export: the export's password becomes the master
    /// password and its items are encrypted for this device. Leaves the vault unlocked;
    /// returns how many items were restored.
    pub fn setup_from_export(&mut self, bytes: &[u8], password: &str) -> Result<usize> {
        if self.is_set_up() {
            return Err(VaultError::AlreadySetUp);
        }
        let (items, params) = export::open(bytes, password)?;
        self.items.clear();
        let tx = self.conn.unchecked_transaction()?;
        let key = self.write_key(password, params)?;
        let added = self.insert(&key, items)?;
        tx.commit()?;
        let count = added.len();
        self.items = added;
        self.sort();
        self.key = Some(key);
        self.reindex();
        self.touch();
        Ok(count)
    }

    /// Writes the items that aren't here yet with `key` (keeping their ids unless taken) and
    /// returns them; the caller commits and adds them to memory.
    fn insert(&self, key: &Key, items: Vec<Item>) -> Result<Vec<Item>> {
        let mut added: Vec<Item> = Vec::new();
        for mut item in items {
            if self.items.iter().chain(&added).any(|i| i.data == item.data) {
                continue;
            }
            if item.id.is_empty() || self.items.iter().chain(&added).any(|i| i.id == item.id) {
                item.id = new_id();
            }
            let envelope =
                Envelope { data: item.data.clone(), created: item.created, modified: item.modified };
            self.write_with(key, &item.id, &envelope)?;
            added.push(item);
        }
        Ok(added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two fake devices.
    fn device_a() -> Arc<dyn DeviceKey> {
        Arc::new(MemoryDeviceKey::new([0xa1; 32]))
    }

    fn device_b() -> Arc<dyn DeviceKey> {
        Arc::new(MemoryDeviceKey::new([0xb2; 32]))
    }

    /// A device whose key can't be read (e.g. the TPM refuses).
    struct BrokenDevice;

    impl DeviceKey for BrokenDevice {
        fn secret(&self) -> std::result::Result<device::DeviceSecret, DeviceKeyError> {
            Err(DeviceKeyError::Tpm("test: no TPM".into()))
        }
    }

    fn scratch_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("axion-vault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// A vault as Axion wrote it before device binding (format 1), with `items`.
    fn password_only_vault(conn_path: Option<&Path>, items: &[ItemData]) -> Vault {
        let v = match conn_path {
            Some(path) => Vault::open(path, device_a()).unwrap(),
            None => Vault::in_memory(device_a()).unwrap(),
        };
        let salt = [3; SALT_LEN];
        let key = crypto::derive_key("test-only-pass", &salt, KdfParams::TEST).unwrap();
        let (nonce, check) = crypto::seal(&key, CHECK_PLAINTEXT_V1, CHECK_AAD).unwrap();
        v.set_meta("salt", &salt).unwrap();
        v.set_meta("kdf", b"256,1,1").unwrap();
        v.set_meta("check_nonce", &nonce).unwrap();
        v.set_meta("check", &check).unwrap();
        for (i, data) in items.iter().enumerate() {
            let envelope = Envelope { data: data.clone(), created: i as i64, modified: i as i64 };
            v.write_with(&key, &format!("old-{i}"), &envelope).unwrap();
        }
        v
    }

    fn vault() -> Vault {
        let mut v = Vault::in_memory(device_a()).unwrap();
        v.setup_with("test-only-pass", KdfParams::TEST).unwrap();
        v
    }

    fn login(site: &str, user: &str, pw: &str) -> ItemData {
        ItemData::Login(Login {
            site: site.into(),
            username: user.into(),
            password: pw.into(),
            notes: String::new(),
        })
    }

    #[test]
    fn the_site_index_answers_while_locked() {
        let mut v = vault();
        v.set_related(site::Related::new(&[vec!["google.com", "youtube.com"]]));
        v.add(login("https://accounts.google.com", "me", "pw")).unwrap();
        assert_eq!(v.site_known("https://youtube.com/"), Some(true));
        v.lock();
        assert_eq!(v.site_known("https://login.google.com/x"), Some(true));
        assert_eq!(v.site_known("https://youtube.com/"), Some(true));
        assert_eq!(v.site_known("https://example.org/"), Some(false));
        // The stored index doesn't contain the domain.
        let raw = v.meta("site_index").unwrap().unwrap();
        assert_eq!(raw.len(), crypto::SITE_TAG_LEN);
        assert!(!raw.windows(6).any(|w| w == b"google"));
        // Deleting the last login empties it.
        v.unlock("test-only-pass").unwrap();
        let id = v.items()[0].id.clone();
        v.delete(&id).unwrap();
        v.lock();
        assert_eq!(v.site_known("https://youtube.com/"), Some(false));
    }

    /// A device key that takes a while to load, like the TPM's.
    struct SlowDevice {
        secret: [u8; crypto::KEY_LEN],
        loaded: std::sync::Mutex<bool>,
    }

    impl DeviceKey for SlowDevice {
        fn secret(&self) -> std::result::Result<device::DeviceSecret, DeviceKeyError> {
            let mut loaded = self.loaded.lock().unwrap();
            if !*loaded {
                std::thread::sleep(Duration::from_millis(300));
                *loaded = true;
            }
            Ok(zeroize::Zeroizing::new(self.secret))
        }

        fn ready_secret(&self) -> Option<device::DeviceSecret> {
            (*self.loaded.try_lock().ok()?).then(|| zeroize::Zeroizing::new(self.secret))
        }
    }

    /// Regression: the locked-state check on a sign-in form unsealed the device key with the
    /// TPM on the UI thread (most of a second, every time while the TPM failed).
    #[test]
    fn the_site_check_never_waits_for_the_device_key() {
        let path = scratch_file("slow-device");
        let slow = || Arc::new(SlowDevice { secret: [7; crypto::KEY_LEN], loaded: Default::default() });
        {
            let mut v = Vault::open(&path, slow()).unwrap();
            v.setup_with("test-only-pass", KdfParams::TEST).unwrap();
            v.add(login("https://example.org", "me", "pw")).unwrap();
        }
        let v = Vault::open(&path, slow()).unwrap();
        assert!(!v.device_ready());
        let started = Instant::now();
        assert_eq!(v.site_known("https://example.org/"), None, "not loaded: no answer yet");
        assert!(started.elapsed() < Duration::from_millis(200), "it waited for the device key");
        v.load_device_key_in_background();
        v.load_device_key_in_background();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !v.device_ready() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(v.device_ready());
        assert_eq!(v.site_known("https://example.org/"), Some(true));
        assert_eq!(v.site_known("https://example.com/"), Some(false));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_vault_without_an_index_doesnt_know() {
        let v = vault();
        v.conn.execute("DELETE FROM meta WHERE key = 'site_index'", []).unwrap();
        *v.index.borrow_mut() = None;
        assert_eq!(v.site_known("https://example.org/"), None);
    }

    #[test]
    fn round_trip_through_the_file() {
        let path = std::env::temp_dir().join(format!("axion-vault-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut v = Vault::open(&path, device_a()).unwrap();
            assert!(!v.is_set_up());
            v.setup_with("test-only-pass", KdfParams::TEST).unwrap();
            v.add(login("example.com", "me", "test-only-secret")).unwrap();
            v.add(ItemData::Card(Card { number: "4111111111111111".into(), ..Default::default() })).unwrap();
        }
        // Nothing readable on disk.
        let raw = std::fs::read(&path).unwrap();
        let raw = String::from_utf8_lossy(&raw);
        assert!(
            !raw.contains("test-only-secret")
                && !raw.contains("4111111111111111")
                && !raw.contains("example.com")
        );

        let mut v = Vault::open(&path, device_a()).unwrap();
        assert!(v.is_set_up() && !v.is_unlocked());
        assert!(v.items().is_empty());
        assert!(matches!(v.unlock("wrong"), Err(VaultError::WrongPassword)));
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.items().len(), 2);
        assert_eq!(v.logins_for("https://login.example.com/").len(), 1);
        assert!(v.logins_for("https://example.org/").is_empty());
        v.lock();
        assert!(v.items().is_empty() && matches!(v.add(login("a", "b", "c")), Err(VaultError::Locked)));
        drop(v);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn verify_leaves_the_state_alone() {
        let mut v = vault();
        assert!(v.is_unlocked());
        assert!(matches!(v.verify("wrong"), Err(VaultError::WrongPassword)));
        v.verify("test-only-pass").unwrap();
        assert!(v.is_unlocked());
        v.lock();
        v.verify("test-only-pass").unwrap();
        assert!(!v.is_unlocked());
        assert!(matches!(Vault::in_memory(device_a()).unwrap().verify("x"), Err(VaultError::NotSetUp)));
    }

    #[test]
    fn tampering_is_detected() {
        let mut v = vault();
        let a = v.add(login("a.com", "u", "pw-a")).unwrap();
        let b = v.add(login("b.com", "u", "pw-b")).unwrap();
        // Moving b's ciphertext onto a fails authentication (the id is part of it) …
        v.conn
            .execute("UPDATE items SET nonce = (SELECT nonce FROM items WHERE id = ?2), data = (SELECT data FROM items WHERE id = ?2) WHERE id = ?1", [&a, &b])
            .unwrap();
        v.lock();
        assert!(matches!(v.unlock("test-only-pass"), Err(VaultError::Corrupt(_))));
        // … and so does a flipped bit.
        let mut v = vault();
        let id = v.add(login("a.com", "u", "pw")).unwrap();
        let mut data: Vec<u8> =
            v.conn.query_row("SELECT data FROM items WHERE id = ?1", [&id], |r| r.get(0)).unwrap();
        data[5] ^= 1;
        v.conn.execute("UPDATE items SET data = ?2 WHERE id = ?1", params![id, data]).unwrap();
        v.lock();
        assert!(matches!(v.unlock("test-only-pass"), Err(VaultError::Corrupt(_))));
    }

    #[test]
    fn update_delete_change_password_import() {
        let mut v = vault();
        let id = v.add(login("a.com", "u", "old")).unwrap();
        v.update(&id, login("a.com", "u", "new")).unwrap();
        assert!(matches!(&v.get(&id).unwrap().data, ItemData::Login(l) if l.password == "new"));
        v.change_password("test-only-pass", "test-only-pass-2").unwrap();
        v.lock();
        assert!(matches!(v.unlock("test-only-pass"), Err(VaultError::WrongPassword)));
        v.unlock("test-only-pass-2").unwrap();
        assert_eq!(v.items().len(), 1);
        v.delete(&id).unwrap();
        assert!(v.items().is_empty());
        let csv = "name,url,username,password,note\nx,https://x.org,me,pw,\n";
        assert_eq!(v.import_csv(csv).unwrap(), 1);
        assert_eq!(v.import_csv(csv).unwrap(), 0, "duplicates are skipped");
    }

    #[test]
    fn auto_lock() {
        let mut v = vault();
        assert!(!v.lock_if_idle(Duration::from_secs(3600)));
        assert!(v.lock_if_idle(Duration::ZERO));
        assert!(!v.is_unlocked());
    }

    #[test]
    fn a_copied_vault_doesnt_open_on_another_device() {
        let path = scratch_file("copied.sqlite");
        {
            let mut v = Vault::open(&path, device_a()).unwrap();
            v.setup_with("test-only-pass", KdfParams::TEST).unwrap();
            v.add(login("a.com", "u", "pw")).unwrap();
        }
        // The same file (a copy of vault.sqlite, or of the whole profile) on device B.
        let mut b = Vault::open(&path, device_b()).unwrap();
        assert!(b.is_set_up());
        assert!(matches!(b.unlock("test-only-pass"), Err(VaultError::OtherDevice)));
        assert!(matches!(b.unlock("wrong"), Err(VaultError::OtherDevice)));
        assert!(matches!(b.verify("test-only-pass"), Err(VaultError::OtherDevice)));
        assert!(matches!(b.change_password("test-only-pass", "x"), Err(VaultError::OtherDevice)));
        assert!(!b.is_unlocked());
        // Back on A it still opens, and a wrong password is still just that.
        let mut a = Vault::open(&path, device_a()).unwrap();
        assert!(matches!(a.unlock("wrong"), Err(VaultError::WrongPassword)));
        a.unlock("test-only-pass").unwrap();
        assert_eq!(a.items().len(), 1);
        // A device whose key is unavailable says so (not "wrong password").
        let broken = Vault::open(&path, Arc::new(BrokenDevice)).unwrap();
        assert!(matches!(broken.verify("test-only-pass"), Err(VaultError::DeviceKey(_))));
        drop((a, b, broken));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn export_on_a_restore_on_b() {
        let mut a = vault();
        a.add(login("a.com", "u", "test-only-secret")).unwrap();
        a.add(ItemData::Note(Note { title: "n".into(), text: "t".into() })).unwrap();
        let path = scratch_file("export.json");
        // Only unlocked, and only with the master password.
        assert!(matches!(a.export_encrypted(&path, "wrong"), Err(VaultError::WrongPassword)));
        a.lock();
        assert!(matches!(a.export_encrypted(&path, "test-only-pass"), Err(VaultError::Locked)));
        a.unlock("test-only-pass").unwrap();
        a.export_encrypted(&path, "test-only-pass").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("test-only-secret"));

        // A new vault on device B, from the export: the export's password is its password.
        let mut b = Vault::in_memory(device_b()).unwrap();
        assert!(matches!(b.setup_from_export(&bytes, "wrong"), Err(VaultError::WrongPassword)));
        assert!(!b.is_set_up());
        assert_eq!(b.setup_from_export(&bytes, "test-only-pass").unwrap(), 2);
        assert!(b.is_unlocked());
        b.lock();
        b.unlock("test-only-pass").unwrap();
        assert_eq!(b.items(), a.items(), "ids, contents and dates survive");
        assert_eq!(b.format().unwrap(), FORMAT_DEVICE_BOUND);
        assert!(matches!(b.setup_from_export(&bytes, "test-only-pass"), Err(VaultError::AlreadySetUp)));

        // Into an existing vault on B with another password: new items only.
        let mut c = Vault::in_memory(device_b()).unwrap();
        c.setup_with("test-only-other", KdfParams::TEST).unwrap();
        c.add(login("a.com", "u", "test-only-secret")).unwrap();
        assert!(matches!(c.import_export(&bytes, "test-only-other"), Err(VaultError::WrongPassword)));
        assert_eq!(c.import_export(&bytes, "test-only-pass").unwrap(), 1, "the login is already there");
        assert_eq!(c.import_export(&bytes, "test-only-pass").unwrap(), 0);
        c.lock();
        c.unlock("test-only-other").unwrap();
        assert_eq!(c.items().len(), 2);
        assert!(matches!(c.import_export(b"name,url\n", "x"), Err(VaultError::NotAnExport)));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn password_only_vaults_are_upgraded_on_unlock() {
        let path = scratch_file("upgrade.sqlite");
        let items = [login("a.com", "u", "pw"), ItemData::Note(Note { title: "n".into(), text: "t".into() })];
        let mut v = password_only_vault(Some(&path), &items);
        assert_eq!(v.format().unwrap(), FORMAT_PASSWORD_ONLY);
        // A wrong password changes nothing.
        assert!(matches!(v.unlock("wrong"), Err(VaultError::WrongPassword)));
        assert_eq!(v.format().unwrap(), FORMAT_PASSWORD_ONLY);
        // verify doesn't upgrade either.
        v.verify("test-only-pass").unwrap();
        assert_eq!(v.format().unwrap(), FORMAT_PASSWORD_ONLY);
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.format().unwrap(), FORMAT_DEVICE_BOUND);
        assert_eq!(v.items().len(), 2);
        let before = v.items().to_vec();
        // Writes after the upgrade use the new key.
        v.add(login("b.com", "u", "pw")).unwrap();
        drop(v);
        let mut v = Vault::open(&path, device_a()).unwrap();
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.items().len(), 3);
        assert!(before.iter().all(|i| v.items().contains(i)), "ids, contents and dates survive");
        // Now it is bound to device A.
        let mut b = Vault::open(&path, device_b()).unwrap();
        assert!(matches!(b.unlock("test-only-pass"), Err(VaultError::OtherDevice)));
        drop((v, b));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_failed_upgrade_keeps_the_old_format() {
        let items = [login("a.com", "u", "pw")];
        let mut v = password_only_vault(None, &items);
        let raw = |v: &Vault| -> Vec<(String, Vec<u8>, Vec<u8>)> {
            let mut stmt = v.conn.prepare("SELECT id, nonce, data FROM items ORDER BY id").unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let meta = |v: &Vault| (v.meta("salt").unwrap(), v.meta("check").unwrap(), v.meta("format").unwrap());
        let (raw_before, meta_before) = (raw(&v), meta(&v));
        v.fail_upgrade = true;
        // It still opens (with the old key) …
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.items().len(), 1);
        // … and nothing on disk changed.
        assert_eq!(v.format().unwrap(), FORMAT_PASSWORD_ONLY);
        assert_eq!(raw(&v), raw_before);
        assert_eq!(meta(&v), meta_before);
        // Edits while in the old format stay readable.
        v.add(login("b.com", "u", "pw")).unwrap();
        v.lock();
        v.fail_upgrade = false;
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.format().unwrap(), FORMAT_DEVICE_BOUND);
        assert_eq!(v.items().len(), 2);
        v.lock();
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.items().len(), 2);
    }

    #[test]
    fn upgrade_waits_for_the_device_key() {
        let v = password_only_vault(None, &[login("a.com", "u", "pw")]);
        let Vault { conn, .. } = v;
        let mut v = Vault::init(conn, Arc::new(BrokenDevice)).unwrap();
        v.unlock("test-only-pass").unwrap();
        assert_eq!(v.items().len(), 1);
        assert_eq!(v.format().unwrap(), FORMAT_PASSWORD_ONLY);
    }

    #[test]
    fn change_password_stays_on_this_device() {
        let path = scratch_file("change.sqlite");
        {
            let mut v = Vault::open(&path, device_a()).unwrap();
            v.setup_with("test-only-pass", KdfParams::TEST).unwrap();
            v.add(login("a.com", "u", "pw")).unwrap();
            v.lock();
            v.change_password("test-only-pass", "test-only-pass-2").unwrap();
        }
        let mut b = Vault::open(&path, device_b()).unwrap();
        assert!(matches!(b.unlock("test-only-pass-2"), Err(VaultError::OtherDevice)));
        let mut a = Vault::open(&path, device_a()).unwrap();
        assert!(matches!(a.unlock("test-only-pass"), Err(VaultError::WrongPassword)));
        a.unlock("test-only-pass-2").unwrap();
        assert_eq!(a.items().len(), 1);
        drop((a, b));
        std::fs::remove_file(&path).unwrap();
    }
}
