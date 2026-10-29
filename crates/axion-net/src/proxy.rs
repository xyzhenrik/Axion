//! The upstream-proxy backend.
//!
//! - HTTP/HTTPS proxies, and SOCKS5 proxies without a login, are given to Chromium as they
//!   are (it speaks them itself, and sends names to the proxy to resolve). [`ProxyMonitor`]
//!   only watches that the proxy keeps answering, for the kill switch.
//! - A SOCKS5 proxy **with** a login is reached through [`ProxyTunnel`]: Chromium cannot log in
//!   to SOCKS5, so a loopback relay does it ([`SocksUpstream`]).

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crate::NetError;
use crate::conn::{Address, Conn, Dialer};
use crate::connection::{ProxyConfig, ProxyScheme};
use crate::secret::{Credentials, Secret};
use crate::server::{LocalServer, ServerOptions};
use crate::socks5;
use crate::tunnel::{LocalEndpoint, Tunnel, TunnelStatus};

/// Timing of connecting and health checks.
#[derive(Clone, Copy, Debug)]
pub struct ProxyTimings {
    /// Connecting to the proxy.
    pub connect_timeout: Duration,
    /// How often the proxy is checked while up.
    pub health_interval: Duration,
}

impl Default for ProxyTimings {
    fn default() -> Self {
        Self { connect_timeout: Duration::from_secs(10), health_interval: Duration::from_secs(10) }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Opens a TCP connection to the proxy (the proxy's own name is the one name resolved here).
fn connect_proxy(proxy: &ProxyConfig, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "the proxy's name has no address");
    for addr in (proxy.host.trim_matches(['[', ']']), proxy.port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Dials through an upstream SOCKS5 proxy, targets sent as names.
pub struct SocksUpstream {
    proxy: ProxyConfig,
    credentials: Option<Credentials>,
    timeout: Duration,
}

impl SocksUpstream {
    pub fn new(proxy: ProxyConfig, password: &Secret, timeout: Duration) -> Self {
        let credentials =
            proxy.has_auth().then(|| Credentials::new(proxy.username.clone(), password.expose()));
        Self { proxy, credentials, timeout }
    }
}

impl SocksUpstream {
    /// Dials through a tunnel's loopback endpoint.
    pub fn local(endpoint: &LocalEndpoint, timeout: Duration) -> Self {
        let proxy = ProxyConfig {
            scheme: ProxyScheme::Socks5,
            host: endpoint.addr.ip().to_string(),
            port: endpoint.addr.port(),
            username: endpoint.credentials.username.clone(),
        };
        Self { proxy, credentials: Some(endpoint.credentials.clone()), timeout }
    }
}

impl Dialer for SocksUpstream {
    fn dial(&self, target: &Address) -> io::Result<Box<dyn Conn>> {
        let mut stream = connect_proxy(&self.proxy, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        socks5::client_connect(&mut stream, target, self.credentials.as_ref()).map_err(|e| match e {
            socks5::SocksError::Io(io) => io,
            other => io::Error::other(other.to_string()),
        })?;
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
        Ok(Box::new(stream))
    }
}

/// One health check of a proxy: reachable, and for SOCKS5 really a SOCKS5 server that accepts
/// our login. Nothing is sent to the internet.
pub fn probe(proxy: &ProxyConfig, password: &Secret, timeout: Duration) -> Result<(), String> {
    let mut stream = connect_proxy(proxy, timeout)
        .map_err(|e| format!("cannot reach {}: {}", proxy.host_port(), short(&e)))?;
    if proxy.scheme == ProxyScheme::Socks5 {
        stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let creds = proxy.has_auth().then(|| Credentials::new(proxy.username.clone(), password.expose()));
        socks5::client_negotiate(&mut stream, creds.as_ref()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn short(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => "connection refused".into(),
        io::ErrorKind::TimedOut => "timed out".into(),
        _ => e.to_string(),
    }
}

struct MonitorState {
    status: TunnelStatus,
}

/// Watches an upstream proxy. Down after two failed checks in a row, up again after one
/// good check.
pub struct ProxyMonitor {
    proxy: ProxyConfig,
    password: Secret,
    timings: ProxyTimings,
    state: Mutex<MonitorState>,
    stop: AtomicBool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl ProxyMonitor {
    pub fn new(proxy: ProxyConfig, password: Secret, timings: ProxyTimings) -> Arc<Self> {
        Arc::new(Self {
            proxy,
            password,
            timings,
            state: Mutex::new(MonitorState { status: TunnelStatus::Stopped }),
            stop: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    /// Checks the proxy once (blocking), then keeps checking in the background.
    pub fn start(self: &Arc<Self>) -> Result<(), NetError> {
        self.stop.store(false, Ordering::Release);
        lock(&self.state).status = TunnelStatus::Connecting;
        match probe(&self.proxy, &self.password, self.timings.connect_timeout) {
            Ok(()) => lock(&self.state).status = self.up(),
            Err(reason) => {
                lock(&self.state).status = TunnelStatus::Failed(reason.clone());
                return Err(NetError::Connect(reason));
            }
        }
        let me = Arc::clone(self);
        let handle = std::thread::Builder::new().name("net-health".into()).spawn(move || me.watch())?;
        *lock(&self.thread) = Some(handle);
        Ok(())
    }

    /// Checks the proxy once now, in the background (a page just failed through it). One
    /// failed check is enough to call it down; the regular checks bring it back.
    pub fn recheck(self: &Arc<Self>) {
        let me = Arc::clone(self);
        let _ = std::thread::Builder::new().name("net-recheck".into()).spawn(move || {
            if let Err(reason) = probe(&me.proxy, &me.password, me.timings.connect_timeout) {
                let mut state = lock(&me.state);
                if state.status.is_up() {
                    tracing::warn!(%reason, "proxy failed a page and a check");
                    state.status = TunnelStatus::Failed(reason);
                }
            }
        });
    }

    fn up(&self) -> TunnelStatus {
        TunnelStatus::Up {
            since: SystemTime::now(),
            info: format!("{} {}", self.proxy.scheme.label(), self.proxy.host_port()),
        }
    }

    fn watch(&self) {
        let mut failures = 0u32;
        loop {
            // Sleep in small steps so `stop` is quick.
            let mut waited = Duration::ZERO;
            while waited < self.timings.health_interval {
                if self.stop.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
                waited += Duration::from_millis(100);
            }
            match probe(&self.proxy, &self.password, self.timings.connect_timeout) {
                Ok(()) => {
                    failures = 0;
                    let mut state = lock(&self.state);
                    if !state.status.is_up() {
                        tracing::info!("proxy answers again");
                        state.status = self.up();
                    }
                }
                Err(reason) => {
                    failures += 1;
                    if failures >= 2 {
                        let mut state = lock(&self.state);
                        if state.status.is_up() {
                            tracing::warn!(%reason, "proxy stopped answering");
                        }
                        state.status = TunnelStatus::Failed(reason);
                    }
                }
            }
        }
    }

    pub fn status(&self) -> TunnelStatus {
        lock(&self.state).status.clone()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
        lock(&self.state).status = TunnelStatus::Stopped;
    }
}

/// A SOCKS5 proxy with a login, relayed on loopback.
pub struct ProxyTunnel {
    monitor: Arc<ProxyMonitor>,
    dialer: Arc<SocksUpstream>,
    options: ServerOptions,
    server: Mutex<Option<(LocalServer, LocalEndpoint)>>,
}

impl ProxyTunnel {
    pub fn new(proxy: ProxyConfig, password: Secret, timings: ProxyTimings, options: ServerOptions) -> Self {
        let dialer = Arc::new(SocksUpstream::new(proxy.clone(), &password, timings.connect_timeout));
        Self {
            monitor: ProxyMonitor::new(proxy, password, timings),
            dialer,
            options,
            server: Mutex::new(None),
        }
    }

    /// The address of the relay while started.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        lock(&self.server).as_ref().map(|(_, e)| e.addr)
    }
}

impl Tunnel for ProxyTunnel {
    fn start(&self) -> Result<LocalEndpoint, NetError> {
        if let Some((_, endpoint)) = lock(&self.server).as_ref() {
            return Ok(endpoint.clone());
        }
        self.monitor.start()?;
        let credentials = Credentials::random_session();
        let server = LocalServer::socks5(credentials.clone(), self.dialer.clone(), self.options)?;
        let endpoint = LocalEndpoint { addr: server.addr(), credentials };
        *lock(&self.server) = Some((server, endpoint.clone()));
        Ok(endpoint)
    }

    fn status(&self) -> TunnelStatus {
        self.monitor.status()
    }

    fn stop(&self) {
        self.monitor.stop();
        if let Some((mut server, _)) = lock(&self.server).take() {
            server.stop();
        }
    }
}

impl Drop for ProxyTunnel {
    fn drop(&mut self) {
        self.stop();
    }
}
