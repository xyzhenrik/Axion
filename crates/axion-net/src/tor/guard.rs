//! What may be asked of Tor at all: no local or private addresses.
//!
//! Tor cannot reach `127.0.0.1` or `192.168.x.x` (an exit relay would connect to *its own*
//! network), and asking would tell the exit relay what the user tried to open. Such requests
//! are refused before they leave this computer.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use crate::conn::{Address, Conn, Dialer};

/// Why a target is refused, `None` if Tor may be asked.
pub fn refusal(target: &Address) -> Option<&'static str> {
    if target.port() == 0 {
        return Some("port 0 is not a destination");
    }
    match target {
        Address::Ip(addr) => ip_refusal(addr.ip()),
        Address::Domain(host, _) => {
            let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
            if host.is_empty() {
                return Some("the host name is empty");
            }
            // A numeric host that arrived as a name ("127.1" aside, the standard forms).
            if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
                return ip_refusal(ip);
            }
            if host == "localhost" || host.ends_with(".localhost") {
                return Some("localhost is this computer");
            }
            None
        }
    }
}

fn ip_refusal(ip: IpAddr) -> Option<&'static str> {
    let local = match ip {
        IpAddr::V4(a) => is_local_v4(a),
        IpAddr::V6(a) => is_local_v6(a),
    };
    local.then_some("local and private addresses cannot be reached through Tor")
}

fn is_local_v4(a: Ipv4Addr) -> bool {
    let [o0, o1, ..] = a.octets();
    a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_unspecified()
        || a.is_broadcast()
        || a.is_multicast()
        || o0 == 0
        || (o0 == 100 && (64..128).contains(&o1)) // carrier-grade NAT
        || (o0 == 192 && o1 == 0 && a.octets()[2] == 0) // IETF protocol assignments
        || o0 >= 240
}

fn is_local_v6(a: Ipv6Addr) -> bool {
    if let Some(v4) = a.to_ipv4_mapped() {
        return is_local_v4(v4);
    }
    let first = a.segments()[0];
    a.is_loopback()
        || a.is_unspecified()
        || a.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local
        || (first & 0xffc0) == 0xfe80 // link local
}

/// Wraps a dialer: refused targets never reach it.
pub struct GuardedDialer<D> {
    inner: D,
}

impl<D: Dialer> GuardedDialer<D> {
    pub fn new(inner: D) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

impl<D: Dialer> Dialer for GuardedDialer<D> {
    fn dial(&self, target: &Address) -> io::Result<Box<dyn Conn>> {
        if let Some(why) = refusal(target) {
            // The target is not logged: it is browsing history.
            tracing::debug!(why, "refused a request for Tor");
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
        }
        self.inner.dial(target)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpStream;
    use std::sync::Mutex;

    use super::*;
    use crate::secret::Credentials;
    use crate::server::{LocalServer, ServerOptions};
    use crate::socks5::{self, SocksError, reply};

    fn domain(h: &str) -> Address {
        Address::Domain(h.into(), 443)
    }

    fn ip(s: &str) -> Address {
        Address::Ip(s.parse().expect("socket address"))
    }

    #[test]
    fn local_and_private_targets_are_refused() {
        for t in [
            ip("127.0.0.1:80"),
            ip("10.1.2.3:80"),
            ip("192.168.0.1:80"),
            ip("172.16.5.5:80"),
            ip("169.254.1.1:80"),
            ip("0.0.0.0:80"),
            ip("100.64.0.1:80"),
            ip("[::1]:80"),
            ip("[fe80::1]:80"),
            ip("[fd00::1]:80"),
            ip("[::ffff:127.0.0.1]:80"),
            domain("localhost"),
            domain("LOCALHOST."),
            domain("app.localhost"),
            domain("127.0.0.1"),
            domain("[::1]"),
            Address::Domain("example.com".into(), 0),
            Address::Domain(String::new(), 80),
        ] {
            assert!(refusal(&t).is_some(), "{t} must be refused");
        }
    }

    #[test]
    fn public_names_onions_and_addresses_pass() {
        for t in [
            domain("example.com"),
            domain("www.example.co.uk"),
            domain("duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion"),
            domain("localhost.example.com"),
            ip("93.184.216.34:443"),
            ip("[2606:2800:220:1:248:1893:25c8:1946]:443"),
        ] {
            assert!(refusal(&t).is_none(), "{t} must pass");
        }
    }

    /// Records what reached it, answers with an echo-less closed stream.
    struct Probe(Mutex<Vec<String>>);

    impl Dialer for Arc<Probe> {
        fn dial(&self, target: &Address) -> io::Result<Box<dyn Conn>> {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(target.to_string());
            // Any stream will do: connect to our own listener.
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            let stream = TcpStream::connect(listener.local_addr()?)?;
            Ok(Box::new(stream))
        }
    }

    #[test]
    fn socks5_front_refuses_numeric_local_and_forwards_onions() {
        let probe = Arc::new(Probe(Mutex::new(Vec::new())));
        let creds = Credentials::random_session();
        let server = LocalServer::socks5(
            creds.clone(),
            GuardedDialer::new(probe.clone()),
            ServerOptions::default(),
        )
        .expect("server");
        let ask = |target: Address| -> Result<(), SocksError> {
            let mut s = TcpStream::connect(server.addr()).expect("connect");
            socks5::client_connect(&mut s, &target, Some(&creds))
        };
        let refused = |r: Result<(), SocksError>| matches!(r, Err(SocksError::Refused(c)) if c == reply::NOT_ALLOWED);
        assert!(refused(ask(ip("127.0.0.1:8080"))));
        assert!(refused(ask(ip("192.168.1.1:80"))));
        assert!(refused(ask(domain("localhost"))));
        let onion = "duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion";
        assert!(ask(domain(onion)).is_ok(), "onion names pass through by name");
        assert!(ask(domain("example.com")).is_ok());
        let seen = probe.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        assert_eq!(seen, [format!("{onion}:443"), "example.com:443".to_owned()], "only allowed targets reach Tor");
    }
}
