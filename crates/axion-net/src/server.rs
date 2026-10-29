//! Loopback servers in front of tunnels: a SOCKS5 server (what a tunnel exposes) and an
//! authenticating HTTP proxy (what Chromium is pointed at).
//!
//! Chromium cannot log in to a SOCKS5 proxy, but it answers an HTTP proxy's `407` with
//! credentials it is given out of band. So every tunnel is reached like this:
//!
//! ```text
//! Chromium --HTTP proxy 127.0.0.1:A (session login)--> front --SOCKS5 127.0.0.1:B (session
//! login)--> tunnel --> network
//! ```
//!
//! Both ports are random, bind to loopback only and demand a login that exists only in this
//! process, so no other program on the machine (or a sandboxed app sharing its network) can
//! route through them.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;

use crate::conn::{Address, Dialer, pipe};
use crate::secret::Credentials;
use crate::socks5::{self, SocksError, reply};

/// Limits of a loopback server.
#[derive(Clone, Copy, Debug)]
pub struct ServerOptions {
    /// Connections served at once; more are closed at once.
    pub max_connections: usize,
    /// How long a client may take to finish its greeting and request.
    pub handshake_timeout: Duration,
    /// The HTTP front also lets in connections from processes of this program's own tree
    /// without the login (see [`crate::peer`]); everyone else must log in.
    pub trust_own_process_tree: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            max_connections: 512,
            handshake_timeout: Duration::from_secs(15),
            trust_own_process_tree: false,
        }
    }
}

/// A running loopback server. Dropping it stops it.
pub struct LocalServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

type Handler = Arc<dyn Fn(TcpStream) + Send + Sync>;

impl LocalServer {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A SOCKS5 server that demands `credentials` and connects through `dialer`.
    pub fn socks5(
        credentials: Credentials,
        dialer: Arc<dyn Dialer>,
        options: ServerOptions,
    ) -> io::Result<Self> {
        let handler: Handler = Arc::new(move |client| {
            let _ = serve_socks(client, &credentials, dialer.as_ref(), options.handshake_timeout);
        });
        Self::start("net-socks", handler, options)
    }

    /// An HTTP proxy (CONNECT and plain `http://` requests) that demands `credentials`
    /// (Basic) and connects through `dialer`.
    pub fn http_front(
        credentials: Credentials,
        dialer: Arc<dyn Dialer>,
        options: ServerOptions,
    ) -> io::Result<Self> {
        let peers = options.trust_own_process_tree.then(crate::peer::PeerCheck::for_this_process);
        let handler: Handler = Arc::new(move |client| {
            let _ =
                serve_http(client, &credentials, dialer.as_ref(), options.handshake_timeout, peers.as_ref());
        });
        Self::start("net-front", handler, options)
    }

    /// A SOCKS5 server that refuses every request (the kill switch's black hole: whatever
    /// is pointed at it cannot reach anything).
    pub fn refusing(options: ServerOptions) -> io::Result<Self> {
        let handler: Handler = Arc::new(move |mut client| {
            let _ = client.set_read_timeout(Some(options.handshake_timeout));
            let _ = refuse(&mut client);
        });
        Self::start("net-sink", handler, options)
    }

    fn start(name: &str, handler: Handler, options: ServerOptions) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new().name(name.to_owned()).spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    if active.fetch_add(1, Ordering::AcqRel) >= options.max_connections {
                        active.fetch_sub(1, Ordering::AcqRel);
                        continue; // dropped: closed at once
                    }
                    let (handler, active) = (handler.clone(), active.clone());
                    let spawned = std::thread::Builder::new().name("net-conn".into()).spawn(move || {
                        let _ = stream.set_nodelay(true);
                        handler(stream);
                        active.fetch_sub(1, Ordering::AcqRel);
                    });
                    if spawned.is_err() {
                        tracing::warn!("could not start a connection thread");
                    }
                }
            })?
        };
        Ok(Self { addr, stop, thread: Some(thread) })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve_socks(
    mut client: TcpStream,
    creds: &Credentials,
    dialer: &dyn Dialer,
    timeout: Duration,
) -> io::Result<()> {
    client.set_read_timeout(Some(timeout))?;
    client.set_write_timeout(Some(timeout))?;
    let target = match socks5::server_handshake(&mut client, Some(creds)) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "local SOCKS5 client refused");
            return Ok(());
        }
    };
    match dialer.dial(&target) {
        Ok(upstream) => {
            socks5::server_reply(&mut client, reply::SUCCEEDED)?;
            client.set_read_timeout(None)?;
            client.set_write_timeout(None)?;
            pipe(Box::new(client), upstream);
        }
        Err(e) => {
            // The target is not logged: it is browsing history.
            tracing::debug!(error = %e, "tunnel could not connect");
            socks5::server_reply(&mut client, SocksError::Io(e).reply_code())?;
        }
    }
    Ok(())
}

fn refuse(client: &mut TcpStream) -> io::Result<()> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head)?;
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods)?;
    client.write_all(&[5, 0])?;
    let mut req = [0u8; 4];
    client.read_exact(&mut req)?;
    socks5::server_reply(client, reply::NOT_ALLOWED)
}

const MAX_HEAD: usize = 64 * 1024;

struct Head {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
    /// Bytes read past the head.
    rest: Vec<u8>,
}

