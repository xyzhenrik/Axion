//! Verifying a CRX3 package the way Chromium does: the header names the extension (`crx_id`,
//! the first 16 bytes of the SHA-256 of its public key) and carries signatures over the header
//! data and the zip that follows it. A package is trusted only if one signature made with the
//! key behind `crx_id` is valid, so the id the Web Store was asked for really is the id of the
//! code that came back.

use ring::signature::{
    ECDSA_P256_SHA256_ASN1, RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY, UnparsedPublicKey,
    VerificationAlgorithm,
};
use sha2::{Digest, Sha256};

use crate::manifest::id_from_hash;
use crate::unpack::{protobuf_fields, u32_at};
use crate::{ExtensionError, Result};

/// What Chromium signs: this context string (with its NUL), the header data's length, the
/// header data and the zip.
const SIGNATURE_CONTEXT: &[u8] = b"CRX3 SignedData\0";

/// A package that passed [`verify_crx3`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The extension id (32 letters a-p).
    pub id: String,
    /// The extension's public key (DER, SubjectPublicKeyInfo).
    pub public_key: Vec<u8>,
    /// Where the zip starts in the package.
    pub zip_at: usize,
}

fn bad(message: &str) -> ExtensionError {
    ExtensionError::Package(message.to_owned())
}

#[derive(Clone, Copy)]
enum Kind {
    Rsa,
    Ecdsa,
}

/// Checks `data` as a CRX3 package: structure, the extension's own signature, and (if given)
/// that its id is `expected_id`. Only RSA and ECDSA P-256 proofs are understood; the package is
/// accepted when the proof made with the key behind its id is valid (a publisher's second proof
/// is not needed for that).
pub fn verify_crx3(data: &[u8], expected_id: Option<&str>) -> Result<Verified> {
    if data.get(..4) != Some(b"Cr24") {
        return Err(bad("not a CRX package"));
    }
    match u32_at(data, 4) {
        Some(3) => {}
        Some(other) => return Err(ExtensionError::Package(format!("CRX version {other} is not supported"))),
        None => return Err(bad("truncated header")),
    }
    let header_len = u32_at(data, 8).ok_or_else(|| bad("truncated header"))?;
    let header = data.get(12..12usize.saturating_add(header_len)).ok_or_else(|| bad("truncated header"))?;
    let zip_at = 12 + header_len;
    let zip = data.get(zip_at..).ok_or_else(|| bad("truncated package"))?;

    let mut signed_data = None;
    let mut proofs: Vec<(Kind, &[u8], &[u8])> = Vec::new();
    for (field, value) in protobuf_fields(header) {
        match field {
            2 | 3 => {
                let kind = if field == 2 { Kind::Rsa } else { Kind::Ecdsa };
                let key = protobuf_fields(value).find(|(f, _)| *f == 1).map(|(_, v)| v);
                let signature = protobuf_fields(value).find(|(f, _)| *f == 2).map(|(_, v)| v);
                if let (Some(key), Some(signature)) = (key, signature) {
                    proofs.push((kind, key, signature));
                }
            }
            10000 => signed_data = Some(value),
            _ => {}
        }
    }
    let signed_data = signed_data.ok_or_else(|| bad("the package has no signed header data"))?;
    let crx_id = protobuf_fields(signed_data)
        .find(|(f, _)| *f == 1)
        .map(|(_, v)| v)
        .filter(|id| id.len() == 16)
        .ok_or_else(|| bad("the package has no extension id"))?;
    let id = id_from_hash(crx_id);
    if let Some(expected) = expected_id
        && !expected.eq_ignore_ascii_case(&id)
    {
        return Err(ExtensionError::Package(format!("the package is for another extension ({id})")));
    }

    let signed_len = u32::try_from(signed_data.len()).map_err(|_| bad("header too large"))?;
    let mut message = Vec::with_capacity(SIGNATURE_CONTEXT.len() + 4 + signed_data.len() + zip.len());
    message.extend_from_slice(SIGNATURE_CONTEXT);
    message.extend_from_slice(&signed_len.to_le_bytes());
    message.extend_from_slice(signed_data);
    message.extend_from_slice(zip);

    for (kind, key, signature) in proofs {
        if Sha256::digest(key).get(..16) != Some(crx_id) {
            continue;
        }
        let Some(raw) = spki_key_bits(key) else { continue };
        let valid = match kind {
            // Old extension keys are 1024 bits; Chromium accepts them, so do we.
            Kind::Rsa => verify(&RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY, raw, &message, signature),
            Kind::Ecdsa => verify(&ECDSA_P256_SHA256_ASN1, raw, &message, signature),
        };
        if valid {
            return Ok(Verified { id, public_key: key.to_vec(), zip_at });
        }
    }
    Err(bad("the package signature is not valid"))
}

