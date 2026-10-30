//! The loopback relay, the HTTP front and the manager against in-process mock proxies.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axion_net::testing::{Upstream, echo_server, http_server, mock_http_connect_proxy, mock_socks5};
use axion_net::*;

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn proxy_conn(id: &str, port: u16, user: &str, scheme: ProxyScheme) -> Connection {
    Connection {
        id: id.into(),
        name: format!("Proxy {id}"),
        kind: ConnectionKind::Proxy(ProxyConfig {
            scheme,
            host: "127.0.0.1".into(),
            port,
            username: user.into(),
        }),
    }
}

#[derive(Default)]
struct MemSecrets {
    map: Mutex<HashMap<String, ConnectionSecrets>>,
    locked: Mutex<bool>,
}

impl SecretStore for MemSecrets {
    fn load(&self, id: &str) -> Result<ConnectionSecrets, SecretError> {
        if *self.locked.lock().unwrap() {
            return Err(SecretError::Locked);
        }
        Ok(self.map.lock().unwrap().get(id).cloned().unwrap_or_default())
    }

    fn save(&self, id: &str, s: &ConnectionSecrets) -> Result<(), SecretError> {
        self.map.lock().unwrap().insert(id.into(), s.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        self.map.lock().unwrap().remove(id);
        Ok(())
    }
}

fn fast_options() -> NetOptions {
    NetOptions {
        timings: ProxyTimings {
            connect_timeout: Duration::from_secs(2),
            health_interval: Duration::from_millis(200),
        },
        ..Default::default()
    }
}

fn roundtrip(stream: &mut TcpStream, data: &[u8]) -> Vec<u8> {
    stream.write_all(data).unwrap();
    let mut got = vec![0u8; data.len()];
    stream.read_exact(&mut got).unwrap();
    got
}

#[test]
fn socks5_client_through_mock_proxy_sends_names_and_logs_in() {
    let echo = echo_server().unwrap();
    let creds = Credentials::new("bob", "s3cret");
    let proxy = mock_socks5(Some(creds.clone()), Upstream { connect_all_to: Some(echo.addr()) }).unwrap();

    let upstream = proxy::SocksUpstream::new(
        ProxyConfig {
            host: "127.0.0.1".into(),
            port: proxy.addr().port(),
            username: "bob".into(),
            ..Default::default()
        },
        &Secret::new("s3cret"),
        Duration::from_secs(2),
    );
    let conn = upstream.dial(&Address::Domain("never-resolved.invalid".into(), 443)).unwrap();
    // Wrap the boxed conn: write through it.
    let mut conn = conn;
    conn.write_all(b"hello").unwrap();
    let mut got = [0u8; 5];
    conn.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"hello");
    // The proxy was asked for the NAME: nothing was resolved on this computer.
    assert_eq!(proxy.requests(), ["never-resolved.invalid:443"]);

    // Wrong password: refused.
    let bad = proxy::SocksUpstream::new(
        ProxyConfig {
            host: "127.0.0.1".into(),
            port: proxy.addr().port(),
            username: "bob".into(),
            ..Default::default()
        },
        &Secret::new("wrong"),
        Duration::from_secs(2),
    );
    assert!(bad.dial(&Address::Domain("x.invalid".into(), 1)).is_err());
    assert_eq!(proxy.requests().len(), 1);
}

