//! Axion's password manager: logins, cards, addresses and secure notes in an encrypted vault.
//!
//! The vault is its own SQLite file in the profile. A key is derived from the master password
//! with Argon2id (random salt, cost stored with the vault); every item is encrypted on its own
//! with XChaCha20-Poly1305, its id authenticated alongside so ciphertexts can't be swapped.
//! A known value encrypted at setup tells a wrong password from a damaged vault.
//!
//! While unlocked, the key and the decrypted items live only in memory; locking (on demand or
//! after inactivity) wipes both. Nothing secret is ever logged.

pub mod autofill;
mod crypto;
pub mod generator;
pub mod import;
mod item;
pub mod site;

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use crypto::KdfParams;
pub use item::{Address, Card, Item, ItemData, Kind, Login, Note};

use crypto::{Key, SALT_LEN};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);
    CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, nonce BLOB NOT NULL, data BLOB NOT NULL);
";
/// Encrypted at setup; decrypting it proves the password.
const CHECK_PLAINTEXT: &[u8] = b"axion-vault-v1";
const CHECK_AAD: &[u8] = b"check";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("wrong password")]
    WrongPassword,
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
    key: Option<Key>,
    items: Vec<Item>,
    last_activity: Instant,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn new_id() -> String {
    crypto::random_bytes::<16>().iter().map(|b| format!("{b:02x}")).collect()
}

impl Vault {
    /// Opens (or creates) the vault file. It starts locked.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn, key: None, items: Vec::new(), last_activity: Instant::now() })
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
        self.touch();
        Ok(())
    }

    /// Derives a new key with a fresh salt and stores salt, cost and check value.
    fn write_key(&self, password: &str, params: KdfParams) -> Result<Key> {
        if password.is_empty() {
            return Err(VaultError::EmptyPassword);
        }
        let salt = crypto::random_bytes::<SALT_LEN>();
        let key =
            crypto::derive_key(password, &salt, params).map_err(|_| VaultError::Corrupt("key derivation"))?;
        let (nonce, check) =
            crypto::seal(&key, CHECK_PLAINTEXT, CHECK_AAD).map_err(|_| VaultError::Corrupt("encryption"))?;
        let kdf = format!("{},{},{}", params.memory, params.iterations, params.parallelism);
        self.set_meta("salt", &salt)?;
        self.set_meta("kdf", kdf.as_bytes())?;
        self.set_meta("check_nonce", &nonce)?;
        self.set_meta("check", &check)?;
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

    /// Derives the key from `password` and checks it, without unlocking.
    fn key_for(&self, password: &str) -> Result<Key> {
        let salt = self.meta("salt")?.ok_or(VaultError::NotSetUp)?;
        let key = crypto::derive_key(password, &salt, self.kdf_params()?)
            .map_err(|_| VaultError::Corrupt("key derivation"))?;
        let nonce = self.meta("check_nonce")?.ok_or(VaultError::Corrupt("check"))?;
        let check = self.meta("check")?.ok_or(VaultError::Corrupt("check"))?;
        match crypto::open(&key, &nonce, &check, CHECK_AAD) {
            Ok(plain) if plain.as_slice() == CHECK_PLAINTEXT => Ok(key),
            _ => Err(VaultError::WrongPassword),
        }
    }

    /// Unlocks with the master password and decrypts the items.
    pub fn unlock(&mut self, password: &str) -> Result<()> {
        let key = self.key_for(password)?;
        let mut stmt = self.conn.prepare("SELECT id, nonce, data FROM items")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (id, nonce, data) = row?;
            let plain = crypto::open(&key, &nonce, &data, id.as_bytes())
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
        drop(stmt);
        self.key = Some(key);
        self.items = items;
        self.sort();
        self.last_activity = Instant::now();
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
        self.touch();
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.key()?;
        self.conn.execute("DELETE FROM items WHERE id = ?1", [id])?;
        self.items.retain(|i| i.id != id);
        self.touch();
        Ok(())
    }

    /// Logins that may be filled into a page at `page_url` (same registrable domain).
    pub fn logins_for(&self, page_url: &str) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|i| matches!(&i.data, ItemData::Login(l) if site::same_site(&l.site, page_url)))
            .collect()
    }

    /// The saved login for this site and user name, if any.
    pub fn login_for(&self, page_url: &str, username: &str) -> Option<&Item> {
        self.logins_for(page_url)
            .into_iter()
            .find(|i| matches!(&i.data, ItemData::Login(l) if l.username == username))
    }

    /// Changes the master password: new salt and key, every item re-encrypted.
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
        for item in &self.items {
            let envelope =
                Envelope { data: item.data.clone(), created: item.created, modified: item.modified };
            self.write_with(&key, &item.id, &envelope)?;
        }
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

    /// Writes an encrypted copy of the vault (same master password) to `path`.
    pub fn export_encrypted(&self, path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path).map_err(|_| VaultError::Corrupt("can't replace the export file"))?;
        }
        self.conn.execute("VACUUM INTO ?1", [path.to_string_lossy()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        let mut v = Vault::in_memory().unwrap();
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
    fn round_trip_through_the_file() {
        let path = std::env::temp_dir().join(format!("axion-vault-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut v = Vault::open(&path).unwrap();
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

        let mut v = Vault::open(&path).unwrap();
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
}
