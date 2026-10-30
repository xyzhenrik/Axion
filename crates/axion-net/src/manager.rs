//! [`NetManager`]: owns the named connections, starts and stops their tunnels, and turns the
//! routing rules into what the browser engine is told (a [`Plan`]).
//!
//! The kill switch lives here: a route to a connection that is not up never becomes a direct
//! route. It becomes the *sink* (a loopback SOCKS5 server that refuses everything), so the
//! requests fail with a proxy error that the browser turns into an explanatory page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crate::NetError;
use crate::connection::{Connection, ConnectionKind, ProxyScheme};
use crate::pac::{self, PacProxyKind, PacTarget};
use crate::proxy::{ProxyMonitor, ProxyTimings, ProxyTunnel, SocksUpstream};
use crate::route::{RouteQuery, RoutingConfig, Target};
use crate::secret::{Credentials, Secret};
use crate::secrets::{SecretError, SecretStore};
use crate::server::{LocalServer, ServerOptions};
use crate::tunnel::{Tunnel, TunnelStatus};

/// Knobs of the manager (from `[network]` in the settings).
#[derive(Clone, Debug)]
pub struct NetOptions {
    pub timings: ProxyTimings,
    pub server: ServerOptions,
    /// Where backends may keep state (Tor's cache, …).
    pub data_dir: std::path::PathBuf,
    /// How long a WireGuard handshake may stay unanswered (`network.wireguard_handshake_timeout_secs`).
    pub wireguard_handshake_timeout: Duration,
}

impl Default for NetOptions {
    fn default() -> Self {
        Self {
            timings: ProxyTimings::default(),
            server: ServerOptions::default(),
            data_dir: std::env::temp_dir(),
            wireguard_handshake_timeout: Duration::from_secs(20),
        }
    }
}

/// What a backend gets to build a tunnel.
#[derive(Clone, Debug)]
pub struct TunnelOptions {
    pub server: ServerOptions,
    pub connect_timeout: Duration,
    pub data_dir: std::path::PathBuf,
    pub wireguard_handshake_timeout: Duration,
}

/// How a connection is doing, for the UI and the routing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Not started (nothing needs it yet).
    Stopped,
    Connecting,
    Up {
        since: SystemTime,
        info: String,
    },
    /// Started and not working (down, unreachable, refused our login, …).
    Failed(String),
    /// Cannot start: the vault is locked, the kind is not in this build, the config is bad.
    Blocked(String),
}

impl ConnectionStatus {
    pub fn is_up(&self) -> bool {
        matches!(self, ConnectionStatus::Up { .. })
    }

    /// A one-line state for lists.
    pub fn summary(&self) -> &str {
        match self {
            ConnectionStatus::Stopped => "Not started",
            ConnectionStatus::Connecting => "Connecting…",
            ConnectionStatus::Up { .. } => "Connected",
            ConnectionStatus::Failed(r) | ConnectionStatus::Blocked(r) => r,
        }
    }
}

impl From<TunnelStatus> for ConnectionStatus {
    fn from(s: TunnelStatus) -> Self {
        match s {
            TunnelStatus::Stopped => ConnectionStatus::Stopped,
            TunnelStatus::Connecting => ConnectionStatus::Connecting,
            TunnelStatus::Up { since, info } => ConnectionStatus::Up { since, info },
            TunnelStatus::Failed(r) => ConnectionStatus::Failed(r),
        }
    }
}

/// Something changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetEvent {
    pub id: String,
    pub name: String,
    pub status: ConnectionStatus,
    /// The status before.
    pub was: ConnectionStatus,
}

/// What Chromium's `proxy` preference should be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxySetting {
    /// No proxy (Chromium's default).
    Direct,
    /// `socks5://host:port`, `http://host:port`, `https://host:port`.
    Fixed(String),
    /// A PAC script (see [`pac`]).
    Pac(String),
}