#[test]
fn proxy_tunnel_relays_with_a_session_login() {
    let echo = echo_server().unwrap();
    let proxy =
        mock_socks5(Some(Credentials::new("bob", "pw")), Upstream { connect_all_to: Some(echo.addr()) })
            .unwrap();
    let tunnel = proxy::ProxyTunnel::new(
        ProxyConfig {
            host: "127.0.0.1".into(),
            port: proxy.addr().port(),
            username: "bob".into(),
            ..Default::default()
        },
        Secret::new("pw"),
        ProxyTimings::default(),
        ServerOptions::default(),
    );
    let endpoint = tunnel.start().unwrap();
    assert!(endpoint.addr.ip().is_loopback());
    assert!(tunnel.status().is_up());

    // Through the relay with the session login.
    let mut s = TcpStream::connect(endpoint.addr).unwrap();
    socks5::client_connect(&mut s, &Address::Domain("example.test".into(), 80), Some(&endpoint.credentials))
        .unwrap();
    assert_eq!(roundtrip(&mut s, b"ping"), b"ping");
    assert_eq!(proxy.requests(), ["example.test:80"]);

    // Without the login, or with a wrong one: refused, and the upstream is never contacted.
    let mut anon = TcpStream::connect(endpoint.addr).unwrap();
    assert!(socks5::client_connect(&mut anon, &Address::Domain("evil.test".into(), 80), None).is_err());
    let mut wrong = TcpStream::connect(endpoint.addr).unwrap();
    let guess = Credentials::new("bob", "pw");
    assert!(
        socks5::client_connect(&mut wrong, &Address::Domain("evil.test".into(), 80), Some(&guess)).is_err()
    );
    assert_eq!(proxy.requests(), ["example.test:80"]);

    tunnel.stop();
    assert_eq!(tunnel.status(), TunnelStatus::Stopped);
    wait_for("relay closed", || {
        TcpStream::connect_timeout(&endpoint.addr, Duration::from_millis(200)).is_err()
    });
}

#[test]
fn http_front_serves_connect_and_plain_requests_only_with_the_login() {
    let web = http_server("<h1>hi</h1>", "127.0.0.1:0".parse().unwrap()).unwrap();
    let proxy = mock_socks5(None, Upstream { connect_all_to: Some(web.addr()) }).unwrap();
    let dialer = Arc::new(proxy::SocksUpstream::new(
        ProxyConfig { host: "127.0.0.1".into(), port: proxy.addr().port(), ..Default::default() },
        &Secret::default(),
        Duration::from_secs(2),
    ));
    let session = Credentials::new("axion-1", "pw1");
    let front = LocalServer::http_front(session.clone(), dialer, ServerOptions::default()).unwrap();

    // Plain request without login: 407 challenge.
    let mut s = TcpStream::connect(front.addr()).unwrap();
    s.write_all(b"GET http://site.example/a HTTP/1.1\r\nHost: site.example\r\n\r\n").unwrap();
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 407"));
    // The same connection answers the challenge, as Chromium does.
    use base64::Engine as _;
    let token = base64::engine::general_purpose::STANDARD.encode("axion-1:pw1");
    s.write_all(
        format!("GET http://site.example/a HTTP/1.1\r\nHost: site.example\r\nProxy-Authorization: Basic {token}\r\nProxy-Connection: keep-alive\r\n\r\n")
            .as_bytes(),
    )
    .unwrap();
    let mut body = String::new();
    s.read_to_string(&mut body).unwrap();
    assert!(body.starts_with("HTTP/1.1 200 OK") && body.contains("<h1>hi</h1>"), "{body}");
    assert_eq!(proxy.requests(), ["site.example:80"]);
    assert!(web.requests()[0].starts_with("GET /a HTTP/1.1"), "origin form: {:?}", web.requests());
    assert!(!web.requests()[0].contains("Proxy-"), "proxy headers not forwarded");

    // Wrong login: never reaches the proxy.
    let bad = base64::engine::general_purpose::STANDARD.encode("axion-1:nope");
    let mut s = TcpStream::connect(front.addr()).unwrap();
    s.write_all(
        format!("CONNECT secret.example:443 HTTP/1.1\r\nProxy-Authorization: Basic {bad}\r\n\r\n").as_bytes(),
    )
    .unwrap();
    let mut out = Vec::new();
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.read_to_end(&mut out);
    assert!(!String::from_utf8_lossy(&out).contains("200 Connection Established"));
    assert_eq!(proxy.requests().len(), 1);
}

