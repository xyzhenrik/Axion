//! Strong random passwords.

use chacha20poly1305::aead::OsRng;
use chacha20poly1305::aead::rand_core::RngCore;

const LOWER: &str = "abcdefghijkmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ";
const DIGITS: &str = "23456789";
const SYMBOLS: &str = "!#$%&*+-=?@^_~";

/// A password of `length` characters (at least 8) with lower and upper case letters, digits
/// and — with `symbols` — symbols, each at least once. Look-alikes (l, I, O, 0, 1) are left out.
pub fn generate(length: usize, symbols: bool) -> String {
    let length = length.max(8);
    let mut sets = vec![LOWER, UPPER, DIGITS];
    if symbols {
        sets.push(SYMBOLS);
    }
    let all: Vec<char> = sets.iter().flat_map(|s| s.chars()).collect();
    loop {
        let password: String = (0..length).map(|_| all[uniform(all.len())]).collect();
        // Retry the rare draw that misses a class (keeps the distribution uniform).
        if sets.iter().all(|set| password.chars().any(|c| set.contains(c))) {
            return password;
        }
    }
}

/// A uniform random index below `n` (rejection sampling, no modulo bias).
fn uniform(n: usize) -> usize {
    let n = n as u32;
    let limit = u32::MAX - u32::MAX % n;
    loop {
        let v = OsRng.next_u32();
        if v < limit {
            return (v % n) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_have_every_class() {
        for _ in 0..50 {
            let p = generate(16, true);
            assert_eq!(p.chars().count(), 16);
            assert!(p.chars().any(|c| SYMBOLS.contains(c)) && p.chars().any(|c| DIGITS.contains(c)));
        }
        assert!(!generate(12, false).chars().any(|c| SYMBOLS.contains(c)));
        assert_eq!(generate(3, true).len(), 8);
        assert_ne!(generate(20, true), generate(20, true));
    }
}
