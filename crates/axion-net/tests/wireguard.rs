//! The userspace WireGuard backend against a loopback WireGuard server
//! ([`axion_net::wg_testing`]): handshake, DNS and TCP inside the tunnel, big transfers,
//! many parallel streams, rekeying, failure reporting, AllowedIPs. No root, no real network.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axion_net::wg_testing::{WgTestServer, WgTestServerConfig, public_key_of, random_private_key};
use axion_net::wireguard::{WgOptions, WireGuardTunnel};
use axion_net::*;
use base64::Engine as _;

const APP_IP: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);
const APP_IP6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 7);

fn b64(k: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(k)
}

fn serve(handler: impl Fn(TcpStream) + Send + Sync + 'static) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(s) = s else { continue };
            let handler = handler.clone();
            std::thread::spawn(move || handler(s));
        }
    });
    addr
}

/// The heavy tests run one at a time: on loopback they would drop each other's datagrams
/// (the kernel's UDP receive buffer holds about a hundred).
static HEAVY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn fnv(mut h: u64, data: &[u8]) -> u64 {
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

/// Deterministic pseudo-random bytes.
fn fill(state: &mut u64, buf: &mut [u8]) {
    for chunk in buf.chunks_mut(8) {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        let bytes = state.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

struct Inside {
    http: SocketAddr,
    echo: SocketAddr,
    source: SocketAddr,
    sink: SocketAddr,
    count: SocketAddr,
}

fn inside() -> Inside {
    let http = serve(|mut s| {
        let mut buf = [0u8; 4096];
        let mut seen: Vec<u8> = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match s.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => seen.extend(&buf[..n]),
            }
        }
        let body = "hello from inside the tunnel";
        let _ =
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    });
    let echo = serve(|mut s| {
        let mut buf = [0u8; 16384];
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 || s.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    });
    // Sends N pseudo-random bytes (N is the first 8 bytes the client sends), then closes.
    let source = serve(|mut s| {
        let mut n = [0u8; 8];
        if s.read_exact(&mut n).is_err() {
            return;
        }
        let total = u64::from_be_bytes(n);
        let mut state = total | 1;
        let mut buf = vec![0u8; 32768];
        let mut sent = 0u64;
        while sent < total {
            let take = (total - sent).min(buf.len() as u64) as usize;
            fill(&mut state, &mut buf[..take]);
            if s.write_all(&buf[..take]).is_err() {
                return;
            }
            sent += take as u64;
        }
    });
    // Reads N bytes, answers with their hash.
    let sink = serve(|mut s| {
        let mut n = [0u8; 8];
        if s.read_exact(&mut n).is_err() {
            return;
        }
        let mut left = u64::from_be_bytes(n);
        let mut h = FNV0;
        let mut buf = vec![0u8; 32768];
        while left > 0 {
            let take = left.min(buf.len() as u64) as usize;
            match s.read(&mut buf[..take]) {
                Ok(0) | Err(_) => return,
                Ok(k) => {
                    h = fnv(h, &buf[..k]);
                    left -= k as u64;
                }
            }
        }
        let _ = s.write_all(&h.to_be_bytes());
    });
    // Reads until the client is done sending, then answers with the byte count (half-close).
    let count = serve(|mut s| {
        let mut total = 0u64;
        let mut buf = [0u8; 8192];
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 {
                break;
            }
            total += n as u64;
        }
        let _ = s.write_all(&total.to_be_bytes());
    });
    Inside { http, echo, source, sink, count }
}

struct World {
    server: WgTestServer,
    config: WgTestServerConfig,
    inside: Inside,
    client_private: [u8; 32],
}

