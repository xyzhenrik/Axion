//! In-process fixture servers for tests and the end-to-end example: a mock SOCKS5 proxy, a
//! mock HTTP `CONNECT` proxy and an echo server. Not used by the browser itself.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;

use crate::conn::{Address, pipe};
use crate::secret::Credentials;
use crate::socks5::{self, reply};

fn log_lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How a mock proxy reaches "the internet".
#[derive(Clone, Debug, Default)]
pub struct Upstream {
    /// Every request connects here, whatever name it asked for (the name is only logged).
    /// `None`: connect to the requested address for real (IP literals only).
    pub connect_all_to: Option<SocketAddr>,
}

/// A running fixture server.
pub struct Fixture {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    /// The targets clients asked for, in order.
    pub requests: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<String> {
        log_lock(&self.requests).clone()
    }

    /// Stops accepting and closes the listener.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(300));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn spawn(handler: impl Fn(TcpStream, &Mutex<Vec<String>>) + Send + Sync + 'static) -> io::Result<Self> {
        Self::spawn_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)), handler)
    }

    fn spawn_on(
        bind: SocketAddr,
        handler: impl Fn(TcpStream, &Mutex<Vec<String>>) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let thread = {
            let (stop, requests) = (stop.clone(), requests.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (handler, requests) = (handler.clone(), requests.clone());
                    std::thread::spawn(move || handler(stream, &requests));
                }
            })
        };
        Ok(Self { addr, stop, requests, thread: Some(thread) })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn connect_upstream(upstream: &Upstream, target: &Address) -> io::Result<TcpStream> {
    let to = match (&upstream.connect_all_to, target) {
        (Some(a), _) => *a,
        (None, Address::Ip(a)) => *a,
        (None, Address::Domain(..)) => return Err(io::Error::other("the mock does not resolve names")),
    };
    TcpStream::connect_timeout(&to, Duration::from_secs(5))
}

/// A SOCKS5 proxy. With `credentials` it demands a login.
pub fn mock_socks5(credentials: Option<Credentials>, upstream: Upstream) -> io::Result<Fixture> {
    Fixture::spawn(move |mut client, requests| {
        let _ = client.set_read_timeout(Some(Duration::from_secs(10)));
        let Ok(target) = socks5::server_handshake(&mut client, credentials.as_ref()) else { return };
        log_lock(requests).push(target.to_string());
        match connect_upstream(&upstream, &target) {
            Ok(up) => {
                if socks5::server_reply(&mut client, reply::SUCCEEDED).is_err() {
                    return;
                }
                let _ = client.set_read_timeout(None);
                pipe(Box::new(client), Box::new(up));
            }
            Err(_) => {
                let _ = socks5::server_reply(&mut client, reply::HOST_UNREACHABLE);
            }
        }
    })
}

/// An HTTP proxy that supports `CONNECT` (and `Proxy-Authorization: Basic` when
/// `credentials` is set).
pub fn mock_http_connect_proxy(credentials: Option<Credentials>, upstream: Upstream) -> io::Result<Fixture> {
    Fixture::spawn(move |mut client, requests| {
        let _ = client.set_read_timeout(Some(Duration::from_secs(10)));
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            match client.read(&mut byte) {
                Ok(1) => buf.push(byte[0]),
                _ => return,
            }
            if buf.len() > 16 * 1024 {
                return;
            }
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let mut lines = text.split("\r\n");
        let mut first = lines.next().unwrap_or_default().split_whitespace();
        let (method, target) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
        if let Some(creds) = &credentials {
            let ok = lines
                .filter_map(|l| l.split_once(':'))
                .find(|(k, _)| k.eq_ignore_ascii_case("proxy-authorization"))
                .and_then(|(_, v)| v.trim().strip_prefix("Basic "))
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b.trim()).ok())
                .and_then(|r| String::from_utf8(r).ok())
                .and_then(|s| s.split_once(':').map(|(u, p)| creds.matches(u, p)))
                .unwrap_or(false);
            if !ok {
                let _ = client.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"mock\"\r\nContent-Length: 0\r\n\r\n");
                return;
            }
        }
        if method != "CONNECT" {
            let _ = client.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        log_lock(requests).push(target.to_owned());
        let Some(address) = Address::parse_host_port(target) else { return };
        match connect_upstream(&upstream, &address) {
            Ok(up) => {
                if client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").is_err() {
                    return;
                }
                let _ = client.set_read_timeout(None);
                pipe(Box::new(client), Box::new(up));
            }
            Err(_) => {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
            }
        }
    })
}

/// Echoes every byte back.
pub fn echo_server() -> io::Result<Fixture> {
    Fixture::spawn(|mut client, _| {
        let mut buf = [0u8; 4096];
        while let Ok(n) = client.read(&mut buf) {
            if n == 0 || client.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    })
}

/// A tiny HTTP server answering every request with `body` (text/html). `bind`: where to
/// listen (all interfaces for a test that must not use loopback).
pub fn http_server(body: &'static str, bind: SocketAddr) -> io::Result<Fixture> {
    Fixture::spawn_on(bind, move |mut client, requests| {
        let _ = client.set_read_timeout(Some(Duration::from_secs(10)));
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            match client.read(&mut byte) {
                Ok(1) => buf.push(byte[0]),
                _ => return,
            }
            if buf.len() > 16 * 1024 {
                return;
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.lines().next().unwrap_or_default().to_owned();
        let host = text
            .lines()
            .find_map(|l| l.strip_prefix("Host: ").or_else(|| l.strip_prefix("host: ")))
            .unwrap_or_default()
            .to_owned();
        log_lock(requests).push(format!("{line} (Host: {host})"));
        let _ = client.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
    })
}