#[test]
fn http_front_connect_tunnels_bytes() {
    let echo = echo_server().unwrap();
    let proxy = mock_socks5(None, Upstream { connect_all_to: Some(echo.addr()) }).unwrap();
    let dialer = Arc::new(proxy::SocksUpstream::new(
        ProxyConfig { host: "127.0.0.1".into(), port: proxy.addr().port(), ..Default::default() },
        &Secret::default(),
        Duration::from_secs(2),
    ));
    let session = Credentials::new("u", "p");
    let front = LocalServer::http_front(session, dialer, ServerOptions::default()).unwrap();
    use base64::Engine as _;
    let token = base64::engine::general_purpose::STANDARD.encode("u:p");
    let mut s = TcpStream::connect(front.addr()).unwrap();
    s.write_all(format!("CONNECT tls.example:443 HTTP/1.1\r\nHost: tls.example:443\r\nProxy-Authorization: Basic {token}\r\n\r\n").as_bytes())
        .unwrap();
    let mut head = vec![0u8; "HTTP/1.1 200 Connection Established\r\n\r\n".len()];
    s.read_exact(&mut head).unwrap();
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"));
    assert_eq!(roundtrip(&mut s, b"tunnelled bytes"), b"tunnelled bytes");
    assert_eq!(proxy.requests(), ["tls.example:443"]);
}

#[cfg(target_os = "linux")]
#[test]
fn http_front_lets_in_own_processes_without_login_only_when_asked() {
    let echo = echo_server().unwrap();
    let proxy = mock_socks5(None, Upstream { connect_all_to: Some(echo.addr()) }).unwrap();
    let dialer = || {
        Arc::new(proxy::SocksUpstream::new(
            ProxyConfig { host: "127.0.0.1".into(), port: proxy.addr().port(), ..Default::default() },
            &Secret::default(),
            Duration::from_secs(2),
        ))
    };
    let ask = |front: &LocalServer| {
        let mut s = TcpStream::connect(front.addr()).unwrap();
        s.write_all(b"CONNECT own.example:443 HTTP/1.1\r\nHost: own.example:443\r\n\r\n").unwrap();
        let mut buf = [0u8; 12];
        s.read_exact(&mut buf).unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    };
    // This test process is its own process tree: trusted only when the option is on.
    let trusting = ServerOptions { trust_own_process_tree: true, ..Default::default() };
    let front = LocalServer::http_front(Credentials::new("u", "p"), dialer(), trusting).unwrap();
    assert!(ask(&front).starts_with("HTTP/1.1 200"), "a process of our own tree needs no login");
    let strict =
        LocalServer::http_front(Credentials::new("u", "p"), dialer(), ServerOptions::default()).unwrap();
    assert!(ask(&strict).starts_with("HTTP/1.1 407"), "without the option everyone must log in");
}

#[test]
fn mock_http_connect_proxy_checks_login() {
    let echo = echo_server().unwrap();
    let creds = Credentials::new("a", "b");
    let proxy = mock_http_connect_proxy(Some(creds), Upstream { connect_all_to: Some(echo.addr()) }).unwrap();
    use base64::Engine as _;
    let mut s = TcpStream::connect(proxy.addr()).unwrap();
    s.write_all(b"CONNECT h.example:443 HTTP/1.1\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 407"));
    let token = base64::engine::general_purpose::STANDARD.encode("a:b");
    let mut s = TcpStream::connect(proxy.addr()).unwrap();
    s.write_all(
        format!("CONNECT h.example:443 HTTP/1.1\r\nProxy-Authorization: Basic {token}\r\n\r\n").as_bytes(),
    )
    .unwrap();
    let mut head = vec![0u8; "HTTP/1.1 200 Connection Established\r\n\r\n".len()];
    s.read_exact(&mut head).unwrap();
    assert_eq!(roundtrip(&mut s, b"x"), b"x");
    assert_eq!(proxy.requests(), ["h.example:443"]);
}

fn routing_with(conn: Connection, kill_switch: bool) -> RoutingConfig {
    RoutingConfig {
        default: Target::Connection(conn.id.clone()),
        connections: vec![conn],
        kill_switch,
        ..Default::default()
    }
}

fn start_and_wait(m: &NetManager, id: &str) -> ConnectionStatus {
    m.start(id);
    wait_for("connection settled", || !matches!(m.status(id), Some(ConnectionStatus::Connecting)));
    m.status(id).unwrap()
}

