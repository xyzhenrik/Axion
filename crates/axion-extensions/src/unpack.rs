//! `.crx` and `.zip` packages.
//!
//! A CRX is a small header and then an ordinary zip. Version 3 (what the Chrome Web Store serves)
//! has a protobuf header with the publisher's public keys; version 2 has the key and signature
//! in plain fields. Signatures are **not verified**: the user picked the file themselves, and an
//! unpacked extension does not need one. The key is kept (written into the manifest as `key`)
//! because it fixes the extension's id.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use flate2::read::DeflateDecoder;
use sha2::{Digest, Sha256};

use crate::{ExtensionError, MAX_BYTES, Result, io};

fn bad(message: impl Into<String>) -> ExtensionError {
    ExtensionError::Package(message.into())
}

fn u16_at(data: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn u32_at(data: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// Where the zip starts in a package, and the public key (base64) of a CRX. A plain zip starts at 0.
pub fn crx_public_key(data: &[u8]) -> Result<(usize, Option<String>)> {
    if data.get(..4) != Some(b"Cr24") {
        return Ok((0, None));
    }
    let version = u32_at(data, 4).ok_or_else(|| bad("truncated header"))?;
    match version {
        2 => {
            let key_len = u32_at(data, 8).ok_or_else(|| bad("truncated header"))?;
            let sig_len = u32_at(data, 12).ok_or_else(|| bad("truncated header"))?;
            let key = data.get(16..16 + key_len).ok_or_else(|| bad("truncated header"))?;
            let key = base64::engine::general_purpose::STANDARD.encode(key);
            Ok((16 + key_len + sig_len, Some(key)))
        }
        3 => {
            let header_len = u32_at(data, 8).ok_or_else(|| bad("truncated header"))?;
            let header = data.get(12..12 + header_len).ok_or_else(|| bad("truncated header"))?;
            Ok((12 + header_len, v3_key(header)))
        }
        other => Err(bad(format!("unknown CRX version {other}"))),
    }
}

/// The public key a CRX3 header's `crx_id` names: the extension's own key, not a publisher's
/// second signing key.
fn v3_key(header: &[u8]) -> Option<String> {
    let mut keys = Vec::new();
    let mut crx_id = None;
    for (field, value) in protobuf_fields(header) {
        match field {
            // sha256_with_rsa / sha256_with_ecdsa: AsymmetricKeyProof { public_key = 1 }
            2 | 3 => keys.extend(protobuf_fields(value).filter(|(f, _)| *f == 1).map(|(_, v)| v)),
            // signed_header_data: SignedData { crx_id = 1 }
            10000 => crx_id = protobuf_fields(value).find(|(f, _)| *f == 1).map(|(_, v)| v),
            _ => {}
        }
    }
    let id = crx_id?;
    let key = keys.into_iter().find(|key| Sha256::digest(key).get(..16) == Some(id))?;
    Some(base64::engine::general_purpose::STANDARD.encode(key))
}

/// The length-delimited fields of a protobuf message (others are skipped).
fn protobuf_fields(data: &[u8]) -> impl Iterator<Item = (u64, &[u8])> {
    let mut rest = data;
    std::iter::from_fn(move || {
        loop {
            let (tag, n) = varint(rest)?;
            rest = &rest[n..];
            match tag & 7 {
                0 => rest = &rest[varint(rest)?.1..],
                1 => rest = rest.get(8..)?,
                5 => rest = rest.get(4..)?,
                2 => {
                    let (len, n) = varint(rest)?;
                    let body = rest.get(n..n.checked_add(len as usize)?)?;
                    rest = &rest[n + body.len()..];
                    return Some((tag >> 3, body));
                }
                _ => return None,
            }
        }
    })
}

fn varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, &byte) in data.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// One entry of the zip's central directory.
struct Entry {
    name: String,
    method: usize,
    compressed: usize,
    size: u64,
    header_at: usize,
}

fn entries(zip: &[u8]) -> Result<Vec<Entry>> {
    // The end-of-central-directory record is at the end (after an optional comment).
    let eocd = (0..=zip.len().saturating_sub(22))
        .rev()
        .take(65_557)
        .find(|&at| zip.get(at..at + 4) == Some(b"PK\x05\x06"))
        .ok_or_else(|| bad("not a zip file"))?;
    let count = u16_at(zip, eocd + 10).ok_or_else(|| bad("truncated zip"))?;
    let mut at = u32_at(zip, eocd + 16).ok_or_else(|| bad("truncated zip"))?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if zip.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(bad("damaged zip directory"));
        }
        let field = |offset: usize, wide: bool| {
            if wide { u32_at(zip, at + offset) } else { u16_at(zip, at + offset) }.ok_or_else(|| bad("truncated zip"))
        };
        let (name_len, extra_len, comment_len) = (field(28, false)?, field(30, false)?, field(32, false)?);
        let name = zip.get(at + 46..at + 46 + name_len).ok_or_else(|| bad("truncated zip"))?;
        let size = field(24, true)?;
        let compressed = field(20, true)?;
        let header_at = field(42, true)?;
        if [size, compressed, header_at].contains(&0xFFFF_FFFF) {
            return Err(bad("zip64 packages are not supported"));
        }
        out.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method: field(10, false)?,
            compressed,
            size: size as u64,
            header_at,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// A file name of the zip as a relative path that stays inside the folder, or `None` for an
/// entry that doesn't (absolute, `..`).
fn safe_relative(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)).then(|| path.to_owned())
}

