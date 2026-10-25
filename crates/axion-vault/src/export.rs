//! The portable export: every item, encrypted with the master password alone (no device key),
//! so it can be restored on a new device.
//!
//! The file is JSON:
//!
//! ```text
//! { "format": "axion-vault-export", "version": 1,
//!   "kdf": { "memory": KiB, "iterations": n, "parallelism": n },
//!   "salt": hex(16), "nonce": hex(24), "data": hex(ciphertext) }
//! ```
//!
//! `key = Argon2id(password, salt, kdf)` with a fresh salt per export, and `data` is
//! XChaCha20-Poly1305 over the JSON list of items (id, data, created, modified). The
//! associated data is `axion-vault-export/1/<memory>,<iterations>,<parallelism>/<salt hex>`,
//! so the header can't be changed without the decryption failing.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams, NONCE_LEN, SALT_LEN};
use crate::item::{Item, ItemData};
use crate::{Result, VaultError};

pub const FORMAT: &str = "axion-vault-export";
pub const VERSION: u32 = 1;
/// Limits on the cost an export may ask for (a crafted file mustn't exhaust memory).
const MAX_MEMORY_KIB: u32 = 4 * 1024 * 1024;
const MAX_ITERATIONS: u32 = 64;
const MAX_PARALLELISM: u32 = 64;

#[derive(Serialize, Deserialize)]
struct Kdf {
    memory: u32,
    iterations: u32,
    parallelism: u32,
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    kdf: Kdf,
    salt: String,
    nonce: String,
    data: String,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    id: String,
    data: ItemData,
    created: i64,
    modified: i64,
}

fn aad(params: KdfParams, salt: &str) -> Vec<u8> {
    format!("{FORMAT}/{VERSION}/{},{},{}/{salt}", params.memory, params.iterations, params.parallelism)
        .into_bytes()
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

/// Encrypts `items` with `password` (fresh salt).
pub fn seal(items: &[Item], password: &str, params: KdfParams) -> Result<Vec<u8>> {
    let entries: Vec<Entry> = items
        .iter()
        .map(|i| Entry { id: i.id.clone(), data: i.data.clone(), created: i.created, modified: i.modified })
        .collect();
    let plain = Zeroizing::new(serde_json::to_vec(&entries).map_err(|_| VaultError::Corrupt("serialize"))?);
    drop(entries);
    let salt = crypto::random_bytes::<SALT_LEN>();
    let salt_hex = hex(&salt);
    let key =
        crypto::derive_key(password, &salt, params).map_err(|_| VaultError::Corrupt("key derivation"))?;
    let (nonce, data) =
        crypto::seal(&key, &plain, &aad(params, &salt_hex)).map_err(|_| VaultError::Corrupt("encryption"))?;
    let file = File {
        format: FORMAT.to_owned(),
        version: VERSION,
        kdf: Kdf { memory: params.memory, iterations: params.iterations, parallelism: params.parallelism },
        salt: salt_hex,
        nonce: hex(&nonce),
        data: hex(&data),
    };
    serde_json::to_vec_pretty(&file).map_err(|_| VaultError::Corrupt("serialize"))
}

/// Decrypts an export: its items and the cost it was made with.
pub fn open(bytes: &[u8], password: &str) -> Result<(Vec<Item>, KdfParams)> {
    let file: File = serde_json::from_slice(bytes).map_err(|_| VaultError::NotAnExport)?;
    if file.format != FORMAT {
        return Err(VaultError::NotAnExport);
    }
    if file.version != VERSION {
        return Err(VaultError::UnsupportedExport(file.version));
    }
    let params = KdfParams {
        memory: file.kdf.memory,
        iterations: file.kdf.iterations,
        parallelism: file.kdf.parallelism,
    };
    if params.memory > MAX_MEMORY_KIB
        || params.iterations > MAX_ITERATIONS
        || params.parallelism > MAX_PARALLELISM
    {
        return Err(VaultError::Corrupt("the export asks for an absurd key derivation cost"));
    }
    let salt = unhex(&file.salt).filter(|s| s.len() == SALT_LEN).ok_or(VaultError::Corrupt("export salt"))?;
    let nonce =
        unhex(&file.nonce).filter(|n| n.len() == NONCE_LEN).ok_or(VaultError::Corrupt("export nonce"))?;
    let data = unhex(&file.data).ok_or(VaultError::Corrupt("export data"))?;
    let key =
        crypto::derive_key(password, &salt, params).map_err(|_| VaultError::Corrupt("key derivation"))?;
    // AEAD can't tell a wrong password from a changed file; the password is far likelier.
    let plain =
        crypto::open(&key, &nonce, &data, &aad(params, &file.salt)).map_err(|_| VaultError::WrongPassword)?;
    let entries: Vec<Entry> =
        serde_json::from_slice(&plain).map_err(|_| VaultError::Corrupt("the export's items don't parse"))?;
    let items = entries
        .into_iter()
        .map(|e| Item { id: e.id, data: e.data, created: e.created, modified: e.modified })
        .collect();
    Ok((items, params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Note;

    fn note(text: &str) -> Item {
        Item {
            id: "n1".into(),
            data: ItemData::Note(Note { title: "t".into(), text: text.into() }),
            created: 1,
            modified: 2,
        }
    }

    #[test]
    fn round_trip_and_failures() {
        let bytes = seal(&[note("test-only-secret")], "test-only-pass", KdfParams::TEST).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("test-only-secret"));
        let (items, params) = open(&bytes, "test-only-pass").unwrap();
        assert_eq!(items, vec![note("test-only-secret")]);
        assert_eq!(params, KdfParams::TEST);
        assert!(matches!(open(&bytes, "wrong"), Err(VaultError::WrongPassword)));
        assert!(matches!(open(b"name,url\n", "x"), Err(VaultError::NotAnExport)));
        // The header is authenticated: a changed cost fails like a wrong password.
        let text = String::from_utf8(bytes.clone()).unwrap();
        let changed = text.replace("\"iterations\": 1", "\"iterations\": 2");
        assert_ne!(changed, text);
        assert!(matches!(open(changed.as_bytes(), "test-only-pass"), Err(VaultError::WrongPassword)));
        let newer = text.replace("\"version\": 1", "\"version\": 2");
        assert!(matches!(open(newer.as_bytes(), "test-only-pass"), Err(VaultError::UnsupportedExport(2))));
        // Two exports never share a salt.
        let again = seal(&[note("x")], "test-only-pass", KdfParams::TEST).unwrap();
        let salt = |b: &[u8]| serde_json::from_slice::<File>(b).unwrap().salt;
        assert_ne!(salt(&bytes), salt(&again));
    }
}