impl World {
    /// A new server with the same keys on the same port, and no memory of any session.
    fn restart_server(&mut self) {
        self.server.stop();
        let mut config = self.config.clone();
        config.bind = self.server.addr();
        let mut tries = 0;
        self.server = loop {
            match WgTestServer::start(config.clone()) {
                Ok(s) => break s,
                Err(e) if tries < 50 => {
                    tries += 1;
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => panic!("cannot restart the test server: {e}"),
            }
        };
    }
}

fn world_with(
    names: Vec<(String, IpAddr)>,
    expected_client: Option<[u8; 32]>,
    psk: Option<[u8; 32]>,
) -> World {
    let inside = inside();
    let client_private = random_private_key();
    let server_private = random_private_key();
    let (http, echo, source, sink, count) =
        (inside.http, inside.echo, inside.source, inside.sink, inside.count);
    let forward: axion_net::wg_testing::Forward = Arc::new(move |dest: SocketAddr| match dest.port() {
        80 => Some(http),
        7 => Some(echo),
        9000 => Some(source),
        9001 => Some(sink),
        8 => Some(count),
        _ => None,
    });
    let mut config = WgTestServerConfig::new(
        server_private,
        expected_client.unwrap_or_else(|| public_key_of(&client_private)),
        forward,
    );
    config.preshared = psk;
    config.tcp_ports = vec![(80, 8), (7, 140), (9000, 4), (9001, 4), (8, 4)];
    config.names = names;
    let server = WgTestServer::start(config.clone()).unwrap();
    World { server, config, inside, client_private }
}

fn default_names() -> Vec<(String, IpAddr)> {
    vec![("app.inside.test".into(), IpAddr::V4(APP_IP)), ("v6.inside.test".into(), IpAddr::V6(APP_IP6))]
}

fn world() -> World {
    world_with(default_names(), None, None)
}

fn client_config(w: &World, allowed: &[&str], dns: &[&str], psk: Option<[u8; 32]>) -> WgConfig {
    WgConfig {
        interface: WgInterface {
            private_key: Secret::new(b64(&w.client_private)),
            addresses: vec!["10.9.0.2/32".into(), "fd00:9::2/128".into()],
            dns: dns.iter().map(|d| (*d).to_owned()).collect(),
            mtu: 0,
            listen_port: 0,
        },
        peers: vec![WgPeer {
            public_key: b64(&w.server.public_key()),
            preshared_key: psk.map(|k| Secret::new(b64(&k))).unwrap_or_default(),
            has_preshared_key: psk.is_some(),
            allowed_ips: allowed.iter().map(|a| (*a).to_owned()).collect(),
            endpoint: w.server.addr().to_string(),
            persistent_keepalive: 0,
        }],
    }
}

fn opts(handshake_secs: u64) -> WgOptions {
    WgOptions {
        handshake_timeout: Duration::from_secs(handshake_secs),
        connect_timeout: Duration::from_secs(8),
        ..WgOptions::default()
    }
}

fn tunnel(config: &WgConfig, handshake_secs: u64) -> WireGuardTunnel {
    WireGuardTunnel::new_with(config, opts(handshake_secs), ServerOptions::default()).unwrap()
}

fn dial(endpoint: &LocalEndpoint, target: Address) -> std::io::Result<Box<dyn Conn>> {
    proxy::SocksUpstream::local(endpoint, Duration::from_secs(10)).dial(&target)
}

fn http_get(mut conn: Box<dyn Conn>) -> String {
    conn.write_all(b"GET / HTTP/1.1\r\nHost: app.inside.test\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    conn.read_to_string(&mut out).unwrap();
    out
}

fn open() -> (World, WireGuardTunnel, LocalEndpoint) {
    let w = world();
    let t = tunnel(&client_config(&w, &["0.0.0.0/0", "::/0"], &["10.9.0.1"], None), 10);
    let endpoint = t.start().unwrap();
    (w, t, endpoint)
}

#[test]
fn handshake_name_resolution_and_http_inside_the_tunnel() {
    let (w, t, endpoint) = open();
    assert!(t.status().is_up(), "{:?}", t.status());
    let TunnelStatus::Up { info, .. } = t.status() else { panic!() };
    assert!(info.contains(&w.server.addr().to_string()), "{info}");
    assert!(info.contains("last handshake"), "{info}");
    // The name only the tunnel's DNS knows (the local resolver would say NXDOMAIN).
    let page = http_get(dial(&endpoint, Address::Domain("app.inside.test".into(), 80)).unwrap());
    assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
    assert!(page.ends_with("hello from inside the tunnel"), "{page}");
    let queries = w.server.stats.dns_queries.lock().unwrap().clone();
    assert!(queries.iter().any(|q| q == "app.inside.test"), "{queries:?}");
    // A literal address, and the IPv6 name.
    let page = http_get(dial(&endpoint, Address::Ip("198.51.100.9:80".parse().unwrap())).unwrap());
    assert!(page.contains("hello from inside"), "{page}");
    let page = http_get(dial(&endpoint, Address::Domain("v6.inside.test".into(), 80)).unwrap());
    assert!(page.contains("hello from inside"), "{page}");
    let page = http_get(dial(&endpoint, Address::Ip("[2001:db8::99]:80".parse().unwrap())).unwrap());
    assert!(page.contains("hello from inside"), "{page}");
    // A name nobody knows.
    let err = dial(&endpoint, Address::Domain("nope.inside.test".into(), 80)).err().unwrap();
    assert!(err.to_string().contains("reply 4"), "{err}");
    // Nothing is listening on that inside port: refused.
    let err = dial(&endpoint, Address::Ip("198.51.100.9:81".parse().unwrap())).err().unwrap();
    assert!(err.to_string().contains("reply 5"), "{err}");
    let stats = t.stats().unwrap();
    assert!(stats.handshakes >= 1 && stats.rx_bytes > 0 && stats.tx_bytes > 0, "{stats:?}");
    t.stop();
    assert_eq!(t.status(), TunnelStatus::Stopped);
}

#[test]
fn preshared_key_and_tcp_echo() {
    let psk = random_private_key();
    let w = world_with(default_names(), None, Some(psk));
    let t = tunnel(&client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], Some(psk)), 10);
    let endpoint = t.start().unwrap();
    let mut conn = dial(&endpoint, Address::Ip("192.0.2.1:7".parse().unwrap())).unwrap();
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    conn.write_all(&data).unwrap();
    let mut got = vec![0u8; data.len()];
    conn.read_exact(&mut got).unwrap();
    assert_eq!(got, data);
    // A wrong preshared key never completes a handshake.
    let bad = tunnel(&client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], Some(random_private_key())), 2);
    assert!(bad.start().is_err());
}