/// What one browser context (account tabs, private tabs) is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextPlan {
    pub setting: ProxySetting,
    /// Some traffic of this context is routed: turn on the leak protections.
    pub leak_protection: bool,
    /// Ids of the connections this context uses that are not up right now (their traffic is
    /// blocked, or goes direct when the kill switch is off).
    pub down: Vec<String>,
}

/// Login for a proxy Chromium asks about (`GetAuthCredentials`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyLogin {
    pub host: String,
    pub port: u16,
    pub credentials: Credentials,
}

/// Everything the engine is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub normal: ContextPlan,
    pub private: ContextPlan,
    pub logins: Vec<ProxyLogin>,
}

enum Running {
    Tunnel {
        tunnel: Arc<dyn Tunnel>,
        _front: LocalServer,
    },
    /// A proxy Chromium talks to itself; we only watch it.
    Monitor(Arc<ProxyMonitor>),
}

impl Running {
    fn status(&self) -> TunnelStatus {
        match self {
            Running::Tunnel { tunnel, .. } => tunnel.status(),
            Running::Monitor(m) => m.status(),
        }
    }

    fn stop(&self) {
        match self {
            Running::Tunnel { tunnel, .. } => tunnel.stop(),
            Running::Monitor(m) => m.stop(),
        }
    }

    fn tunnel(&self) -> Option<&Arc<dyn Tunnel>> {
        match self {
            Running::Tunnel { tunnel, .. } => Some(tunnel),
            Running::Monitor(_) => None,
        }
    }
}

/// How Chromium reaches a started connection.
#[derive(Clone)]
struct Gateway {
    target: PacTarget,
    /// Host and port the credentials are for (Chromium asks with these).
    login: Option<(String, u16, Credentials)>,
}

struct Entry {
    conn: Connection,
    status: ConnectionStatus,
    /// Bumped by every start and stop: a worker that finds it changed throws its work away.
    generation: u64,
    running: Option<Running>,
    gateway: Option<Gateway>,
}

struct Inner {
    routing: RoutingConfig,
    entries: HashMap<String, Entry>,
    events: Vec<NetEvent>,
    sink: Option<LocalServer>,
}

pub type Waker = Box<dyn Fn() + Send + Sync>;

struct Shared {
    inner: Mutex<Inner>,
    secrets: Arc<dyn SecretStore>,
    options: NetOptions,
    waker: Mutex<Option<Waker>>,
}

