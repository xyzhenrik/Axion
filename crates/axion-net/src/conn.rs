//! Byte streams that tunnels hand out, and the loop that joins two of them.
//!
//! A [`Dialer`] opens a stream to a target *by name* (the far end resolves it: no DNS on this
//! computer). TCP sockets are streams, and so are the streams of a userspace WireGuard
//! stack or a Tor circuit once their backends wrap them in [`Conn`].

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};

/// What to connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    /// A name the far end resolves. Never resolved locally.
    Domain(String, u16),
    Ip(SocketAddr),
}

impl Address {
    pub fn port(&self) -> u16 {
        match self {
            Address::Domain(_, p) => *p,
            Address::Ip(a) => a.port(),
        }
    }

    /// Parses `host:port` / `[v6]:port` (an IP literal becomes [`Address::Ip`]).
    pub fn parse_host_port(s: &str) -> Option<Address> {
        let (host, port) = crate::wgconf::split_endpoint(s)?;
        Some(match host.parse::<std::net::IpAddr>() {
            Ok(ip) => Address::Ip(SocketAddr::new(ip, port)),
            Err(_) => Address::Domain(host.to_owned(), port),
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::Domain(h, p) => write!(f, "{h}:{p}"),
            Address::Ip(a) => write!(f, "{a}"),
        }
    }
}

/// A duplex byte stream that can be split into two halves for the two copy directions.
pub trait Conn: Read + Write + Send {
    /// A second handle on the same stream (the other direction's).
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>>;
    /// No more data will be written; the peer sees end-of-file.
    fn shutdown_write(&self) -> io::Result<()>;
}

impl Conn for TcpStream {
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// Opens streams through a tunnel.
pub trait Dialer: Send + Sync {
    fn dial(&self, target: &Address) -> io::Result<Box<dyn Conn>>;
}

/// Copies both ways until both sides are done. Blocks.
pub fn pipe(a: Box<dyn Conn>, b: Box<dyn Conn>) {
    let (Ok(mut a_read), Ok(mut b_read)) = (a.try_clone_conn(), b.try_clone_conn()) else { return };
    let (mut a_write, mut b_write) = (a, b);
    let up = std::thread::Builder::new().name("net-pipe".into()).spawn(move || {
        let _ = copy(&mut a_read, &mut b_write);
        let _ = b_write.shutdown_write();
    });
    let _ = copy(&mut b_read, &mut a_write);
    let _ = a_write.shutdown_write();
    if let Ok(handle) = up {
        let _ = handle.join();
    }
}

fn copy(from: &mut dyn Read, to: &mut dyn Write) -> io::Result<u64> {
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..n])?;
        total += n as u64;
    }
}