fn read_head(client: &mut TcpStream, mut buf: Vec<u8>) -> io::Result<Option<Head>> {
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = find(&buf, b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            let text = String::from_utf8_lossy(&buf[..end]).into_owned();
            let mut lines = text.split("\r\n");
            let mut first = lines.next().unwrap_or_default().split_whitespace();
            let (Some(method), Some(target), Some(version)) = (first.next(), first.next(), first.next())
            else {
                return Ok(None);
            };
            let headers = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                .collect();
            return Ok(Some(Head {
                method: method.to_ascii_uppercase(),
                target: target.to_owned(),
                version: version.to_owned(),
                headers,
                rest,
            }));
        }
        if buf.len() > MAX_HEAD {
            return Ok(None);
        }
        let n = client.read(&mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend(&chunk[..n]);
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn authorized(head: &Head, creds: &Credentials) -> bool {
    head.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("proxy-authorization"))
        .and_then(|(_, v)| v.strip_prefix("Basic ").or_else(|| v.strip_prefix("basic ")))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok())
        .and_then(|raw| String::from_utf8(raw).ok())
        .and_then(|s| s.split_once(':').map(|(u, p)| creds.matches(u, p)))
        .unwrap_or(false)
}

fn serve_http(
    mut client: TcpStream,
    creds: &Credentials,
    dialer: &dyn Dialer,
    timeout: Duration,
    peers: Option<&crate::peer::PeerCheck>,
) -> io::Result<()> {
    client.set_read_timeout(Some(timeout))?;
    client.set_write_timeout(Some(timeout))?;
    let own = peers.is_some_and(|p| match (client.peer_addr(), client.local_addr()) {
        (Ok(peer), Ok(local)) => p.is_own(peer, local),
        _ => false,
    });
    let mut carry = Vec::new();
    let mut head = None;
    for _ in 0..4 {
        let Some(h) = read_head(&mut client, std::mem::take(&mut carry))? else {
            tracing::debug!("front: client closed before a request");
            return Ok(());
        };
        tracing::debug!(method = %h.method, own_process = own, "front: request head");
        if own || authorized(&h, creds) {
            head = Some(h);
            break;
        }
        // Chromium answers the challenge on the same connection.
        client.write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"Axion\"\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n",
        )?;
        carry = h.rest;
    }
    let Some(head) = head else { return Ok(()) };
    client.set_read_timeout(None)?;
    client.set_write_timeout(None)?;
    if head.method == "CONNECT" {
        let Some(target) = Address::parse_host_port(&head.target) else {
            return bad_request(&mut client);
        };
        match dialer.dial(&target) {
            Ok(mut upstream) => {
                client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
                if !head.rest.is_empty() {
                    upstream.write_all(&head.rest)?;
                }
                pipe(Box::new(client), upstream);
            }
            Err(e) => {
                tracing::debug!(error = %e, "tunnel could not connect");
                bad_gateway(&mut client)?;
            }
        }
        return Ok(());
    }
    // A plain http:// request: forward it in origin form, once (the connection is closed
    // after the response, so one client connection never talks to two hosts).
    let Some((target, path)) = split_absolute(&head.target) else {
        return bad_request(&mut client);
    };
    let mut upstream = match dialer.dial(&target) {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!(error = %e, "tunnel could not connect");
            return bad_gateway(&mut client);
        }
    };
    let mut out = format!("{} {} {}\r\n", head.method, path, head.version).into_bytes();
    for (k, v) in &head.headers {
        let lower = k.to_ascii_lowercase();
        if matches!(lower.as_str(), "proxy-authorization" | "proxy-connection" | "connection" | "keep-alive")
        {
            continue;
        }
        out.extend(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend(b"Connection: close\r\n\r\n");
    out.extend(&head.rest);
    upstream.write_all(&out)?;
    pipe(Box::new(client), upstream);
    Ok(())
}

/// `http://host[:port]/path` -> (address, `/path`).
fn split_absolute(target: &str) -> Option<(Address, String)> {
    let rest = target.strip_prefix("http://")?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes().get(i) == Some(&b'/') => (&rest[..i], rest[i..].to_owned()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_owned()),
    };
    let authority = authority.rsplit('@').next()?;
    let has_port =
        if authority.starts_with('[') { authority.contains("]:") } else { authority.contains(':') };
    let with_port = if has_port { authority.to_owned() } else { format!("{authority}:80") };
    Some((Address::parse_host_port(&with_port)?, path))
}

fn bad_request(client: &mut TcpStream) -> io::Result<()> {
    client.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
}

fn bad_gateway(client: &mut TcpStream) -> io::Result<()> {
    let body = "Axion could not reach this site through the connection.";
    client.write_all(
        format!(
            "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_targets() {
        let (a, p) = split_absolute("http://example.com/a/b?x=1").expect("parses");
        assert_eq!((a, p.as_str()), (Address::Domain("example.com".into(), 80), "/a/b?x=1"));
        let (a, p) = split_absolute("http://example.com:8080").expect("parses");
        assert_eq!((a, p.as_str()), (Address::Domain("example.com".into(), 8080), "/"));
        let (a, p) = split_absolute("http://example.com?q=1").expect("parses");
        assert_eq!((a, p.as_str()), (Address::Domain("example.com".into(), 80), "/?q=1"));
        let (a, _) = split_absolute("http://[::1]:81/x").expect("parses");
        assert_eq!(a, Address::Ip("[::1]:81".parse().expect("addr")));
        let (a, _) = split_absolute("http://u:p@h.example/").expect("parses");
        assert_eq!(a, Address::Domain("h.example".into(), 80));
        assert!(split_absolute("https://example.com/").is_none());
    }
}
