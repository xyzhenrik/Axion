//! The interface every backend implements (proxy relay, WireGuard, Tor).

use std::net::SocketAddr;
use std::time::SystemTime;

use crate::secret::Credentials;

/// Where a started tunnel can be reached: a SOCKS5 server on `127.0.0.1` (random port) that
/// demands this session's login. Build it with [`crate::LocalServer::socks5`], dialing
/// through the tunnel's own streams.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEndpoint {
    pub addr: SocketAddr,
    pub credentials: Credentials,
}

/// How a tunnel is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TunnelStatus {
    Stopped,
    /// Starting: handshaking, bootstrapping the circuit, …
    Connecting,
    Up {
        since: SystemTime,
        /// A short line for the UI ("Exit: de", "Peer vpn.example.net:51820", …). No secrets.
        info: String,
    },
    /// Not working (was up and broke, or could not start). A short reason for the UI.
    Failed(String),
}

impl TunnelStatus {
    pub fn is_up(&self) -> bool {
        matches!(self, TunnelStatus::Up { .. })
    }
}

/// A tunnel to somewhere. It runs on its own threads; none of these methods may need an async
/// runtime from the caller. All methods take `&self` (interior mutability), and `status`
/// must never block (it is polled from the UI thread).
pub trait Tunnel: Send + Sync {
    /// Starts the tunnel and blocks until it is usable or has failed (the manager calls this
    /// on a worker thread; a Tor bootstrap may take a while). Starting a started tunnel
    /// returns its endpoint again.
    fn start(&self) -> Result<LocalEndpoint, crate::NetError>;

    fn status(&self) -> TunnelStatus;

    /// Stops the tunnel and closes its endpoint. Safe to call in any state, and more than once.
    fn stop(&self);
}