#[test]
fn manager_plain_socks_proxy_is_given_to_chromium_directly() {
    let proxy = mock_socks5(None, Upstream::default()).unwrap();
    let m = NetManager::new(Arc::new(MemSecrets::default()), fast_options());
    m.set_routing(routing_with(proxy_conn("c-1", proxy.addr().port(), "", ProxyScheme::Socks5), true));
    assert!(start_and_wait(&m, "c-1").is_up());
    let plan = m.plan();
    assert_eq!(
        plan.normal.setting,
        ProxySetting::Fixed(format!("socks5://127.0.0.1:{}", proxy.addr().port()))
    );
    assert!(plan.normal.leak_protection && plan.normal.down.is_empty());
    assert!(plan.logins.is_empty());
    m.shutdown();
}

#[test]
fn manager_kill_switch_blocks_when_the_proxy_goes_down() {
    let mut proxy = mock_socks5(None, Upstream::default()).unwrap();
    let port = proxy.addr().port();
    let m = NetManager::new(Arc::new(MemSecrets::default()), fast_options());
    m.set_routing(routing_with(proxy_conn("c-1", port, "", ProxyScheme::Socks5), true));
    assert!(start_and_wait(&m, "c-1").is_up());

    proxy.stop();
    drop(proxy);
    wait_for("failure noticed", || {
        m.poll();
        matches!(m.status("c-1"), Some(ConnectionStatus::Failed(_)))
    });
    let plan = m.plan();
    let ProxySetting::Fixed(server) = &plan.normal.setting else { panic!("{:?}", plan.normal) };
    assert!(server.starts_with("socks5://127.0.0.1:"), "{server}");
    assert_ne!(server, &format!("socks5://127.0.0.1:{port}"), "points at the sink, not the dead proxy");
    assert_eq!(plan.normal.down, ["c-1"]);

    // The sink refuses everything.
    let sink = m.sink_addr().unwrap();
    let mut s = TcpStream::connect(sink).unwrap();
    assert!(matches!(
        socks5::client_connect(&mut s, &Address::Domain("example.com".into(), 443), None),
        Err(socks5::SocksError::Refused(_))
    ));

    // Kill switch off: explicit opt-in to falling back to direct.
    let mut r = m.routing();
    r.kill_switch = false;
    m.set_routing(r);
    assert_eq!(m.plan().normal.setting, ProxySetting::Direct);
    m.shutdown();
}

#[test]
fn manager_authenticated_socks_goes_through_the_front_with_session_login() {
    let echo = echo_server().unwrap();
    let proxy =
        mock_socks5(Some(Credentials::new("bob", "pw")), Upstream { connect_all_to: Some(echo.addr()) })
            .unwrap();
    let secrets = Arc::new(MemSecrets::default());
    secrets.save("c-1", &ConnectionSecrets { password: Secret::new("pw"), wireguard: None }).unwrap();
    let m = NetManager::new(secrets.clone(), fast_options());
    m.set_routing(routing_with(proxy_conn("c-1", proxy.addr().port(), "bob", ProxyScheme::Socks5), true));
    assert!(start_and_wait(&m, "c-1").is_up());
    let plan = m.plan();
    let ProxySetting::Fixed(server) = &plan.normal.setting else { panic!() };
    assert!(server.starts_with("http://127.0.0.1:"), "Chromium gets the authenticating front: {server}");
    let login = &plan.logins[0];
    assert!(server.ends_with(&format!(":{}", login.port)));

    // Act like Chromium: CONNECT, 407, retry with the login it was given.
    use base64::Engine as _;
    let token = base64::engine::general_purpose::STANDARD.encode(format!(
        "{}:{}",
        login.credentials.username,
        login.credentials.password.expose()
    ));
    let mut s = TcpStream::connect(("127.0.0.1", login.port)).unwrap();
    s.write_all(
        format!("CONNECT page.example:443 HTTP/1.1\r\nProxy-Authorization: Basic {token}\r\n\r\n").as_bytes(),
    )
    .unwrap();
    let mut head = vec![0u8; "HTTP/1.1 200 Connection Established\r\n\r\n".len()];
    s.read_exact(&mut head).unwrap();
    assert_eq!(roundtrip(&mut s, b"abc"), b"abc");
    assert_eq!(proxy.requests(), ["page.example:443"]);
    m.shutdown();
}