fn verify(
    algorithm: &'static dyn VerificationAlgorithm,
    key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> bool {
    UnparsedPublicKey::new(algorithm, key).verify(message, signature).is_ok()
}

/// One DER element at the start of `data`: (tag, content, rest).
fn der(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *data.first()?;
    let first = *data.get(1)?;
    let (len, header) = if first < 0x80 {
        (usize::from(first), 2)
    } else {
        let n = usize::from(first & 0x7F);
        if n == 0 || n > 4 {
            return None;
        }
        let bytes = data.get(2..2 + n)?;
        (bytes.iter().fold(0usize, |acc, b| (acc << 8) | usize::from(*b)), 2 + n)
    };
    let end = header.checked_add(len)?;
    Some((tag, data.get(header..end)?, data.get(end..)?))
}

/// The key bits of a SubjectPublicKeyInfo: the PKCS#1 `RSAPublicKey` of an RSA key, the
/// uncompressed point of an EC key (what `ring` verifies against).
fn spki_key_bits(spki: &[u8]) -> Option<&[u8]> {
    let (tag, body, _) = der(spki)?;
    if tag != 0x30 {
        return None;
    }
    let (_, _, rest) = der(body)?; // the algorithm
    let (tag, bits, _) = der(rest)?;
    // BIT STRING: the first byte counts unused bits (0 for keys).
    (tag == 0x03 && bits.first() == Some(&0)).then(|| &bits[1..])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::unpack::tests::{field, make_zip};
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, RSA_PKCS1_SHA256, RsaKeyPair};

    const RSA_PK8: &[u8] = include_bytes!("../tests/fixtures/test_rsa.der");
    pub(crate) const RSA_SPKI: &[u8] = include_bytes!("../tests/fixtures/test_rsa.spki.der");

    /// A fresh P-256 key: its PKCS#8 form and its SubjectPublicKeyInfo.
    fn ec_key() -> (Vec<u8>, Vec<u8>) {
        use ring::signature::KeyPair;
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pk8.as_ref(), &rng).unwrap();
        // id-ecPublicKey, prime256v1, then the uncompressed point as a BIT STRING.
        let mut spki = hex_bytes("3059301306072a8648ce3d020106082a8648ce3d030107034200");
        spki.extend_from_slice(pair.public_key().as_ref());
        (pk8.as_ref().to_vec(), spki)
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    fn sign_rsa(message: &[u8]) -> Vec<u8> {
        let pair = RsaKeyPair::from_der(RSA_PK8).unwrap();
        let mut signature = vec![0; pair.public().modulus_len()];
        pair.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), message, &mut signature).unwrap();
        signature
    }

    fn sign_ec(pk8: &[u8], message: &[u8]) -> Vec<u8> {
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pk8, &rng).unwrap();
        pair.sign(&rng, message).unwrap().as_ref().to_vec()
    }

    /// A CRX3 around `zip`, signed by the RSA fixture key (or the EC one) as the extension key.
    pub(crate) fn signed_crx3(zip: &[u8], ecdsa: bool) -> Vec<u8> {
        let (ec_pk8, ec_spki) = ec_key();
        let key: &[u8] = if ecdsa { &ec_spki } else { RSA_SPKI };
        let id = &Sha256::digest(key)[..16];
        let signed_data = field(1, id);
        let mut message = SIGNATURE_CONTEXT.to_vec();
        message.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
        message.extend_from_slice(&signed_data);
        message.extend_from_slice(zip);
        let signature = if ecdsa { sign_ec(&ec_pk8, &message) } else { sign_rsa(&message) };
        let proof = [field(1, key), field(2, &signature)].concat();
        let mut header = field(if ecdsa { 3 } else { 2 }, &proof);
        header.extend(field(10000, &signed_data));
        let mut out = b"Cr24".to_vec();
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend(header);
        out.extend_from_slice(zip);
        out
    }

    fn zip() -> Vec<u8> {
        make_zip(&[("manifest.json", br#"{"manifest_version":3,"name":"T","version":"1"}"#)])
    }

    #[test]
    fn a_signed_package_verifies_and_names_the_id() {
        for ecdsa in [false, true] {
            let crx = signed_crx3(&zip(), ecdsa);
            let verified = verify_crx3(&crx, None).unwrap();
            assert_eq!(verified.id.len(), 32);
            assert!(verified.id.bytes().all(|b| (b'a'..=b'p').contains(&b)));
            assert_eq!(verified.zip_at, crx.len() - zip().len());
            assert_eq!(verify_crx3(&crx, Some(&verified.id)).unwrap(), verified);
        }
    }

    #[test]
    fn another_id_is_refused() {
        let crx = signed_crx3(&zip(), false);
        let err = verify_crx3(&crx, Some("dhdgffkkebhmkfjojejmpbldmpobfkfo")).unwrap_err();
        assert!(err.to_string().contains("another extension"), "{err}");
    }

    #[test]
    fn a_changed_zip_breaks_the_signature() {
        let mut crx = signed_crx3(&zip(), false);
        let last = crx.len() - 30;
        crx[last] ^= 1;
        assert!(verify_crx3(&crx, None).is_err());
    }

    #[test]
    fn a_changed_header_breaks_the_signature() {
        let mut crx = signed_crx3(&zip(), false);
        let zip_at = verify_crx3(&crx, None).unwrap().zip_at;
        // One byte of the signature itself (inside the header).
        crx[zip_at - 20] ^= 1;
        assert!(verify_crx3(&crx, None).is_err());
    }

    #[test]
    fn unsigned_and_foreign_packages_are_refused() {
        // The unsigned test packages of the unpack tests have no valid signature.
        let crx = crate::unpack::tests::make_crx3(&zip(), RSA_SPKI);
        assert!(verify_crx3(&crx, None).is_err());
        assert!(verify_crx3(b"PK\x03\x04 a zip", None).is_err());
        assert!(verify_crx3(b"Cr24\x02\0\0\0", None).is_err());
        assert!(verify_crx3(b"", None).is_err());
    }

    /// Manual: `AXION_TEST_CRX=<file> AXION_TEST_CRX_ID=<id> cargo test -p axion-extensions -- --ignored real_store_package`
    /// checks a package downloaded from the Chrome Web Store.
    #[test]
    #[ignore = "needs a downloaded package"]
    fn real_store_package() {
        let path = std::env::var("AXION_TEST_CRX").unwrap();
        let id = std::env::var("AXION_TEST_CRX_ID").unwrap();
        let data = std::fs::read(path).unwrap();
        let verified = verify_crx3(&data, Some(&id)).unwrap();
        assert_eq!(verified.id, id);
    }

    #[test]
    fn spki_is_unwrapped() {
        let bits = spki_key_bits(RSA_SPKI).unwrap();
        assert_eq!(bits[0], 0x30);
        assert!(spki_key_bits(b"junk").is_none());
    }
}