/// The connection manager. Cheap to clone (it is a handle).
#[derive(Clone)]
pub struct NetManager {
    shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl NetManager {
    pub fn new(secrets: Arc<dyn SecretStore>, options: NetOptions) -> Self {
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    routing: RoutingConfig::default(),
                    entries: HashMap::new(),
                    events: Vec::new(),
                    sink: None,
                }),
                secrets,
                options,
                waker: Mutex::new(None),
            }),
        }
    }

    /// Called from worker threads when a status may have changed (wake the UI loop).
    pub fn set_waker(&self, waker: Option<Waker>) {
        *lock(&self.shared.waker) = waker;
    }

    fn wake(shared: &Shared) {
        if let Some(w) = lock(&shared.waker).as_ref() {
            w();
        }
    }

    pub fn routing(&self) -> RoutingConfig {
        lock(&self.shared.inner).routing.clone()
    }

    /// Applies new settings. Connections that are gone are stopped; connections whose
    /// definition changed are stopped and, if they were running, started again.
    pub fn set_routing(&self, routing: RoutingConfig) {
        let mut restart = Vec::new();
        {
            let mut inner = lock(&self.shared.inner);
            let ids: Vec<String> = routing.connections.iter().map(|c| c.id.clone()).collect();
            let gone: Vec<String> = inner.entries.keys().filter(|k| !ids.contains(k)).cloned().collect();
            for id in gone {
                if let Some(mut e) = inner.entries.remove(&id) {
                    Self::halt(&mut e);
                }
            }
            for conn in &routing.connections {
                match inner.entries.get_mut(&conn.id) {
                    Some(e) if e.conn == *conn => {}
                    Some(e) => {
                        let active = !matches!(e.status, ConnectionStatus::Stopped);
                        Self::halt(e);
                        e.conn = conn.clone();
                        e.status = ConnectionStatus::Stopped;
                        if active {
                            restart.push(conn.id.clone());
                        }
                    }
                    None => {
                        inner.entries.insert(
                            conn.id.clone(),
                            Entry {
                                conn: conn.clone(),
                                status: ConnectionStatus::Stopped,
                                generation: 0,
                                running: None,
                                gateway: None,
                            },
                        );
                    }
                }
            }
            inner.routing = routing;
        }
        for id in restart {
            self.start(&id);
        }
    }

    fn halt(entry: &mut Entry) {
        entry.generation += 1;
        if let Some(r) = entry.running.take() {
            r.stop();
        }
        entry.gateway = None;
    }

    pub fn status(&self, id: &str) -> Option<ConnectionStatus> {
        lock(&self.shared.inner).entries.get(id).map(|e| e.status.clone())
    }

    pub fn connection(&self, id: &str) -> Option<Connection> {
        lock(&self.shared.inner).entries.get(id).map(|e| e.conn.clone())
    }

    /// Starts every connection a kind of tab can use (those named by the default and the site
    /// rules). Already started ones are left alone.
    pub fn ensure_started(&self, private: bool) {
        let ids: Vec<String> = {
            let inner = lock(&self.shared.inner);
            inner
                .routing
                .targets_for(private)
                .iter()
                .filter_map(|t| t.connection_id().map(str::to_owned))
                .collect()
        };
        for id in ids {
            let needs = matches!(self.status(&id), Some(ConnectionStatus::Stopped));
            if needs {
                self.start(&id);
            }
        }
    }

    /// Starts (or restarts) a connection on a worker thread.
    pub fn start(&self, id: &str) {
        let (conn, generation) = {
            let mut inner = lock(&self.shared.inner);
            let Some(entry) = inner.entries.get_mut(id) else { return };
            Self::halt(entry);
            let was = std::mem::replace(&mut entry.status, ConnectionStatus::Connecting);
            let (conn, generation) = (entry.conn.clone(), entry.generation);
            let name = conn.name.clone();
            inner.events.push(NetEvent {
                id: id.to_owned(),
                name,
                status: ConnectionStatus::Connecting,
                was,
            });
            (conn, generation)
        };
        let shared = Arc::clone(&self.shared);
        let id = id.to_owned();
        let spawned = std::thread::Builder::new().name("net-start".into()).spawn(move || {
            let outcome = build(&shared, &conn);
            Self::finish(&shared, &id, generation, outcome);
            Self::wake(&shared);
        });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the connection thread");
        }
    }

    fn finish(shared: &Arc<Shared>, id: &str, generation: u64, outcome: Result<(Running, Gateway), Started>) {
        let mut inner = lock(&shared.inner);
        let Some(entry) = inner.entries.get_mut(id) else {
            if let Ok((running, _)) = outcome {
                running.stop();
            }
            return;
        };
        if entry.generation != generation {
            // Stopped or restarted meanwhile: throw the work away.
            if let Ok((running, _)) = outcome {
                running.stop();
            }
            return;
        }
        let was = entry.status.clone();
        let status = match outcome {
            Ok((running, gateway)) => {
                let status: ConnectionStatus = running.status().into();
                entry.running = Some(running);
                entry.gateway = Some(gateway);
                status
            }
            Err(Started::Blocked(reason)) => ConnectionStatus::Blocked(reason),
            Err(Started::Failed(reason)) => ConnectionStatus::Failed(reason),
        };
        entry.status = status.clone();
        let name = entry.conn.name.clone();
        tracing::info!(connection = %name, up = status.is_up(), state = status.summary(), "connection started");
        inner.events.push(NetEvent { id: id.to_owned(), name, status, was });
    }

    pub fn stop(&self, id: &str) {
        let mut inner = lock(&self.shared.inner);
        let Some(entry) = inner.entries.get_mut(id) else { return };
        Self::halt(entry);
        let was = std::mem::replace(&mut entry.status, ConnectionStatus::Stopped);
        let name = entry.conn.name.clone();
        if was != ConnectionStatus::Stopped {
            inner.events.push(NetEvent { id: id.to_owned(), name, status: ConnectionStatus::Stopped, was });
        }
    }

    /// Stops everything (the browser is closing).
    pub fn shutdown(&self) {
        let mut inner = lock(&self.shared.inner);
        for entry in inner.entries.values_mut() {
            Self::halt(entry);
            entry.status = ConnectionStatus::Stopped;
        }
        if let Some(mut sink) = inner.sink.take() {
            sink.stop();
        }
    }

    /// Looks at the running tunnels and returns what changed since the last call. Cheap;
    /// call it every UI tick.
    pub fn poll(&self) -> Vec<NetEvent> {
        let mut inner = lock(&self.shared.inner);
        let mut changed = Vec::new();
        for (id, entry) in &mut inner.entries {
            let Some(running) = &entry.running else { continue };
            let now: ConnectionStatus = running.status().into();
            if let (ConnectionStatus::Up { .. }, ConnectionStatus::Up { .. }) = (&now, &entry.status) {
                // Still up: only the info line moves (a WireGuard handshake age), no event.
                entry.status = now;
            } else if now != entry.status {
                let was = std::mem::replace(&mut entry.status, now.clone());
                changed.push(NetEvent { id: id.clone(), name: entry.conn.name.clone(), status: now, was });
            }
        }
        inner.events.extend(changed);
        std::mem::take(&mut inner.events)
    }

    /// A page failed through this connection's proxy although it looked up: check it now
    /// (the result arrives with the next [`poll`](Self::poll)).
    pub fn report_failure(&self, id: &str) {
        let inner = lock(&self.shared.inner);
        if let Some(Running::Monitor(m)) = inner.entries.get(id).and_then(|e| e.running.as_ref()) {
            m.recheck();
        }
    }

    /// The running tunnel of a connection (not for proxies Chromium talks to itself).
    fn tunnel_of(&self, id: &str) -> Option<Arc<dyn Tunnel>> {
        lock(&self.shared.inner).entries.get(id)?.running.as_ref()?.tunnel().cloned()
    }

    /// How far a connection that is still starting has come (Tor's bootstrap).
    pub fn progress(&self, id: &str) -> Option<crate::TunnelProgress> {
        self.tunnel_of(id)?.progress()
    }

    /// The circuit that carries `host`'s traffic through a connection, if it has circuits.
    pub fn circuit_for(&self, id: &str, host: &str) -> Option<crate::CircuitInfo> {
        self.tunnel_of(id)?.circuit_for(host)
    }

    /// New circuits for `host`'s site (open connections of the site are closed).
    pub fn new_circuit(&self, id: &str, host: &str) {
        if let Some(t) = self.tunnel_of(id) {
            t.new_circuit(host);
        }
    }

    /// New circuits for everything the connection carries.
    pub fn new_identity(&self, id: &str) {
        if let Some(t) = self.tunnel_of(id) {
            t.new_identity();
        }
    }

    /// Which target a request uses.
    pub fn resolve(&self, query: &RouteQuery<'_>) -> Target {
        lock(&self.shared.inner).routing.resolve(query)
    }

    /// The engine settings for the current routing and connection states.
    pub fn plan(&self) -> Plan {
        let mut inner = lock(&self.shared.inner);
        let kill_switch = inner.routing.kill_switch;
        let mut logins = Vec::new();
        let normal = Self::plan_for(&mut inner, false, kill_switch, &mut logins, &self.shared.options);
        let private = Self::plan_for(&mut inner, true, kill_switch, &mut logins, &self.shared.options);
        logins.dedup_by(|a, b| a.host == b.host && a.port == b.port);
        Plan { normal, private, logins }
    }

    fn sink_target(inner: &mut Inner, options: &NetOptions) -> PacTarget {
        if inner.sink.is_none() {
            match LocalServer::refusing(options.server) {
                Ok(s) => inner.sink = Some(s),
                Err(e) => tracing::error!(error = %e, "could not start the kill switch sink"),
            }
        }
        // Without a sink the address is a closed port: also a refusal.
        let port = inner.sink.as_ref().map_or(1, |s| s.addr().port());
        PacTarget::Proxy { kind: PacProxyKind::Socks5, host_port: format!("127.0.0.1:{port}") }
    }

    fn plan_for(
        inner: &mut Inner,
        private: bool,
        kill_switch: bool,
        logins: &mut Vec<ProxyLogin>,
        options: &NetOptions,
    ) -> ContextPlan {
        let routing = inner.routing.clone();
        let mut down = Vec::new();
        let mut leak = false;
        let mut pac_target =
            |inner: &mut Inner, t: &Target, logins: &mut Vec<ProxyLogin>, down: &mut Vec<String>| match t {
                Target::Direct => PacTarget::Direct,
                Target::Connection(id) => {
                    leak = true;
                    let gateway =
                        inner.entries.get(id).filter(|e| e.status.is_up()).and_then(|e| e.gateway.clone());
                    match gateway {
                        Some(g) => {
                            if let Some((host, port, credentials)) = g.login {
                                logins.push(ProxyLogin { host, port, credentials });
                            }
                            g.target
                        }
                        None => {
                            if !down.contains(id) {
                                down.push(id.clone());
                            }
                            if kill_switch { Self::sink_target(inner, options) } else { PacTarget::Direct }
                        }
                    }
                }
            };
        let default = pac_target(inner, routing.default_for(private), logins, &mut down);
        let rules = routing.effective_rules(private);
        let setting = if rules.is_empty() {
            match &default {
                PacTarget::Direct => ProxySetting::Direct,
                PacTarget::Proxy { kind, host_port } => ProxySetting::Fixed(format!(
                    "{}://{host_port}",
                    match kind {
                        PacProxyKind::Socks5 => "socks5",
                        PacProxyKind::Http => "http",
                        PacProxyKind::Https => "https",
                    }
                )),
            }
        } else {
            let mut list = Vec::new();
            for rule in rules {
                let t = pac_target(inner, &rule.connection, logins, &mut down);
                list.push((rule.host.clone(), t));
            }
            ProxySetting::Pac(pac::generate(&default, &list))
        };
        ContextPlan { setting, leak_protection: leak, down }
    }

    /// The address of the kill switch sink (tests).
    #[doc(hidden)]
    pub fn sink_addr(&self) -> Option<std::net::SocketAddr> {
        lock(&self.shared.inner).sink.as_ref().map(LocalServer::addr)
    }
}