/// Unpacks the zip part of `data` (a `.crx` or a `.zip`) into the existing folder `dest`.
/// Returns the CRX's public key (base64), if it has one.
pub fn unzip(data: &[u8], dest: &Path) -> Result<Option<String>> {
    let (start, key) = crx_public_key(data)?;
    let zip = data.get(start..).ok_or_else(|| bad("truncated package"))?;
    let mut total = 0u64;
    for entry in entries(zip)? {
        if entry.name.ends_with('/') {
            continue;
        }
        let relative = safe_relative(&entry.name).ok_or_else(|| bad(format!("unsafe path {}", entry.name)))?;
        total += entry.size;
        if total > MAX_BYTES {
            return Err(ExtensionError::TooLarge(MAX_BYTES / (1024 * 1024)));
        }
        let at = entry.header_at;
        if zip.get(at..at + 4) != Some(b"PK\x03\x04") {
            return Err(bad("damaged zip entry"));
        }
        let name_len = u16_at(zip, at + 26).ok_or_else(|| bad("truncated zip"))?;
        let extra_len = u16_at(zip, at + 28).ok_or_else(|| bad("truncated zip"))?;
        let start = at + 30 + name_len + extra_len;
        let raw = zip.get(start..start + entry.compressed).ok_or_else(|| bad("truncated zip"))?;
        let mut content = Vec::with_capacity(entry.size as usize);
        match entry.method {
            0 => content.extend_from_slice(raw),
            8 => {
                // `take` caps a lying size field (zip bombs).
                DeflateDecoder::new(raw)
                    .take(entry.size + 1)
                    .read_to_end(&mut content)
                    .map_err(|e| bad(format!("{}: {e}", entry.name)))?;
            }
            other => return Err(bad(format!("{}: unsupported compression {other}", entry.name))),
        }
        if content.len() as u64 != entry.size {
            return Err(bad(format!("{}: wrong size", entry.name)));
        }
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&target, content).map_err(io(&target))?;
    }
    Ok(key)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A zip of `files` (name, content), deflated, built by hand.
    pub(crate) fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for (name, content) in files {
            let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(content).unwrap();
            let packed = encoder.finish().unwrap();
            let offset = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&0u32.to_le_bytes()); // crc (not checked)
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(content.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            directory.extend_from_slice(b"PK\x01\x02");
            directory.extend_from_slice(&[20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            directory.extend_from_slice(&0u32.to_le_bytes());
            directory.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(content.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&[0; 12]);
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let directory_at = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&directory_at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn field(number: u64, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut tag = (number << 3) | 2;
        loop {
            let byte = (tag & 0x7F) as u8;
            tag >>= 7;
            if tag == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.push(body.len() as u8);
        out.extend_from_slice(body);
        out
    }

    /// A CRX3 around `zip` with `key` as the extension's key and another key beside it.
    pub(crate) fn make_crx3(zip: &[u8], key: &[u8]) -> Vec<u8> {
        let id = &Sha256::digest(key)[..16];
        let mut header = field(2, &field(1, key));
        header.extend(field(2, &field(1, b"some other publisher key")));
        header.extend(field(10000, &field(1, id)));
        let mut out = b"Cr24".to_vec();
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend(header);
        out.extend_from_slice(zip);
        out
    }

    #[test]
    fn unpacks_a_zip() {
        let dir = tempfile::tempdir().unwrap();
        let zip = make_zip(&[("manifest.json", b"{}"), ("js/a.js", b"let a = 1;\n".repeat(50).as_slice())]);
        assert_eq!(unzip(&zip, dir.path()).unwrap(), None);
        assert_eq!(std::fs::read(dir.path().join("manifest.json")).unwrap(), b"{}");
        assert_eq!(std::fs::read(dir.path().join("js/a.js")).unwrap().len(), 550);
    }

    #[test]
    fn a_crx3_gives_the_key_its_id_names() {
        let dir = tempfile::tempdir().unwrap();
        let crx = make_crx3(&make_zip(&[("manifest.json", b"{}")]), b"the extension key");
        let key = unzip(&crx, dir.path()).unwrap().unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(key).unwrap(), b"the extension key");
        assert!(dir.path().join("manifest.json").exists());
    }

    #[test]
    fn paths_outside_the_folder_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["../evil.js", "/etc/evil", "a/../../evil"] {
            let zip = make_zip(&[(name, b"x")]);
            assert!(unzip(&zip, dir.path()).is_err(), "{name}");
        }
    }

    #[test]
    fn garbage_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(unzip(b"not a zip at all, but long enough to scan", dir.path()).is_err());
        assert!(unzip(b"Cr24\x09\0\0\0", dir.path()).is_err());
        assert!(unzip(b"", dir.path()).is_err());
    }
}
