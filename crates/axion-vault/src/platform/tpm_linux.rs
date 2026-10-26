//! Sealing the device key with a TPM 2.0 through the kernel's resource manager (`/dev/tpmrm0`).
//!
//! Objects (all transient; nothing is persisted in the TPM, so nothing needs cleaning up):
//! - a primary storage key under the owner hierarchy from the standard ECC P-256 SRK template
//!   (AES-128-CFB). A primary key is derived from the hierarchy's seed and the template, so the
//!   same TPM recreates the same key on every start, and no other TPM can.
//! - a sealed data object (keyed hash, no scheme) under it holding the 32-byte secret:
//!   `fixedTPM | fixedParent | userWithAuth | noDA`, empty auth, no PCR policy (a firmware or
//!   kernel update must not lock the user out of their passwords).
//!
//! The sealed object's public and private parts (the private part is encrypted by the TPM
//! under the storage key) form the blob stored in `device.tpm`. Every command runs in an HMAC
//! session salted with the storage key, with parameter encryption both ways, so the secret
//! never crosses the bus in clear.

use std::str::FromStr;

use tss_esapi::attributes::{ObjectAttributesBuilder, SessionAttributesBuilder};
use tss_esapi::constants::SessionType;
use tss_esapi::handles::{KeyHandle, ObjectHandle, SessionHandle};
use tss_esapi::interface_types::algorithm::{HashingAlgorithm, PublicAlgorithm};
use tss_esapi::interface_types::ecc::EccCurve;
use tss_esapi::interface_types::resource_handles::Hierarchy;
use tss_esapi::structures::{
    EccParameter, EccPoint, EccScheme, KeyDerivationFunctionScheme, KeyedHashScheme, Private, Public,
    PublicBuilder, PublicEccParametersBuilder, PublicKeyedHashParameters, SensitiveData, SymmetricDefinition,
    SymmetricDefinitionObject,
};
use tss_esapi::tcti_ldr::DeviceConfig;
use tss_esapi::traits::{Marshall, UnMarshall};
use tss_esapi::{Context, TctiNameConf};
use zeroize::Zeroizing;

use crate::crypto::KEY_LEN;
use crate::device::{DeviceKeyError, DeviceSecret};

/// The kernel's TPM resource manager (shared between processes, unlike `/dev/tpm0`).
const DEVICE: &str = "/dev/tpmrm0";
const BLOB_MAGIC: &[u8; 4] = b"AXDK";
const BLOB_VERSION: u8 = 1;
/// P-256 coordinates are 32 bytes; the SRK template's unique field is two zero coordinates.
const P256_COORDINATE: usize = 32;

fn tpm(e: tss_esapi::Error) -> DeviceKeyError {
    DeviceKeyError::Tpm(e.to_string())
}

fn context() -> Result<Context, DeviceKeyError> {
    // Opening it first gives a clear reason for the common failures (TSS logs only codes).
    std::fs::OpenOptions::new().read(true).write(true).open(DEVICE).map_err(|e| {
        DeviceKeyError::Tpm(match e.kind() {
            std::io::ErrorKind::NotFound => format!("{DEVICE} doesn't exist"),
            std::io::ErrorKind::PermissionDenied => {
                format!("no access to {DEVICE} (the user must be in its group, usually tss)")
            }
            _ => format!("{DEVICE}: {e}"),
        })
    })?;
    let config = DeviceConfig::from_str(DEVICE).map_err(tpm)?;
    Context::new(TctiNameConf::Device(config)).map_err(tpm)
}

/// The TCG "ECC P-256 SRK" template.
fn storage_key_template() -> Result<Public, DeviceKeyError> {
    let attributes = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_sensitive_data_origin(true)
        .with_user_with_auth(true)
        .with_no_da(true)
        .with_restricted(true)
        .with_decrypt(true)
        .build()
        .map_err(tpm)?;
    let zero = || EccParameter::try_from(vec![0u8; P256_COORDINATE]).map_err(tpm);
    PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::Ecc)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attributes)
        .with_ecc_parameters(
            PublicEccParametersBuilder::new()
                .with_symmetric(SymmetricDefinitionObject::AES_128_CFB)
                .with_ecc_scheme(EccScheme::Null)
                .with_curve(EccCurve::NistP256)
                .with_key_derivation_function_scheme(KeyDerivationFunctionScheme::Null)
                .with_is_decryption_key(true)
                .with_restricted(true)
                .build()
                .map_err(tpm)?,
        )
        .with_ecc_unique_identifier(EccPoint::new(zero()?, zero()?))
        .build()
        .map_err(tpm)
}

fn sealed_object_template() -> Result<Public, DeviceKeyError> {
    let attributes = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_user_with_auth(true)
        .with_no_da(true)
        .build()
        .map_err(tpm)?;
    PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::KeyedHash)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attributes)
        .with_keyed_hash_parameters(PublicKeyedHashParameters::new(KeyedHashScheme::Null))
        .with_keyed_hash_unique_identifier(Default::default())
        .build()
        .map_err(tpm)
}