enum Started {
    Blocked(String),
    Failed(String),
}

fn build(shared: &Arc<Shared>, conn: &Connection) -> Result<(Running, Gateway), Started> {
    let loaded = if conn.kind.needs_secrets() {
        shared.secrets.load(&conn.id)
    } else {
        Ok(crate::connection::ConnectionSecrets::default())
    };
    let secrets = match loaded {
        Ok(s) => s,
        Err(SecretError::Locked) => {
            return Err(Started::Blocked(
                "The password vault is locked, so the secrets of this connection are not available.".into(),
            ));
        }
        Err(e) => return Err(Started::Blocked(format!("The connection's secrets cannot be read: {e}"))),
    };
    if let Err(e) = conn.kind.validate() {
        return Err(Started::Blocked(e.to_string()));
    }
    let timings = shared.options.timings;
    let tunnel_options = TunnelOptions {
        server: shared.options.server,
        connect_timeout: timings.connect_timeout,
        data_dir: shared.options.data_dir.clone(),
        wireguard_handshake_timeout: shared.options.wireguard_handshake_timeout,
    };
    let fail = |e: NetError| Started::Failed(e.to_string());
    match &conn.kind {
        ConnectionKind::Direct => Err(Started::Blocked("A direct connection needs no start.".into())),
        ConnectionKind::Proxy(p) => {
            if p.scheme == ProxyScheme::Socks5 && !p.has_auth() || p.scheme != ProxyScheme::Socks5 {
                let monitor = ProxyMonitor::new(p.clone(), secrets.password.clone(), timings);
                monitor.start().map_err(fail)?;
                let kind = match p.scheme {
                    ProxyScheme::Socks5 => PacProxyKind::Socks5,
                    ProxyScheme::Http => PacProxyKind::Http,
                    ProxyScheme::Https => PacProxyKind::Https,
                };
                let login = (p.scheme != ProxyScheme::Socks5 && p.has_auth()).then(|| {
                    (p.host.clone(), p.port, Credentials::new(p.username.clone(), secrets.password.expose()))
                });
                let gateway = Gateway { target: PacTarget::Proxy { kind, host_port: p.host_port() }, login };
                Ok((Running::Monitor(monitor), gateway))
            } else {
                let tunnel: Arc<dyn Tunnel> = Arc::new(ProxyTunnel::new(
                    p.clone(),
                    secrets.password.clone(),
                    timings,
                    shared.options.server,
                ));
                front(tunnel, shared).map_err(fail)
            }
        }
        ConnectionKind::WireGuard(w) => {
            let config = match &secrets.wireguard {
                Some(s) => w.clone().with_secrets(s),
                None => w.clone(),
            };
            if config.interface.private_key.is_empty() {
                return Err(Started::Blocked("The WireGuard private key is missing from the vault.".into()));
            }
            let tunnel = crate::wireguard::create(&config, &tunnel_options).map_err(|e| match e {
                NetError::NotAvailable(_) => Started::Blocked(e.to_string()),
                other => fail(other),
            })?;
            front(tunnel, shared).map_err(fail)
        }
        ConnectionKind::Tor(t) => {
            let tunnel = crate::tor::create(t, &tunnel_options).map_err(|e| match e {
                NetError::NotAvailable(_) => Started::Blocked(e.to_string()),
                other => fail(other),
            })?;
            front(tunnel, shared).map_err(fail)
        }
    }
}

