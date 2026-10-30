//! What a tunnel can tell about the path of a site's traffic: the hops of a Tor circuit.
//!
//! The data comes from the backend (the circuit path of a real stream); nothing here is
//! guessed. A hop the backend cannot describe is simply missing.

use std::net::IpAddr;

/// What a hop does on the circuit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HopRole {
    /// The first relay (the entry guard, or a bridge).
    Guard,
    Middle,
    /// The last relay of a circuit to an ordinary site.
    Exit,
    /// The last relay of a circuit to an onion service: where both sides meet.
    Rendezvous,
    /// The onion service itself (the far end; its own relays are hidden from us).
    OnionService,
}

/// One relay (or the onion service) on a circuit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    pub role: HopRole,
    /// The relay's RSA identity, upper-case hex (empty for the onion service).
    pub fingerprint: String,
    /// The relay's addresses (empty when unknown, as for some bridges).
    pub addrs: Vec<IpAddr>,
    /// Upper-case two-letter country code from the embedded GeoIP database.
    pub country: Option<String>,
}

impl Hop {
    /// The address to show: IPv4 first.
    pub fn primary_ip(&self) -> Option<IpAddr> {
        self.addrs.iter().find(|a| a.is_ipv4()).or_else(|| self.addrs.first()).copied()
    }

    /// The address as text (`1.2.3.4`, `[2001:db8::1]`); empty when unknown.
    pub fn ip_text(&self) -> String {
        match self.primary_ip() {
            Some(IpAddr::V6(a)) => format!("[{a}]"),
            Some(a) => a.to_string(),
            None => String::new(),
        }
    }

    /// The first 8 characters of the fingerprint (enough to tell relays apart in a list).
    pub fn short_fingerprint(&self) -> &str {
        self.fingerprint.get(..8).unwrap_or(&self.fingerprint)
    }
}

/// The path of the circuit that carries a site's traffic, first hop first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircuitInfo {
    pub hops: Vec<Hop>,
    /// The circuit leads to an onion service.
    pub onion: bool,
}

/// The roles of a path of `relays` relays, followed by `virtual_hops` hops that are not relays
/// (the onion service): `[Guard, Middle…, Exit]`, or `[Guard, Middle…, Rendezvous,
/// OnionService…]` for onion services.
pub fn roles_for(relays: usize, virtual_hops: usize) -> Vec<HopRole> {
    let onion = virtual_hops > 0;
    let mut roles = Vec::with_capacity(relays + virtual_hops);
    for i in 0..relays {
        roles.push(if i == 0 {
            HopRole::Guard
        } else if i + 1 == relays {
            if onion { HopRole::Rendezvous } else { HopRole::Exit }
        } else {
            HopRole::Middle
        });
    }
    roles.extend(std::iter::repeat_n(HopRole::OnionService, virtual_hops));
    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_of_a_normal_circuit() {
        use HopRole::*;
        assert_eq!(roles_for(3, 0), [Guard, Middle, Exit]);
        assert_eq!(roles_for(4, 0), [Guard, Middle, Middle, Exit]);
        assert_eq!(roles_for(2, 0), [Guard, Exit]);
        assert_eq!(roles_for(1, 0), [Guard]);
        assert!(roles_for(0, 0).is_empty());
    }

    #[test]
    fn roles_of_an_onion_circuit() {
        use HopRole::*;
        assert_eq!(roles_for(3, 1), [Guard, Middle, Rendezvous, OnionService]);
    }

    #[test]
    fn hop_formatting() {
        let hop = Hop {
            role: HopRole::Guard,
            fingerprint: "ABCDEF0123456789ABCDEF0123456789ABCDEF01".into(),
            addrs: vec!["2001:db8::1".parse().expect("ip"), "192.0.2.7".parse().expect("ip")],
            country: Some("DE".into()),
        };
        assert_eq!(hop.ip_text(), "192.0.2.7", "IPv4 is preferred");
        assert_eq!(hop.short_fingerprint(), "ABCDEF01");
        let v6 = Hop { addrs: vec!["2001:db8::1".parse().expect("ip")], ..hop.clone() };
        assert_eq!(v6.ip_text(), "[2001:db8::1]");
        let none = Hop { addrs: vec![], fingerprint: String::new(), ..hop };
        assert_eq!(none.ip_text(), "");
        assert_eq!(none.short_fingerprint(), "");
    }
}
