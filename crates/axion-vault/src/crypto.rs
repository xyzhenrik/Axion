//! Key derivation (Argon2id, HKDF-SHA256) and authenticated encryption (XChaCha20-Poly1305).

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, OsRng, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 24;

/// Argon2id cost. Stored with the vault so it can be raised later without breaking old vaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub memory: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// OWASP's recommendation for Argon2id (64 MiB, 3 passes): about half a second to unlock.
    pub const DEFAULT: Self = Self { memory: 64 * 1024, iterations: 3, parallelism: 1 };
    /// Cheap parameters for tests only.
    pub const TEST: Self = Self { memory: 256, iterations: 1, parallelism: 1 };
}

/// The vault key, wiped from memory when dropped.
pub type Key = Zeroizing<[u8; KEY_LEN]>;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("key derivation failed")]
    Kdf,
    /// Wrong key, or the data was changed.
    #[error("decryption failed")]
    Decrypt,
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// Derives the vault key from the master password.
pub fn derive_key(password: &str, salt: &[u8], params: KdfParams) -> Result<Key, CryptoError> {
    let params = Params::new(params.memory, params.iterations, params.parallelism, Some(KEY_LEN))
        .map_err(|_| CryptoError::Kdf)?;
    let mut key = Zeroizing::new([0; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| CryptoError::Kdf)?;
    Ok(key)
}

/// HKDF `info` for the device-bound vault key.
const VAULT_KEY_INFO: &[u8] = b"axion-vault/v2/vault-key";
/// HKDF `info` for the value that recognises the device.
const DEVICE_CHECK_INFO: &[u8] = b"axion-vault/v2/device-check";

/// The device-bound vault key: HKDF-SHA256 with the vault's salt over the password key and
/// the device secret. Without either, the key can't be computed.
pub fn combine(password_key: &Key, device_secret: &[u8; KEY_LEN], salt: &[u8]) -> Result<Key, CryptoError> {
    let mut ikm = Zeroizing::new([0; 2 * KEY_LEN]);
    ikm[..KEY_LEN].copy_from_slice(password_key.as_ref());
    ikm[KEY_LEN..].copy_from_slice(device_secret);
    let mut key = Zeroizing::new([0; KEY_LEN]);
    Hkdf::<Sha256>::new(Some(salt), ikm.as_ref())
        .expand(VAULT_KEY_INFO, key.as_mut())
        .map_err(|_| CryptoError::Kdf)?;
    Ok(key)
}

/// A value stored with the vault that tells whether this is the device it was made on, so a
/// wrong device isn't mistaken for a wrong password. It reveals nothing about the secret
/// (HKDF output; salted per vault, so vaults of one device can't be linked by it).
pub fn device_check(device_secret: &[u8; KEY_LEN], salt: &[u8]) -> Result<[u8; KEY_LEN], CryptoError> {
    let mut check = [0; KEY_LEN];
    Hkdf::<Sha256>::new(Some(salt), device_secret)
        .expand(DEVICE_CHECK_INFO, &mut check)
        .map_err(|_| CryptoError::Kdf)?;
    Ok(check)
}

/// Encrypts `plaintext`; `aad` (e.g. the item's id) is authenticated but not encrypted, so a
/// ciphertext can't be moved to another item. Returns (nonce, ciphertext).
pub fn seal(key: &Key, plaintext: &[u8], aad: &[u8]) -> Result<([u8; NONCE_LEN], Vec<u8>), CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    let nonce = random_bytes::<NONCE_LEN>();
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| CryptoError::Decrypt)?;
    Ok((nonce, ciphertext))
}

pub fn open(
    key: &Key,
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if nonce.len() != NONCE_LEN {
        return Err(CryptoError::Decrypt);
    }
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip() {
        let key = derive_key("test-only-pass", &[7; SALT_LEN], KdfParams::TEST).unwrap();
        let (nonce, ct) = seal(&key, b"secret", b"id-1").unwrap();
        assert_eq!(open(&key, &nonce, &ct, b"id-1").unwrap().as_slice(), b"secret");
        // Another id (aad), another key or a flipped bit all fail.
        assert!(open(&key, &nonce, &ct, b"id-2").is_err());
        let other = derive_key("wrong", &[7; SALT_LEN], KdfParams::TEST).unwrap();
        assert!(open(&other, &nonce, &ct, b"id-1").is_err());
        let mut tampered = ct.clone();
        tampered[0] ^= 1;
        assert!(open(&key, &nonce, &tampered, b"id-1").is_err());
    }

    #[test]
    fn combined_key_needs_both_factors() {
        let pw = derive_key("pw", &[1; SALT_LEN], KdfParams::TEST).unwrap();
        let other_pw = derive_key("pw2", &[1; SALT_LEN], KdfParams::TEST).unwrap();
        let salt = [1; SALT_LEN];
        let k = combine(&pw, &[9; KEY_LEN], &salt).unwrap();
        assert_eq!(*k, *combine(&pw, &[9; KEY_LEN], &salt).unwrap());
        assert_ne!(*k, *combine(&pw, &[8; KEY_LEN], &salt).unwrap());
        assert_ne!(*k, *combine(&other_pw, &[9; KEY_LEN], &salt).unwrap());
        assert_ne!(*k, *pw);
        let check = device_check(&[9; KEY_LEN], &salt).unwrap();
        assert_ne!(check, device_check(&[8; KEY_LEN], &salt).unwrap());
        assert_ne!(check, device_check(&[9; KEY_LEN], &[2; SALT_LEN]).unwrap());
        assert_ne!(check, *k);
    }

    #[test]
    fn same_password_same_key_other_salt_other_key() {
        let a = derive_key("pw", &[1; SALT_LEN], KdfParams::TEST).unwrap();
        let b = derive_key("pw", &[1; SALT_LEN], KdfParams::TEST).unwrap();
        let c = derive_key("pw", &[2; SALT_LEN], KdfParams::TEST).unwrap();
        assert_eq!(*a, *b);
        assert_ne!(*a, *c);
    }
}
