//! WireGuard backend: a fully userspace client (no root, no TUN device, no kernel module, no
//! external program), the way `wireproxy` works.
//!
//! [`boringtun`] speaks the protocol over one UDP socket to the peers' endpoints; a userspace
//! TCP/IP stack ([`smoltcp`], see [`crate::wg_net`]) runs *inside* the tunnel. The tunnel
//! exposes a SOCKS5 server on `127.0.0.1` ([`LocalServer::socks5`]) whose dialer opens TCP
//! streams in that stack. Consequences, all deliberate:
//!
//! - Names are resolved through the config's `DNS =` servers, over the tunnel. The local
//!   resolver is used for exactly one thing: the peers' endpoint host name (at start, and
//!   again when a handshake stays unanswered).
//! - `AllowedIPs` decide what may enter the tunnel. A destination outside every peer's
//!   AllowedIPs is refused, never sent to the local network.
//! - Only TCP is carried (SOCKS5 CONNECT): no UDP, so no QUIC / HTTP/3 through the tunnel.
//! - `PreUp`/`PostUp`/... scripts and routing-table options of a `.conf` are never run
//!   (the parser reports them as ignored).
//!
//! Keys live in [`zeroize::Zeroizing`] buffers and are never logged.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use zeroize::Zeroizing;

use crate::connection::WgConfig;
use crate::secret::Credentials;
use crate::server::{LocalServer, ServerOptions};
use crate::tunnel::{LocalEndpoint, Tunnel, TunnelStatus};
use crate::wg_net::{Cidr, PeerSpec, Phase, Stack, StackSpec};
pub use crate::wg_net::{WgOptions, WgStats};
use crate::{NetError, TunnelOptions};

/// Whether this build can run WireGuard connections.
pub const AVAILABLE: bool = true;

/// The tunnel MTU when the config has none (what `wg-quick` uses).
pub const DEFAULT_MTU: u16 = 1420;

/// Builds the tunnel for a config (validates it; nothing runs until `start`).
pub fn create(config: &WgConfig, options: &TunnelOptions) -> Result<Arc<dyn Tunnel>, NetError> {
    Ok(Arc::new(WireGuardTunnel::new(config, options)?))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key_bytes(what: &str, text: &str) -> Result<Zeroizing<[u8; 32]>, NetError> {
    let mut raw = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| NetError::Invalid(format!("{what}: not valid base64")))?,
    );
    let key: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| NetError::Invalid(format!("{what}: a key is 32 bytes, this one has {}", raw.len())))?;
    raw.iter_mut().for_each(|b| *b = 0);
    Ok(Zeroizing::new(key))
}

/// The config in the form the stack runs.
fn spec_of(config: &WgConfig) -> Result<StackSpec, NetError> {
    crate::wgconf::validate(config)?;
    let iface = &config.interface;
    if iface.private_key.is_empty() {
        return Err(NetError::Invalid("the WireGuard private key is missing".into()));
    }
    let private = key_bytes("PrivateKey", iface.private_key.expose())?;
    let mut addresses = Vec::new();
    for a in &iface.addresses {
        addresses.push(
            Cidr::parse(a).ok_or_else(|| NetError::Invalid(format!("Address: {a:?} is not an address")))?,
        );
    }
    let mut dns = Vec::new();
    for d in &iface.dns {
        dns.push(
            d.parse::<IpAddr>().map_err(|_| NetError::Invalid(format!("DNS: {d:?} is not an address")))?,
        );
    }
    let mut peers = Vec::new();
    for p in &config.peers {
        let public = key_bytes("PublicKey", &p.public_key)?;
        let preshared = if p.preshared_key.is_empty() {
            None
        } else {
            Some(key_bytes("PresharedKey", p.preshared_key.expose())?)
        };
        let mut allowed = Vec::new();
        for a in &p.allowed_ips {
            allowed.push(
                Cidr::parse(a)
                    .ok_or_else(|| NetError::Invalid(format!("AllowedIPs: {a:?} is not a network")))?,
            );
        }
        peers.push(PeerSpec {
            public: *public,
            preshared,
            allowed,
            endpoint_text: p.endpoint.clone(),
            endpoint_addrs: Vec::new(),
            keepalive: p.persistent_keepalive,
        });
    }
    let mtu = if iface.mtu == 0 { DEFAULT_MTU } else { iface.mtu };
    Ok(StackSpec { private, addresses, dns, mtu: usize::from(mtu), listen_port: iface.listen_port, peers })
}

struct Live {
    stack: Arc<Stack>,
    server: Option<LocalServer>,
    endpoint: Option<LocalEndpoint>,
}

/// A WireGuard connection (see the module docs).
pub struct WireGuardTunnel {
    spec: StackSpec,
    options: WgOptions,
    server_options: ServerOptions,
    gate: Mutex<()>,
    live: Mutex<Option<Live>>,
    idle: Mutex<TunnelStatus>,
}

impl WireGuardTunnel {
    pub fn new(config: &WgConfig, options: &TunnelOptions) -> Result<Self, NetError> {
        let spec = spec_of(config)?;
        let wg = WgOptions {
            handshake_timeout: options.wireguard_handshake_timeout,
            connect_timeout: options.connect_timeout.max(Duration::from_secs(5)),
            max_streams: options.server.max_connections + 8,
            ..WgOptions::default()
        };
        Ok(Self::with_options(spec, wg, options.server))
    }

    fn with_options(spec: StackSpec, options: WgOptions, server_options: ServerOptions) -> Self {
        Self {
            spec,
            options,
            server_options,
            gate: Mutex::new(()),
            live: Mutex::new(None),
            idle: Mutex::new(TunnelStatus::Stopped),
        }
    }

