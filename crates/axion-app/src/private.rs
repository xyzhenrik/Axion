//! The lock on private browsing, and the rules for moving between normal and private tabs.
//!
//! The lock uses the account password (the password vault's master password), and there is
//! one unlock per session: entering the password anywhere (the private tabs' lock screen, the
//! unlock-to-fill prompt, Passwords, Settings) unlocks private tabs and password filling until
//! Axion quits or the account menu's Lock is used. Nothing locks again by itself.
//!
//! The decisions here are kept free of the engine and the UI so they can be tested.

use axion_store::Store;

/// Where versions before accounts kept a separate private browsing password's hash.
const OLD_HASH_KEY: &str = "private_password_hash";

/// The session's unlock state.
pub struct PrivateLock {
    unlocked: bool,
}

impl PrivateLock {
    /// A session starts locked: private tabs (and filling passwords) need the account
    /// password once.
    pub fn new() -> Self {
        Self { unlocked: false }
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

    /// Locked (only matters while the lock is on and the account has a password).
    pub fn is_locked(&self) -> bool {
        !self.unlocked
    }

    pub fn unlock(&mut self) {
        self.unlocked = true;
    }

    /// The account menu's Lock (or the Passwords sheet's lock button).
    pub fn lock_now(&mut self) {
        self.unlocked = false;
    }

    /// Follows the vault: unlocked somewhere else (a fill prompt, Passwords), or locked.
    /// Returns whether the session's state changed.
    pub fn follow_vault(&mut self, vault_unlocked: bool) -> bool {
        let changed = vault_unlocked != self.unlocked;
        self.unlocked = vault_unlocked;
        changed
    }

    /// Whether private tabs are kept out of sight: the lock is on (`enabled`: the setting is
    /// on and the account has a password) and the session isn't unlocked.
    pub fn hides_private(&self, enabled: bool) -> bool {
        enabled && self.is_locked()
    }
}

/// The group (`true`: private) a tab opened by a page goes to: the opener's own group, never
/// the one that happens to be shown (a background normal tab's popup must not land among
/// private tabs, nor a private one's among normal tabs). Without an opener, the shown group.
pub fn group_for_new_view(opener_private: Option<bool>, shown_private: bool) -> bool {
    opener_private.unwrap_or(shown_private)
}

/// What dropping a dragged tab means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabDrop {
    /// Within its own group: it just moves to its new place.
    Reorder,
    /// Into the other group: the same address opens there (its own cookies and logins) and
    /// the original tab closes.
    Reopen,
    /// Into locked private tabs: the account password comes first.
    NeedsUnlock,
}

/// Decides a drop of a tab from one group into another; `hidden` is
/// [`PrivateLock::hides_private`].
pub fn drop_into(from_private: bool, to_private: bool, hidden: bool) -> TabDrop {
    if from_private == to_private {
        TabDrop::Reorder
    } else if to_private && hidden {
        TabDrop::NeedsUnlock
    } else {
        TabDrop::Reopen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_starts_locked_and_one_unlock_lasts() {
        let mut lock = PrivateLock::new();
        assert!(lock.is_locked());
        assert!(lock.hides_private(true));
        // Without a lock (no password, or the setting off) nothing is hidden.
        assert!(!lock.hides_private(false));
        lock.unlock();
        assert!(!lock.hides_private(true));
        // Only Lock locks again.
        lock.lock_now();
        assert!(lock.hides_private(true));
    }

    #[test]
    fn unlocking_the_vault_elsewhere_unlocks_private_tabs() {
        let mut lock = PrivateLock::new();
        assert!(!lock.follow_vault(false));
        assert!(lock.follow_vault(true));
        assert!(!lock.is_locked());
        assert!(!lock.follow_vault(true));
        assert!(lock.follow_vault(false));
        assert!(lock.is_locked());
    }

    #[test]
    fn pages_open_tabs_in_their_own_group() {
        // A normal tab in the background opens a popup while private tabs are shown.
        assert!(!group_for_new_view(Some(false), true));
        assert!(group_for_new_view(Some(true), false));
        assert!(group_for_new_view(None, true));
        assert!(!group_for_new_view(None, false));
    }

    #[test]
    fn drops_between_groups() {
        assert_eq!(drop_into(false, false, true), TabDrop::Reorder);
        assert_eq!(drop_into(true, true, false), TabDrop::Reorder);
        assert_eq!(drop_into(false, true, false), TabDrop::Reopen);
        assert_eq!(drop_into(false, true, true), TabDrop::NeedsUnlock);
        // Out of private tabs never needs the password.
        assert_eq!(drop_into(true, false, true), TabDrop::Reopen);
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
