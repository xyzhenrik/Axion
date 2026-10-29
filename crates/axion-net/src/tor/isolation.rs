//! Stream isolation: which connections may share a Tor circuit.
//!
//! Tor Browser isolates by the first-party site of the page. The browser's loopback front
//! cannot tell which page a request belongs to, so Axion isolates by the **destination**
//! site (registrable domain): every connection to `example.com` and its subdomains shares
//! circuits, a connection to another site gets different ones, so no exit relay sees both.
//! `network.tor_isolation` can widen this to one set per connection, or turn it off.
//!
//! A site's *token* is retired by "New Tor circuit for this site": the next connections get a
//! new token (so new circuits) and the open ones are told to close.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use tokio::sync::watch;

use crate::connection::TorIsolation;

/// Most sites remembered at once; beyond it the map starts over (new tokens, so new circuits).
const MAX_SLOTS: usize = 4096;

/// The key all traffic shares in [`TorIsolation::Context`] mode.
const CONTEXT_KEY: &str = "*";

/// The site a host belongs to: its registrable domain; the onion address of an onion service
/// (whatever subdomain); an IP address itself.
pub fn site_key(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return host;
    }
    if let Some(onion) = host.strip_suffix(".onion") {
        let label = onion.rsplit('.').next().unwrap_or(onion);
        return format!("{label}.onion");
    }
    addr::parse_domain_name(&host).ok().and_then(|d| d.root().map(str::to_owned)).unwrap_or(host)
}

/// The isolation key of a host under a mode; `None`: no explicit isolation.
pub fn isolation_key(mode: TorIsolation, host: &str) -> Option<String> {
    match mode {
        TorIsolation::Site => Some(site_key(host)),
        TorIsolation::Context => Some(CONTEXT_KEY.to_owned()),
        TorIsolation::None => None,
    }
}

/// What a connection holds while it is open: its token and a signal that says "close".
#[derive(Clone)]
pub struct Lease<T> {
    pub token: T,
    pub cancel: watch::Receiver<bool>,
}

struct Slot<T> {
    token: T,
    cancel: watch::Sender<bool>,
}

/// The isolation tokens of one Tor connection.
pub struct Isolation<T> {
    mode: TorIsolation,
    make: fn() -> T,
    slots: Mutex<HashMap<String, Slot<T>>>,
}

impl<T: Clone> Isolation<T> {
    pub fn new(mode: TorIsolation, make: fn() -> T) -> Self {
        Self { mode, make, slots: Mutex::new(HashMap::new()) }
    }

    pub fn mode(&self) -> TorIsolation {
        self.mode
    }

    /// The token for a connection to `host` (a new one for a site seen the first time).
    pub fn lease(&self, host: &str) -> Option<Lease<T>> {
        let key = isolation_key(self.mode, host)?;
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        if slots.len() >= MAX_SLOTS && !slots.contains_key(&key) {
            slots.clear();
        }
        let make = self.make;
        let slot = slots.entry(key).or_insert_with(|| Slot { token: make(), cancel: watch::channel(false).0 });
        Some(Lease { token: slot.token.clone(), cancel: slot.cancel.subscribe() })
    }

    /// Retires the token of `host`'s site and tells its open connections to close.
    pub fn retire(&self, host: &str) {
        let Some(key) = isolation_key(self.mode, host) else { return };
        let slot = self.slots.lock().unwrap_or_else(PoisonError::into_inner).remove(&key);
        if let Some(slot) = slot {
            slot.cancel.send_replace(true);
        }
    }

    /// Retires every token.
    pub fn retire_all(&self) {
        let slots: Vec<Slot<T>> =
            self.slots.lock().unwrap_or_else(PoisonError::into_inner).drain().map(|(_, s)| s).collect();
        for slot in slots {
            slot.cancel.send_replace(true);
        }
    }

    /// How many sites hold a token (tests).
    pub fn len(&self) -> usize {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn next() -> u64 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn site_keys() {
        assert_eq!(site_key("www.Example.com"), "example.com");
        assert_eq!(site_key("a.b.example.co.uk."), "example.co.uk");
        assert_eq!(site_key("example.com"), "example.com");
        assert_eq!(site_key("93.184.216.34"), "93.184.216.34");
        assert_eq!(site_key("[2001:db8::1]"), "[2001:db8::1]");
        assert_eq!(site_key("sub.abcdefghijklmnop.onion"), "abcdefghijklmnop.onion");
        assert_eq!(site_key("abcdefghijklmnop.onion"), "abcdefghijklmnop.onion");
        assert_eq!(site_key("localhost"), "localhost");
    }

    #[test]
    fn keys_by_mode() {
        assert_eq!(isolation_key(TorIsolation::Site, "cdn.example.com").as_deref(), Some("example.com"));
        assert_eq!(isolation_key(TorIsolation::Context, "cdn.example.com").as_deref(), Some("*"));
        assert_eq!(isolation_key(TorIsolation::Context, "other.org"), isolation_key(TorIsolation::Context, "x.com"));
        assert_eq!(isolation_key(TorIsolation::None, "example.com"), None);
    }

    #[test]
    fn same_site_shares_a_token_other_sites_do_not() {
        let iso = Isolation::new(TorIsolation::Site, next);
        let a = iso.lease("www.example.com").expect("lease");
        let b = iso.lease("cdn.example.com").expect("lease");
        let c = iso.lease("example.org").expect("lease");
        assert_eq!(a.token, b.token);
        assert_ne!(a.token, c.token);
        assert_eq!(iso.len(), 2);
    }

    #[test]
    fn context_mode_shares_one_token_and_none_has_none() {
        let iso = Isolation::new(TorIsolation::Context, next);
        let a = iso.lease("example.com").expect("lease");
        let b = iso.lease("example.org").expect("lease");
        assert_eq!(a.token, b.token);
        assert!(Isolation::new(TorIsolation::None, next).lease("example.com").is_none());
    }

    #[test]
    fn retiring_gives_a_new_token_and_signals_the_open_connections() {
        let iso = Isolation::new(TorIsolation::Site, next);
        let a = iso.lease("example.com").expect("lease");
        let other = iso.lease("example.org").expect("lease");
        assert!(!*a.cancel.borrow());
        iso.retire("www.example.com");
        assert!(*a.cancel.borrow(), "open connections are told to close");
        assert!(!*other.cancel.borrow(), "other sites are untouched");
        let b = iso.lease("example.com").expect("lease");
        assert_ne!(a.token, b.token);
        assert!(!*b.cancel.borrow());
        iso.retire_all();
        assert!(*b.cancel.borrow() && *other.cancel.borrow());
        assert!(iso.is_empty());
    }
}