#[test]
fn manager_http_proxy_login_is_for_chromium_to_answer() {
    let echo = echo_server().unwrap();
    let proxy = mock_http_connect_proxy(None, Upstream { connect_all_to: Some(echo.addr()) }).unwrap();
    let secrets = Arc::new(MemSecrets::default());
    secrets.save("c-1", &ConnectionSecrets { password: Secret::new("pw"), wireguard: None }).unwrap();
    let m = NetManager::new(secrets, fast_options());
    m.set_routing(routing_with(proxy_conn("c-1", proxy.addr().port(), "bob", ProxyScheme::Http), true));
    assert!(start_and_wait(&m, "c-1").is_up());
    let plan = m.plan();
    assert_eq!(plan.normal.setting, ProxySetting::Fixed(format!("http://127.0.0.1:{}", proxy.addr().port())));
    assert_eq!(plan.logins.len(), 1);
    assert_eq!(plan.logins[0].credentials.username, "bob");
    assert_eq!(plan.logins[0].credentials.password.expose(), "pw");
    m.shutdown();
}

#[test]
fn manager_blocks_when_the_vault_is_locked_or_the_config_is_invalid() {
    let secrets = Arc::new(MemSecrets::default());
    let m = NetManager::new(secrets.clone(), fast_options());
    let mut routing = routing_with(proxy_conn("c-1", 1080, "bob", ProxyScheme::Socks5), true);
    routing.connections.push(Connection {
        id: "c-2".into(),
        name: "VPN".into(),
        // Invalid on purpose: a backend refuses to start a connection that does not validate.
        kind: ConnectionKind::Tor(TorConfig { use_bridges: true, ..TorConfig::default() }),
    });
    m.set_routing(routing);
    *secrets.locked.lock().unwrap() = true;
    let ConnectionStatus::Blocked(reason) = start_and_wait(&m, "c-1") else { panic!() };
    assert!(reason.contains("vault"), "{reason}");
    *secrets.locked.lock().unwrap() = false;
    let ConnectionStatus::Blocked(reason) = start_and_wait(&m, "c-2") else { panic!() };
    assert!(reason.contains("bridge"), "{reason}");
    // Not up: the context is blocked, never direct.
    let plan = m.plan();
    assert!(matches!(plan.normal.setting, ProxySetting::Fixed(_)));
    assert_eq!(plan.normal.down, ["c-1"]);
    m.shutdown();
}

#[test]
fn manager_builds_a_pac_for_site_rules_and_private_default() {
    let a = mock_socks5(None, Upstream::default()).unwrap();
    let b = mock_socks5(None, Upstream::default()).unwrap();
    let m = NetManager::new(Arc::new(MemSecrets::default()), fast_options());
    let mut routing = routing_with(proxy_conn("a", a.addr().port(), "", ProxyScheme::Socks5), true);
    routing.connections.push(proxy_conn("b", b.addr().port(), "", ProxyScheme::Socks5));
    routing.private = Some(Target::Connection("b".into()));
    routing.set_site_rule("direct.example", Some(Target::Direct));
    m.set_routing(routing);
    assert!(start_and_wait(&m, "a").is_up());
    assert!(start_and_wait(&m, "b").is_up());
    let plan = m.plan();
    let ProxySetting::Pac(js) = &plan.normal.setting else { panic!("{:?}", plan.normal) };
    assert!(js.contains("'direct.example'") && js.contains("return 'DIRECT'"));
    assert!(js.contains(&format!("SOCKS5 127.0.0.1:{}", a.addr().port())));
    let ProxySetting::Pac(pjs) = &plan.private.setting else { panic!() };
    assert!(pjs.contains(&format!("SOCKS5 127.0.0.1:{}", b.addr().port())));
    assert!(!pjs.contains(&format!("{}", a.addr().port())));
    m.shutdown();
}
