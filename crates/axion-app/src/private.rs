//! The lock on private browsing: an Argon2 hash of the user's password in the profile
//! database, and the rule for when the private tabs lock again.

use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axion_store::Store;

const HASH_KEY: &str = "private_password_hash";

/// Hashes a new password (Argon2id with a random salt) for storing.
pub fn hash_password(password: &str) -> Option<String> {
    Argon2::default().hash_password(password.as_bytes()).ok().map(|h| h.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

pub struct PrivateLock {
    hash: Option<String>,
    locked: bool,
    /// When the window lost focus (the lock timer runs from here).
    away_since: Option<Instant>,
}

impl PrivateLock {
    pub fn load(store: Option<&Store>) -> Self {
        let hash = store.and_then(|s| s.meta(HASH_KEY).ok().flatten());
        // Private tabs never survive a restart, so there is nothing to unlock at start.
        Self { hash, locked: false, away_since: None }
    }

    pub fn has_password(&self) -> bool {
        self.hash.is_some()
    }

    pub fn is_locked(&self) -> bool {
        self.locked && self.hash.is_some()
    }

    pub fn set_password(&mut self, store: Option<&Store>, password: Option<&str>) -> bool {
        let hash = match password {
            Some(p) if !p.is_empty() => match hash_password(p) {
                Some(h) => Some(h),
                None => return false,
            },
            _ => None,
        };
        if let Some(store) = store
            && let Err(e) = store.set_meta(HASH_KEY, hash.as_deref())
        {
            tracing::error!("saving the private browsing password failed: {e}");
            return false;
        }
        self.hash = hash;
        self.locked = false;
        true
    }

    /// Tries to unlock; returns whether the password was right.
    pub fn unlock(&mut self, password: &str) -> bool {
        let ok = self.hash.as_deref().is_some_and(|h| verify_password(password, h));
        if ok {
            self.locked = false;
        }
        ok
    }

    pub fn lock_now(&mut self) {
        self.locked = self.hash.is_some();
    }

    /// Window focus changes: locks if the window was away longer than `after`.
    pub fn window_focused(&mut self, focused: bool, after: Duration) {
        if focused {
            if self.away_since.take().is_some_and(|t| t.elapsed() >= after) {
                self.lock_now();
            }
        } else {
            self.away_since.get_or_insert(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn locks_after_being_away() {
        let store = Store::in_memory().unwrap();
        let mut lock = PrivateLock::load(Some(&store));
        assert!(!lock.is_locked());
        assert!(lock.set_password(Some(&store), Some("pw")));
        lock.window_focused(false, Duration::ZERO);
        lock.window_focused(true, Duration::ZERO);
        assert!(lock.is_locked());
        assert!(!lock.unlock("nope"));
        assert!(lock.unlock("pw"));
        // Short absences don't lock.
        lock.window_focused(false, Duration::from_secs(3600));
        lock.window_focused(true, Duration::from_secs(3600));
        assert!(!lock.is_locked());
        // The hash survives a reload; removing the password unlocks for good.
        assert!(PrivateLock::load(Some(&store)).has_password());
        lock.set_password(Some(&store), None);
        lock.lock_now();
        assert!(!lock.is_locked());
    }
}