fn transfer_check(endpoint: &LocalEndpoint, megabytes: u64) -> (f64, f64) {
    let total = megabytes * 1024 * 1024;
    // Download from the source.
    let t0 = Instant::now();
    let mut conn = dial(endpoint, Address::Ip("192.0.2.2:9000".parse().unwrap())).unwrap();
    conn.write_all(&total.to_be_bytes()).unwrap();
    let mut state = total | 1;
    let mut expected = FNV0;
    let mut want = vec![0u8; 32768];
    let mut got_total = 0u64;
    let mut buf = vec![0u8; 65536];
    let mut pending: Vec<u8> = Vec::new();
    while got_total < total {
        let n = conn.read(&mut buf).unwrap();
        assert!(n > 0, "stream ended after {got_total} of {total}");
        pending.extend_from_slice(&buf[..n]);
        got_total += n as u64;
        while pending.len() >= want.len() || (got_total == total && !pending.is_empty()) {
            let take = pending.len().min(want.len());
            let mut exp = vec![0u8; take];
            fill(&mut state, &mut exp);
            assert!(
                pending[..take] == exp[..],
                "data differs near byte {}",
                got_total - pending.len() as u64
            );
            expected = fnv(expected, &exp);
            pending.drain(..take);
        }
    }
    let _ = (expected, &mut want);
    let down = (total as f64 * 8.0 / 1e6) / t0.elapsed().as_secs_f64();
    // Upload to the sink.
    let t1 = Instant::now();
    let mut conn = dial(endpoint, Address::Ip("192.0.2.3:9001".parse().unwrap())).unwrap();
    conn.write_all(&total.to_be_bytes()).unwrap();
    let mut state = 12345u64;
    let mut hash = FNV0;
    let mut chunk = vec![0u8; 65536];
    let mut sent = 0u64;
    while sent < total {
        let take = (total - sent).min(chunk.len() as u64) as usize;
        fill(&mut state, &mut chunk[..take]);
        hash = fnv(hash, &chunk[..take]);
        conn.write_all(&chunk[..take]).unwrap();
        sent += take as u64;
    }
    let mut reply = [0u8; 8];
    conn.read_exact(&mut reply).unwrap();
    assert_eq!(u64::from_be_bytes(reply), hash, "the uploaded data arrived changed");
    let up = (total as f64 * 8.0 / 1e6) / t1.elapsed().as_secs_f64();
    (down, up)
}

