//! The circuit path of a stream, for the circuit display.
//!
//! Arti hands out the path of the circuit a stream runs on (`DataStream::client_stream_ctrl`
//! with `tor-proto`'s `stream-ctrl` feature, then `ClientTunnel::all_paths`): the identity and
//! the addresses of each relay. Countries come from the GeoIP database that is compiled into
//! the program; there is no lookup over the network. Nothing is invented: a stream whose path
//! cannot be read has no entry.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};

use arti_client::DataStream;
use tor_geoip::GeoipDb;
use tor_linkspec::{HasAddrs as _, HasRelayIds as _};
use tor_proto::client::stream::ClientStreamCtrl as _;

use crate::circuit::{CircuitInfo, Hop, roles_for};

/// Most sites whose circuit is remembered.
const MAX_ENTRIES: usize = 1024;

/// The embedded country database.
pub(super) struct GeoIp(Arc<GeoipDb>);

impl GeoIp {
    pub(super) fn embedded() -> Self {
        Self(GeoipDb::new_embedded())
    }

    fn country(&self, addrs: &[IpAddr]) -> Option<String> {
        addrs.iter().find_map(|ip| self.0.lookup_country_code(*ip)).map(|c| c.get().to_ascii_uppercase())
    }
}

/// A circuit path and whether the circuit still exists.
pub(super) struct Described {
    info: CircuitInfo,
    open: Box<dyn Fn() -> bool + Send + Sync>,
}

/// Reads the path of the circuit a stream runs on; `None` when Arti does not tell.
pub(super) fn describe(stream: &DataStream, geoip: &GeoIp) -> Option<Described> {
    let tunnel = stream.client_stream_ctrl()?.tunnel()?;
    let paths = tunnel.all_paths();
    let path = paths.first()?;
    let mut relays = Vec::new();
    let mut virtual_hops = 0;
    for entry in path.iter() {
        match entry.as_chan_target() {
            Some(target) => {
                let addrs: Vec<IpAddr> = target.addrs().map(|a| a.ip()).collect();
                let fingerprint = target
                    .rsa_identity()
                    .map(|id| id.as_bytes().iter().map(|b| format!("{b:02X}")).collect::<String>())
                    .unwrap_or_default();
                let country = geoip.country(&addrs);
                relays.push((fingerprint, addrs, country));
            }
            None => virtual_hops += 1,
        }
    }
    if relays.is_empty() {
        return None;
    }
    let roles = roles_for(relays.len(), virtual_hops);
    let mut hops: Vec<Hop> = relays
        .into_iter()
        .zip(&roles)
        .map(|((fingerprint, addrs, country), role)| Hop { role: *role, fingerprint, addrs, country })
        .collect();
    for role in roles.iter().skip(hops.len()) {
        hops.push(Hop { role: *role, fingerprint: String::new(), addrs: Vec::new(), country: None });
    }
    // Only a weak handle is kept: remembering a path never keeps a circuit alive.
    let weak = Arc::downgrade(&tunnel);
    Some(Described {
        info: CircuitInfo { hops, onion: virtual_hops > 0 },
        open: Box::new(move || weak.upgrade().is_some_and(|t| !t.is_closed())),
    })
}

struct Entry {
    described: Described,
    seq: u64,
}

/// The most recent circuit per site.
pub(super) struct CircuitBook {
    entries: Mutex<(u64, HashMap<String, Entry>)>,
}

impl CircuitBook {
    pub(super) fn new() -> Self {
        Self { entries: Mutex::new((0, HashMap::new())) }
    }

    /// Remembers the circuit of the newest stream of a site (`None`: it could not be read, and
    /// the old entry, which may belong to a circuit that is gone, is dropped).
    pub(super) fn record(&self, key: &str, described: Option<Described>) {
        let mut guard = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let (counter, map) = &mut *guard;
        let Some(described) = described else {
            map.remove(key);
            return;
        };
        *counter += 1;
        if map.len() >= MAX_ENTRIES && !map.contains_key(key) {
            // Forget the oldest.
            if let Some(oldest) = map.iter().min_by_key(|(_, e)| e.seq).map(|(k, _)| k.clone()) {
                map.remove(&oldest);
            }
        }
        map.insert(key.to_owned(), Entry { described, seq: *counter });
    }

    /// The circuit of a site, if it is known and still open.
    pub(super) fn get(&self, key: &str) -> Option<CircuitInfo> {
        let guard = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard.1.get(key)?;
        (entry.described.open)().then(|| entry.described.info.clone())
    }

    pub(super) fn forget(&self, key: &str) {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).1.remove(key);
    }

    pub(super) fn clear(&self) {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).1.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn described(open: bool) -> Described {
        Described { info: CircuitInfo { hops: Vec::new(), onion: false }, open: Box::new(move || open) }
    }

    #[test]
    fn book_keeps_the_newest_open_circuit_per_site() {
        let book = CircuitBook::new();
        book.record("example.com", Some(described(true)));
        assert!(book.get("example.com").is_some());
        assert!(book.get("example.org").is_none());
        book.record("example.com", Some(described(false)));
        assert!(book.get("example.com").is_none(), "a closed circuit is not shown");
        book.record("example.com", Some(described(true)));
        book.record("example.com", None);
        assert!(book.get("example.com").is_none(), "an unreadable path erases the old one");
        book.record("a.org", Some(described(true)));
        book.clear();
        assert!(book.get("a.org").is_none());
    }

    #[test]
    fn book_is_bounded() {
        let book = CircuitBook::new();
        for i in 0..MAX_ENTRIES + 10 {
            book.record(&format!("site{i}.example"), Some(described(true)));
        }
        assert!(book.get("site0.example").is_none(), "the oldest went first");
        assert!(book.get(&format!("site{}.example", MAX_ENTRIES + 9)).is_some());
    }
}
