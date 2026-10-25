//! The lock on private browsing. It uses the account password (the password vault's master
//! password): unlocking either unlocks both, and locking either locks both — one unlock
//! session per account. This keeps the rule for when the private tabs lock again (after the
//! window was in the background for a while); the browser applies it to the vault too.

use std::time::{Duration, Instant};

use axion_store::Store;

/// Where versions before accounts kept a separate private browsing password's hash.
const OLD_HASH_KEY: &str = "private_password_hash";

pub struct PrivateLock {
    locked: bool,
    /// When the window lost focus (the lock timer runs from here).
    away_since: Option<Instant>,
}

impl PrivateLock {
    /// Private tabs never survive a restart, so there is nothing to unlock at start.
    pub fn new() -> Self {
        Self { locked: false, away_since: None }
    }

    /// Drops the separate private browsing password of older versions: the account password
    /// replaces it (and is created when it's first needed).
    pub fn drop_old_password(store: Option<&Store>) {
        let Some(store) = store else { return };
        if store.meta(OLD_HASH_KEY).ok().flatten().is_none() {
            return;
        }
        match store.set_meta(OLD_HASH_KEY, None) {
            Ok(()) => tracing::info!(
                "removed the old private browsing password; private tabs now lock with the account password"
            ),
            Err(e) => tracing::warn!("removing the old private browsing password failed: {e}"),
        }
    }

    /// Locked (only matters while the account has a password).
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn unlock(&mut self) {
        self.locked = false;
    }

    pub fn lock_now(&mut self) {
        self.locked = true;
    }

    /// Locks once the window has been in the background for `after`, while it still is —
    /// so private tabs are never visible in an unfocused window past the limit. Returns
    /// whether it just locked.
    pub fn tick(&mut self, after: Duration) -> bool {
        if !self.locked && self.away_since.is_some_and(|t| t.elapsed() >= after) {
            self.locked = true;
            return true;
        }
        false
    }

    /// Window focus changes: locks if the window was away longer than `after`. Returns
    /// whether it just locked.
    pub fn window_focused(&mut self, focused: bool, after: Duration) -> bool {
        if focused {
            if self.away_since.take().is_some_and(|t| t.elapsed() >= after) && !self.locked {
                self.locked = true;
                return true;
            }
        } else {
            self.away_since.get_or_insert(Instant::now());
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_being_away() {
        let mut lock = PrivateLock::new();
        assert!(!lock.is_locked());
        assert!(!lock.window_focused(false, Duration::ZERO));
        assert!(lock.window_focused(true, Duration::ZERO));
        assert!(lock.is_locked());
        lock.unlock();
        // Short absences don't lock.
        lock.window_focused(false, Duration::from_secs(3600));
        assert!(!lock.tick(Duration::from_secs(3600)));
        assert!(!lock.window_focused(true, Duration::from_secs(3600)));
        assert!(!lock.is_locked());
        // While away, the timer locks.
        lock.window_focused(false, Duration::ZERO);
        assert!(lock.tick(Duration::ZERO));
        assert!(!lock.tick(Duration::ZERO));
    }

    #[test]
    fn drops_the_old_password_hash() {
        let store = Store::in_memory().unwrap();
        store.set_meta(OLD_HASH_KEY, Some("$argon2id$old")).unwrap();
        PrivateLock::drop_old_password(Some(&store));
        assert_eq!(store.meta(OLD_HASH_KEY).unwrap(), None);
        // Nothing to drop is fine too.
        PrivateLock::drop_old_password(Some(&store));
        PrivateLock::drop_old_password(None);
    }
}