#[test]
fn fifty_megabytes_each_way_arrive_intact_and_fast() {
    let _heavy = HEAVY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_w, t, endpoint) = open();
    let (down, up) = transfer_check(&endpoint, 50);
    eprintln!("loopback throughput: download {down:.0} Mbit/s, upload {up:.0} Mbit/s");
    let floor = if cfg!(debug_assertions) { 20.0 } else { 50.0 };
    assert!(down > floor && up > floor, "too slow: down {down:.0}, up {up:.0} Mbit/s (floor {floor})");
    t.stop();
}

#[test]
fn a_hundred_parallel_connections() {
    let _heavy = HEAVY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (w, t, endpoint) = open();
    let endpoint = Arc::new(endpoint);
    let started = Instant::now();
    let t = Arc::new(t);
    let w2 = Arc::new(w);
    {
        let t = t.clone();
        let w2 = w2.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(12));
            for _ in 0..3 {
                eprintln!(
                    "STALL DUMP: {:?} server_sent={}\n{}",
                    t.stats(),
                    w2.server.stats.sent_bytes.load(Ordering::Relaxed),
                    t.debug_dump()
                );
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }
    let handles: Vec<_> = (0..100u32)
        .map(|i| {
            let endpoint = endpoint.clone();
            std::thread::spawn(move || {
                // Alternate between names and addresses.
                let target = if i % 2 == 0 {
                    Address::Domain("app.inside.test".into(), 7)
                } else {
                    Address::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 7))
                };
                let mut conn = dial(&endpoint, target).unwrap();
                let data: Vec<u8> = (0..200_000u32).map(|n| ((n + i) % 253) as u8).collect();
                let writer = {
                    let mut w = conn.try_clone_conn().unwrap();
                    let data = data.clone();
                    std::thread::spawn(move || {
                        w.write_all(&data).unwrap();
                        w.shutdown_write().unwrap();
                    })
                };
                let mut got = Vec::new();
                conn.read_to_end(&mut got).unwrap();
                writer.join().unwrap();
                assert_eq!(got.len(), data.len());
                assert!(got == data, "stream {i} corrupted");
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    for l in std::fs::read_to_string("/proc/net/udp").unwrap_or_default().lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.last().is_some_and(|d| *d != "0") {
            eprintln!("UDP drops: local {} rx_queue {} drops {}", f[1], f[4], f.last().unwrap());
        }
    }
    eprintln!("100 parallel echoes of 200 kB: {:?}", started.elapsed());
    assert!(w2.server.stats.tcp_accepted.load(Ordering::Relaxed) >= 100);
    // Everything closes again.
    let start = Instant::now();
    while t.stats().unwrap().streams > 0 && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(t.stats().unwrap().streams, 0, "streams leaked");
}

#[test]
fn half_close_delivers_the_answer_after_our_fin() {
    let (_w, _t, endpoint) = open();
    let mut conn = dial(&endpoint, Address::Ip("192.0.2.5:8".parse().unwrap())).unwrap();
    conn.write_all(&vec![1u8; 70_000]).unwrap();
    conn.shutdown_write().unwrap();
    let mut reply = [0u8; 8];
    conn.read_exact(&mut reply).unwrap();
    assert_eq!(u64::from_be_bytes(reply), 70_000);
}

#[test]
fn rekey_keeps_streams_and_new_connections_working() {
    let (_w, t, endpoint) = open();
    let mut conn = dial(&endpoint, Address::Ip("192.0.2.1:7".parse().unwrap())).unwrap();
    let roundtrip = |conn: &mut Box<dyn Conn>, tag: u8| {
        let data = vec![tag; 20_000];
        conn.write_all(&data).unwrap();
        let mut got = vec![0u8; data.len()];
        conn.read_exact(&mut got).unwrap();
        assert_eq!(got, data);
    };
    roundtrip(&mut conn, 1);
    let before = t.stats().unwrap().handshakes;
    for _ in 0..3 {
        t.rekey_now();
        std::thread::sleep(Duration::from_millis(300));
        roundtrip(&mut conn, 2);
    }
    assert!(t.stats().unwrap().handshakes > before, "no new handshake happened");
    let mut fresh = dial(&endpoint, Address::Domain("app.inside.test".into(), 7)).unwrap();
    roundtrip(&mut fresh, 3);
    assert!(t.status().is_up());
}

#[test]
fn allowed_ips_refuse_everything_else_and_never_leak() {
    let w = world();
    // Only 10.0.0.0/8 (the DNS server's net) and one /32 are routed.
    let t = tunnel(&client_config(&w, &["10.0.0.0/8", "203.0.113.7/32"], &["10.9.0.1"], None), 10);
    let endpoint = t.start().unwrap();
    let accepted_before = w.server.stats.tcp_accepted.load(Ordering::Relaxed);
    // Outside: refused with "not allowed" (SOCKS reply 2), without any packet for it.
    let err = dial(&endpoint, Address::Ip("8.8.8.8:80".parse().unwrap())).err().unwrap();
    assert!(err.to_string().contains("reply 2"), "{err}");
    let err = dial(&endpoint, Address::Ip("[2001:db8::99]:80".parse().unwrap())).err().unwrap();
    assert!(!err.to_string().is_empty());
    assert_eq!(w.server.stats.tcp_accepted.load(Ordering::Relaxed), accepted_before);
    // Inside the allowed list: works, by name too (203.0.113.7 is allowed).
    let page = http_get(dial(&endpoint, Address::Domain("app.inside.test".into(), 80)).unwrap());
    assert!(page.contains("hello from inside"));
    // A name that resolves outside AllowedIPs (the v6 one) is refused as well.
    assert!(dial(&endpoint, Address::Domain("v6.inside.test".into(), 80)).is_err());
    // A host that is only on this computer (loopback) never connects locally either.
    assert!(
        dial(&endpoint, Address::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), w.inside.http.port())))
            .is_err()
    );
}

