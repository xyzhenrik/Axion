//! SHA-256 checksums: `SHA256SUMS` files (as `sha256sum` writes them) and verifying a file.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::UpdateError;

/// The lower-case hex SHA-256 of the file at `path`.
pub fn sha256_file(path: &Path) -> Result<String, UpdateError> {
    let mut file = std::fs::File::open(path).map_err(|e| UpdateError::io_path("opening", path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| UpdateError::io_path("reading", path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(hasher.finalize().as_slice()))
}

/// The hash of `bytes`, lower-case hex.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes).as_slice())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash `sums` (the text of a `SHA256SUMS` file: `<hex>  <name>` or `<hex> *<name>` per
/// line) lists for the file called `name`.
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let line = line.trim();
        let (hash, rest) = line.split_once(char::is_whitespace)?;
        let file = rest.trim_start().trim_start_matches('*');
        // Paths like "./dist/x" count by their last component.
        let file = file.rsplit('/').next().unwrap_or(file);
        let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (valid && file == name).then(|| hash.to_ascii_lowercase())
    })
}

/// Fails with [`UpdateError::ChecksumMismatch`] unless the file at `path` hashes to `expected`.
pub fn verify_file(path: &Path, expected: &str) -> Result<(), UpdateError> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch { expected: expected.trim().to_ascii_lowercase(), actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn hashes_known_values() {
        assert_eq!(sha256_bytes(b"abc"), ABC);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc").expect("write");
        assert_eq!(sha256_file(&path).expect("hash"), ABC);
    }

    #[test]
    fn reads_sums_files() {
        let other = "0".repeat(64);
        let sums =
            format!("{other}  other.bin\n{ABC}  Axion-1.0-x86_64.AppImage\n{} *star.bin\n", "A".repeat(64));
        assert_eq!(expected_hash(&sums, "Axion-1.0-x86_64.AppImage").as_deref(), Some(ABC));
        assert_eq!(expected_hash(&sums, "star.bin"), Some("a".repeat(64)));
        assert_eq!(expected_hash(&sums, "missing"), None);
        // Malformed hashes are ignored, paths count by their file name.
        assert_eq!(expected_hash("abc  x.bin", "x.bin"), None);
        assert_eq!(expected_hash(&format!("{ABC}  ./dist/x.bin"), "x.bin").as_deref(), Some(ABC));
    }

    #[test]
    fn verification_accepts_and_rejects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc").expect("write");
        assert!(verify_file(&path, ABC).is_ok());
        assert!(verify_file(&path, &ABC.to_ascii_uppercase()).is_ok());
        let err = verify_file(&path, &"0".repeat(64)).expect_err("mismatch");
        assert!(matches!(err, UpdateError::ChecksumMismatch { .. }));
        assert!(matches!(verify_file(&dir.path().join("nope"), ABC), Err(UpdateError::Io { .. })));
    }
}