    /// Tuning for tests and tools (the browser uses [`WireGuardTunnel::new`]).
    #[doc(hidden)]
    pub fn new_with(
        config: &WgConfig,
        options: WgOptions,
        server_options: ServerOptions,
    ) -> Result<Self, NetError> {
        Ok(Self::with_options(spec_of(config)?, options, server_options))
    }

    /// Live counters while running.
    pub fn stats(&self) -> Option<WgStats> {
        let stack = lock(&self.live).as_ref().map(|l| l.stack.clone())?;
        Some(stack.shared().stats())
    }

    #[doc(hidden)]
    pub fn debug_dump(&self) -> String {
        lock(&self.live).as_ref().map(|l| l.stack.debug_dump()).unwrap_or_default()
    }

    /// Starts a new handshake with every peer now (a rekey). Mostly for tests.
    #[doc(hidden)]
    pub fn rekey_now(&self) {
        if let Some(l) = lock(&self.live).as_ref() {
            l.stack.shared().rekey_now();
        }
    }

    fn fail(&self, reason: String) -> NetError {
        *lock(&self.idle) = TunnelStatus::Failed(reason.clone());
        NetError::Connect(reason)
    }

    /// Resolves the endpoint names (the one lookup made outside the tunnel).
    fn resolved_spec(&self) -> Result<StackSpec, NetError> {
        let mut spec = StackSpec {
            private: self.spec.private.clone(),
            addresses: self.spec.addresses.clone(),
            dns: self.spec.dns.clone(),
            mtu: self.spec.mtu,
            listen_port: self.spec.listen_port,
            peers: Vec::new(),
        };
        for p in &self.spec.peers {
            let mut peer = PeerSpec {
                public: p.public,
                preshared: p.preshared.clone(),
                allowed: p.allowed.clone(),
                endpoint_text: p.endpoint_text.clone(),
                endpoint_addrs: Vec::new(),
                keepalive: p.keepalive,
            };
            if let Some((host, port)) = crate::wgconf::split_endpoint(&p.endpoint_text) {
                let addrs: Vec<SocketAddr> = (host, port)
                    .to_socket_addrs()
                    .map_err(|e| NetError::Connect(format!("Cannot resolve the endpoint {host}: {e}")))?
                    .collect();
                if addrs.is_empty() {
                    return Err(NetError::Connect(format!("The endpoint {host} has no address")));
                }
                peer.endpoint_addrs = addrs;
            }
            spec.peers.push(peer);
        }
        Ok(spec)
    }
}

fn age_text(age: Duration) -> String {
    let s = age.as_secs();
    if s < 90 {
        format!("{s} s")
    } else if s < 5400 {
        format!("{} min", s / 60)
    } else {
        format!("{} h", s / 3600)
    }
}

impl Tunnel for WireGuardTunnel {
    fn start(&self) -> Result<LocalEndpoint, NetError> {
        let _gate = lock(&self.gate);
        if let Some(endpoint) = lock(&self.live).as_ref().and_then(|l| l.endpoint.clone()) {
            return Ok(endpoint);
        }
        *lock(&self.idle) = TunnelStatus::Connecting;
        let spec = self.resolved_spec().map_err(|e| self.fail(e.to_string()))?;
        let stack = Arc::new(
            Stack::start(&spec, self.options.clone())
                .map_err(|e| self.fail(format!("Could not start the tunnel: {e}")))?,
        );
        *lock(&self.live) = Some(Live { stack: stack.clone(), server: None, endpoint: None });
        let slack = Duration::from_secs(3);
        match stack.shared().wait_phase(self.options.handshake_timeout + slack) {
            Phase::Up { .. } => {}
            Phase::Failed(reason) => {
                self.stop();
                return Err(self.fail(reason));
            }
            Phase::Stopped => return Err(self.fail("The connection was stopped.".into())),
            Phase::Connecting => {
                self.stop();
                return Err(self.fail("The server did not answer the handshake.".into()));
            }
        }
        let credentials = Credentials::random_session();
        let server = LocalServer::socks5(credentials.clone(), stack.dialer(), self.server_options)
            .map_err(|e| self.fail(format!("Could not open the local relay: {e}")))?;
        let endpoint = LocalEndpoint { addr: server.addr(), credentials };
        let mut live = lock(&self.live);
        match live.as_mut() {
            Some(l) => {
                l.server = Some(server);
                l.endpoint = Some(endpoint.clone());
                Ok(endpoint)
            }
            None => Err(NetError::Connect("The connection was stopped.".into())),
        }
    }

    fn status(&self) -> TunnelStatus {
        let stack = lock(&self.live).as_ref().map(|l| l.stack.clone());
        let Some(stack) = stack else { return lock(&self.idle).clone() };
        let shared = stack.shared();
        match shared.phase() {
            Phase::Connecting => TunnelStatus::Connecting,
            Phase::Failed(reason) => TunnelStatus::Failed(reason),
            Phase::Stopped => TunnelStatus::Stopped,
            Phase::Up { since } => {
                let info = match lock(&shared.last_handshake).clone() {
                    Some((at, label)) => {
                        format!(
                            "Peer {label}, last handshake {} ago",
                            age_text(Instant::now().saturating_duration_since(at))
                        )
                    }
                    None => "Connected".to_owned(),
                };
                TunnelStatus::Up { since: since.min(SystemTime::now()), info }
            }
        }
    }

    fn stop(&self) {
        let live = lock(&self.live).take();
        if let Some(live) = live {
            // The relay first (no new streams), then the network under it.
            drop(live.server);
            live.stack.stop();
        }
        *lock(&self.idle) = TunnelStatus::Stopped;
    }
}

impl Drop for WireGuardTunnel {
    fn drop(&mut self) {
        self.stop();
    }
}