#[test]
fn without_a_dns_server_names_are_refused_not_resolved_locally() {
    let w = world();
    let t = tunnel(&client_config(&w, &["0.0.0.0/0"], &[], None), 10);
    let endpoint = t.start().unwrap();
    // `localhost` resolves on this computer; through the tunnel there is nobody to ask.
    let err = dial(&endpoint, Address::Domain("localhost".into(), w.inside.http.port())).err().unwrap();
    assert!(!err.to_string().is_empty());
    assert!(w.server.stats.dns_queries.lock().unwrap().is_empty());
}

#[test]
fn a_silent_server_reports_failed_after_the_handshake_timeout() {
    let w = world();
    w.server.set_blackhole(true);
    let t = tunnel(&client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None), 2);
    let started = Instant::now();
    let err = t.start().err().unwrap();
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
    let text = err.to_string();
    assert!(text.contains("No answer") && text.contains(&w.server.addr().to_string()), "{text}");
    assert!(matches!(t.status(), TunnelStatus::Failed(_)), "{:?}", t.status());
}

#[test]
fn a_wrong_key_looks_like_a_silent_server() {
    // The server knows a different client key: it drops the handshake without a word.
    let w = world_with(default_names(), Some(public_key_of(&random_private_key())), None);
    let t = tunnel(&client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None), 2);
    let err = t.start().err().unwrap();
    assert!(err.to_string().contains("No answer"), "{err}");
    assert!(err.to_string().contains("keys"), "{err}");
}