/// Starts a tunnel and puts the authenticating HTTP front in front of it.
fn front(tunnel: Arc<dyn Tunnel>, shared: &Arc<Shared>) -> Result<(Running, Gateway), NetError> {
    // A tunnel that finishes starting later (Tor) wakes the UI through this.
    let weak = Arc::downgrade(shared);
    tunnel.set_notifier(Arc::new(move || {
        if let Some(shared) = weak.upgrade() {
            NetManager::wake(&shared);
        }
    }));
    let endpoint = tunnel.start()?;
    let dialer = Arc::new(SocksUpstream::local(&endpoint, shared.options.timings.connect_timeout));
    let session = Credentials::random_session();
    let server = match LocalServer::http_front(session.clone(), dialer, shared.options.server) {
        Ok(s) => s,
        Err(e) => {
            tunnel.stop();
            return Err(e.into());
        }
    };
    let addr = server.addr();
    let gateway = Gateway {
        target: PacTarget::Proxy { kind: PacProxyKind::Http, host_port: addr.to_string() },
        login: Some((addr.ip().to_string(), addr.port(), session)),
    };
    Ok((Running::Tunnel { tunnel, _front: server }, gateway))
}

/// A password for tests and tools that have none.
#[doc(hidden)]
pub fn no_password() -> Secret {
    Secret::default()
}