/// Runs `f` with the storage key loaded and an encrypting session salted with it, then
/// flushes both.
fn with_storage_key<T>(
    f: impl FnOnce(&mut Context, KeyHandle) -> Result<T, DeviceKeyError>,
) -> Result<T, DeviceKeyError> {
    let mut ctx = context()?;
    let template = storage_key_template()?;
    let primary = ctx
        .execute_with_nullauth_session(|ctx| {
            ctx.create_primary(Hierarchy::Owner, template, None, None, None, None)
        })
        .map_err(tpm)?
        .key_handle;
    let result = (|| {
        let session = ctx
            .start_auth_session(
                Some(primary),
                None,
                None,
                SessionType::Hmac,
                SymmetricDefinition::AES_128_CFB,
                HashingAlgorithm::Sha256,
            )
            .map_err(tpm)?
            .ok_or_else(|| DeviceKeyError::Tpm("no session".into()))?;
        let (attributes, mask) = SessionAttributesBuilder::new()
            .with_continue_session(true)
            .with_decrypt(true)
            .with_encrypt(true)
            .build();
        ctx.tr_sess_set_attributes(session, attributes, mask).map_err(tpm)?;
        let result = ctx.execute_with_session(Some(session), |ctx| f(ctx, primary));
        let _ = ctx.flush_context(SessionHandle::from(session).into());
        result
    })();
    let _ = ctx.flush_context(ObjectHandle::from(primary));
    result
}

/// Seals `secret`; the returned blob opens only on this TPM.
pub fn seal(secret: &DeviceSecret) -> Result<Vec<u8>, DeviceKeyError> {
    let data = SensitiveData::try_from(secret.to_vec()).map_err(tpm)?;
    let template = sealed_object_template()?;
    let created = with_storage_key(|ctx, primary| {
        ctx.create(primary, template, None, Some(data), None, None).map_err(tpm)
    })?;
    let public = created.out_public.marshall().map_err(tpm)?;
    let private = created.out_private.value();
    let mut blob = Vec::with_capacity(9 + public.len() + private.len());
    blob.extend_from_slice(BLOB_MAGIC);
    blob.push(BLOB_VERSION);
    for part in [public.as_slice(), private] {
        let len = u16::try_from(part.len()).map_err(|_| DeviceKeyError::Damaged)?;
        blob.extend_from_slice(&len.to_be_bytes());
        blob.extend_from_slice(part);
    }
    Ok(blob)
}

/// Splits a blob into its public and private parts.
fn parts(blob: &[u8]) -> Result<(&[u8], &[u8]), DeviceKeyError> {
    let rest = blob.strip_prefix(BLOB_MAGIC.as_slice()).ok_or(DeviceKeyError::Damaged)?;
    let rest = match rest.split_first() {
        Some((&BLOB_VERSION, rest)) => rest,
        _ => return Err(DeviceKeyError::Damaged),
    };
    let take = |rest: &[u8]| -> Result<(usize, usize), DeviceKeyError> {
        let len = rest.get(..2).ok_or(DeviceKeyError::Damaged)?;
        let len = u16::from_be_bytes([len[0], len[1]]) as usize;
        if rest.len() < 2 + len {
            return Err(DeviceKeyError::Damaged);
        }
        Ok((2, 2 + len))
    };
    let (start, end) = take(rest)?;
    let public = &rest[start..end];
    let rest = &rest[end..];
    let (start, end) = take(rest)?;
    if end != rest.len() {
        return Err(DeviceKeyError::Damaged);
    }
    Ok((public, &rest[start..end]))
}

/// Opens a blob made by [`seal`] on this TPM.
pub fn unseal(blob: &[u8]) -> Result<DeviceSecret, DeviceKeyError> {
    let (public, private) = parts(blob)?;
    let public = Public::unmarshall(public).map_err(|_| DeviceKeyError::Damaged)?;
    let private = Private::try_from(private.to_vec()).map_err(|_| DeviceKeyError::Damaged)?;
    let data = with_storage_key(|ctx, primary| {
        let object = ctx.load(primary, private, public).map_err(tpm)?;
        let data = ctx.unseal(object.into());
        let _ = ctx.flush_context(object.into());
        data.map_err(tpm)
    })?;
    let bytes = Zeroizing::new(data.value().to_vec());
    let secret: [u8; KEY_LEN] = bytes.as_slice().try_into().map_err(|_| DeviceKeyError::Damaged)?;
    Ok(Zeroizing::new(secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_parsing_rejects_garbage() {
        assert!(parts(b"").is_err());
        assert!(parts(b"AXDK\x02\x00\x00\x00\x00").is_err(), "unknown version");
        assert!(parts(b"AXDK\x01\x00\x05ab").is_err(), "truncated");
        assert!(parts(b"AXDK\x01\x00\x01a\x00\x01bc").is_err(), "trailing bytes");
        assert_eq!(parts(b"AXDK\x01\x00\x01a\x00\x02bc").unwrap(), (&b"a"[..], &b"bc"[..]));
    }
}
