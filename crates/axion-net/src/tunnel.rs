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

    /// Gives the tunnel a function to call (from any thread) when its status or progress
    /// changed on its own, so the UI can look at it. Called before [`start`](Self::start).
    fn set_notifier(&self, _notify: std::sync::Arc<dyn Fn() + Send + Sync>) {}

    /// How far a slow start is (Tor's bootstrap); `None` when the backend has nothing to say.
    fn progress(&self) -> Option<TunnelProgress> {
        None
    }

    /// The circuit that carries the most recent traffic to `host`'s site, if the backend has
    /// circuits and one is known. Never invented: unknown is `None`.
    fn circuit_for(&self, _host: &str) -> Option<crate::CircuitInfo> {
        None
    }

    /// Retires the circuits of `host`'s site: the next connections build new ones. The
    /// connections that are open now are closed (the browser reloads the page).
    fn new_circuit(&self, _host: &str) {}

    /// Like [`new_circuit`](Self::new_circuit) for every site.
    fn new_identity(&self) {}
}

/// Where a slow start is, for the UI ("Bootstrapping 40%: loading relays").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelProgress {
    pub percent: u8,
    pub stage: ProgressStage,
    /// Why it is not moving (a blocked network, a skewed clock), as the backend says it.
    pub stuck: Option<String>,
}

/// What a slow start is doing; the app translates these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressStage {
    Connecting,
    Consensus,
    Relays,
    Ready,
}

impl ProgressStage {
    /// The stage a bootstrap percentage is in.
    pub fn for_percent(percent: u8) -> Self {
        match percent {
            0..=14 => ProgressStage::Connecting,
            15..=39 => ProgressStage::Consensus,
            40..=99 => ProgressStage::Relays,
            _ => ProgressStage::Ready,
        }
    }
}