#[test]
fn no_server_at_the_endpoint_fails_too() {
    let mut w = world();
    let config = client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None);
    w.server.stop();
    let t = tunnel(&config, 2);
    assert!(t.start().is_err());
}

#[test]
fn stopping_wakes_blocked_streams_and_the_tunnel_can_start_again() {
    let (_w, t, endpoint) = open();
    let mut conn = dial(&endpoint, Address::Ip("192.0.2.1:7".parse().unwrap())).unwrap();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 16];
        conn.read(&mut buf).map(|n| n == 0).unwrap_or(true)
    });
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    t.stop();
    assert!(reader.join().unwrap());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(dial(&endpoint, Address::Ip("192.0.2.1:7".parse().unwrap())).is_err());
    let endpoint = t.start().unwrap();
    let page = http_get(dial(&endpoint, Address::Domain("app.inside.test".into(), 80)).unwrap());
    assert!(page.contains("hello"));
}

#[test]
fn bad_configs_say_what_is_wrong() {
    let w = world();
    let mut c = client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None);
    c.interface.private_key = Secret::new("not base64!");
    let err = WireGuardTunnel::new_with(&c, opts(5), ServerOptions::default()).err().unwrap();
    assert!(err.to_string().contains("PrivateKey"), "{err}");
    let mut c = client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None);
    c.interface.private_key = Secret::default();
    let err = WireGuardTunnel::new_with(&c, opts(5), ServerOptions::default()).err().unwrap();
    assert!(err.to_string().contains("private key"), "{err}");
    let mut c = client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None);
    c.peers[0].endpoint = "no-such-host.invalid:51820".into();
    let t = WireGuardTunnel::new_with(&c, opts(5), ServerOptions::default()).unwrap();
    let err = t.start().err().unwrap();
    assert!(err.to_string().contains("no-such-host.invalid"), "{err}");
}

/// The server restarts (it forgets all sessions): the client notices on its own, within about
/// the WireGuard retry time (15 s of silence plus a handshake), and works again.
#[test]
#[ignore = "takes about 30 seconds (WireGuard's own retry timers)"]
fn recovers_when_the_server_restarts() {
    let mut w = world();
    let config = client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None);
    let t = tunnel(&config, 10);
    let endpoint = t.start().unwrap();
    assert!(
        http_get(dial(&endpoint, Address::Domain("app.inside.test".into(), 80)).unwrap()).contains("hello")
    );
    w.restart_server();
    let started = Instant::now();
    // New connections work again once WireGuard has re-handshaken.
    let mut worked = false;
    while started.elapsed() < Duration::from_secs(60) {
        if let Ok(conn) = dial(&endpoint, Address::Domain("app.inside.test".into(), 80))
            && http_get(conn).contains("hello")
        {
            worked = true;
            break;
        }
    }
    eprintln!("recovered after {:?}", started.elapsed());
    assert!(worked, "the tunnel did not come back after the server restarted");
}

/// The path goes dark (sleep/wake, network change) and returns: the status turns failed and
/// then up again, and streams survive through TCP retransmission.
#[test]
#[ignore = "takes about 30 seconds"]
fn fails_in_the_dark_and_comes_back() {
    let w = world();
    let config = client_config(&w, &["0.0.0.0/0"], &["10.9.0.1"], None);
    let t = tunnel(&config, 3);
    let endpoint = t.start().unwrap();
    let mut conn = dial(&endpoint, Address::Ip("192.0.2.1:7".parse().unwrap())).unwrap();
    w.server.set_blackhole(true);
    conn.write_all(b"ping").unwrap();
    let started = Instant::now();
    while !matches!(t.status(), TunnelStatus::Failed(_)) && started.elapsed() < Duration::from_secs(40) {
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(matches!(t.status(), TunnelStatus::Failed(_)), "{:?}", t.status());
    w.server.set_blackhole(false);
    let started = Instant::now();
    while !t.status().is_up() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(t.status().is_up(), "{:?}", t.status());
    let mut got = [0u8; 4];
    conn.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"ping");
}
