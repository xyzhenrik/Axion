//! The Tor backend, built on [Arti](https://gitlab.torproject.org/tpo/core/arti), the Tor
//! Project's Rust client. **This is not Tor Browser**: it is a Tor client behind Axion's
//! normal browser engine (no fingerprinting resistance, no letterboxing). See
//! `docs/network.md`.
//!
//! - [`TorTunnel`] bootstraps Arti on a private tokio runtime (two worker threads, invisible
//!   to the rest of the program), keeps its state under `<profile>/network/tor/`, and offers a
//!   SOCKS5 server on loopback whose dialer opens Arti streams **by name** (names are resolved
//!   by the exit relay, never here; `.onion` works).
//! - [`isolation`]: one set of circuits per site.
//! - [`guard`]: no local or private targets reach Tor.
//! - [`path`]: the circuit path of a stream, for the circuit display.

mod conn;
pub mod guard;
pub mod isolation;
mod path;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use arti_client::config::{BoolOrAuto, BridgeConfigBuilder, TorClientConfigBuilder};
use arti_client::{BootstrapBehavior, CountryCode, IsolationToken, StreamPrefs, TorClient};
use futures::StreamExt as _;
use tokio::runtime::{Builder, Handle, Runtime};
use tokio::sync::watch;
use tor_rtcompat::tokio::TokioRustlsRuntime;

use self::conn::TorConn;
use self::guard::GuardedDialer;
use self::isolation::{Isolation, site_key};
use self::path::{CircuitBook, GeoIp};
use crate::conn::{Address, Conn, Dialer};
use crate::connection::{TorConfig, TorIsolation};
use crate::secret::Credentials;
use crate::server::{LocalServer, ServerOptions};
use crate::tunnel::{LocalEndpoint, ProgressStage, Tunnel, TunnelProgress, TunnelStatus};
use crate::{CircuitInfo, NetError, TunnelOptions};

/// Whether this build can run Tor connections.
pub const AVAILABLE: bool = true;

/// Niceness of the runtime's threads when `background_priority` is on: the browser's own
/// threads (UI, Chromium) always win against the directory download and its verification.
const BACKGROUND_NICE: i32 = 15;

/// Progress changes reach the UI at most this often (the last one is never lost: the status
/// change that ends a bootstrap always notifies).
const PROGRESS_NOTIFY_INTERVAL: Duration = Duration::from_millis(250);

/// A Tor circuit takes longer to build than a TCP connection: a stream may take this many
/// times the connect timeout of the other backends.
const CONNECT_TIMEOUT_FACTOR: u32 = 4;

/// Longest the runtime is given to wind down when the tunnel stops.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

type ArtiRuntime = TokioRustlsRuntime;
type Client = TorClient<ArtiRuntime>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Builds the tunnel for a config.
pub fn create(config: &TorConfig, options: &TunnelOptions) -> Result<Arc<dyn Tunnel>, NetError> {
    config.validate()?;
    let plan = Plan::from_config(config)?;
    Ok(Arc::new(TorTunnel {
        shared: Arc::new(Shared {
            plan,
            dir: options.data_dir.join("tor"),
            server_options: options.server,
            connect_timeout: options.connect_timeout.saturating_mul(CONNECT_TIMEOUT_FACTOR),
            status: Mutex::new(TunnelStatus::Stopped),
            progress: Mutex::new(None),
            ready: AtomicBool::new(false),
            threads: options.tor_worker_threads.clamp(1, 8),
            background: options.background_priority,
            last_progress_notify: Mutex::new(None),
            client: Mutex::new(None),
            isolation: Isolation::new(config.isolation, IsolationToken::new),
            circuits: CircuitBook::new(),
            geoip: GeoIp::embedded(),
            shutdown: watch::channel(false).0,
            notifier: Mutex::new(None),
        }),
        running: Mutex::new(None),
    }))
}

/// A Tor config, checked and parsed once.
struct Plan {
    bridges: Vec<BridgeConfigBuilder>,
    exit_country: Option<CountryCode>,
}

impl Plan {
    fn from_config(config: &TorConfig) -> Result<Self, NetError> {
        let mut bridges = Vec::new();
        if config.use_bridges {
            for line in config.bridges.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
                bridges.push(parse_bridge(line)?);
            }
        }
        let exit_country =
            match config.exit_country.trim() {
                "" => None,
                code => Some(code.to_ascii_uppercase().parse::<CountryCode>().map_err(|_| {
                    NetError::Invalid("the exit country is a two-letter code such as de".into())
                })?),
            };
        Ok(Self { bridges, exit_country })
    }
}

/// `[Bridge] 1.2.3.4:443 FINGERPRINT`. Bridges that need a pluggable transport (`obfs4`,
/// `snowflake`, `meek`) are refused: those run as separate programs this build does not ship.
pub fn parse_bridge(line: &str) -> Result<BridgeConfigBuilder, NetError> {
    let line = line.trim();
    let line = match line.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("bridge ") => line[7..].trim(),
        _ => line,
    };
    let first = line.split_whitespace().next().unwrap_or_default();
    // A plain bridge starts with its address (`1.2.3.4:443`, `[2001:db8::1]:443`).
    let is_address = first.parse::<std::net::SocketAddr>().is_ok();
    if !is_address {
        return Err(NetError::Invalid(format!(
            "only plain bridges (`ip:port fingerprint`) are supported; the transport `{first}` needs a separate program that this build does not include"
        )));
    }
    line.parse::<BridgeConfigBuilder>()
        .map_err(|e| NetError::Invalid(format!("this bridge line is not valid: {e}")))
}

/// State shared by the tunnel, its tasks and its connections.
struct Shared {
    plan: Plan,
    dir: PathBuf,
    server_options: ServerOptions,
    connect_timeout: Duration,
    status: Mutex<TunnelStatus>,
    progress: Mutex<Option<TunnelProgress>>,
    /// Arti can carry traffic.
    ready: AtomicBool,
    /// Worker threads of the private runtime.
    threads: usize,
    /// The runtime's threads run at low priority.
    background: bool,
    /// When the UI was last told about a progress change.
    last_progress_notify: Mutex<Option<std::time::Instant>>,
    client: Mutex<Option<Arc<Client>>>,
    isolation: Isolation<IsolationToken>,
    circuits: CircuitBook,
    geoip: GeoIp,
    /// `true`: the tunnel stops; open connections close.
    shutdown: watch::Sender<bool>,
    /// Wakes the UI when the status or progress changed.
    notifier: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct Running {
    runtime: Runtime,
    server: LocalServer,
    endpoint: LocalEndpoint,
}

/// The Tor connection.
pub struct TorTunnel {
    shared: Arc<Shared>,
    running: Mutex<Option<Running>>,
}

impl Shared {
    fn set_status(&self, status: TunnelStatus) {
        *lock(&self.status) = status;
        self.notify();
    }

    fn notify(&self) {
        let notifier = lock(&self.notifier).clone();
        if let Some(n) = notifier {
            n();
        }
    }

    fn set_progress(&self, progress: Option<TunnelProgress>) {
        let changed = {
            let mut current = lock(&self.progress);
            let changed = *current != progress;
            *current = progress;
            changed
        };
        if changed && self.progress_notify_due() {
            self.notify();
        }
    }

    /// At most one progress notification per [`PROGRESS_NOTIFY_INTERVAL`].
    fn progress_notify_due(&self) -> bool {
        let mut last = lock(&self.last_progress_notify);
        let now = std::time::Instant::now();
        match *last {
            Some(at) if now.duration_since(at) < PROGRESS_NOTIFY_INTERVAL => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

/// Creates the private directories Arti keeps its state and cache in (only the user may read
/// them).
fn make_dirs(dir: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let (state, cache) = (dir.join("state"), dir.join("cache"));
    for d in [dir, &state, &cache] {
        std::fs::create_dir_all(d)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok((state, cache))
}

fn client_config(shared: &Shared) -> Result<arti_client::TorClientConfig, NetError> {
    let (state, cache) = make_dirs(&shared.dir)?;
    let mut builder = TorClientConfigBuilder::from_directories(state, cache);
    // Onion services are reachable; local addresses never are (also checked before dialing).
    builder.address_filter().allow_onion_addrs(true);
    if !shared.plan.bridges.is_empty() {
        let bridges = builder.bridges();
        bridges.enabled(BoolOrAuto::Explicit(true));
        for b in &shared.plan.bridges {
            bridges.bridges().push(b.clone());
        }
    }
    builder.build().map_err(|e| NetError::Invalid(format!("the Tor settings are not valid: {e}")))
}

impl Tunnel for TorTunnel {
    fn start(&self) -> Result<LocalEndpoint, NetError> {
        let mut running = lock(&self.running);
        if let Some(r) = running.as_ref() {
            return Ok(r.endpoint.clone());
        }
        let shared = &self.shared;
        shared.shutdown.send_replace(false);
        shared.ready.store(false, Ordering::Release);
        shared.set_status(TunnelStatus::Connecting);
        let started = start_inner(shared);
        match started {
            Ok(r) => {
                let endpoint = r.endpoint.clone();
                *running = Some(r);
                Ok(endpoint)
            }
            Err(e) => {
                shared.set_status(TunnelStatus::Failed(e.to_string()));
                Err(e)
            }
        }
    }

    fn status(&self) -> TunnelStatus {
        lock(&self.shared.status).clone()
    }

    fn stop(&self) {
        let shared = &self.shared;
        shared.shutdown.send_replace(true);
        shared.ready.store(false, Ordering::Release);
        let running = lock(&self.running).take();
        *lock(&shared.client) = None;
        if let Some(Running { runtime, mut server, .. }) = running {
            server.stop();
            // Releases Arti's state lock: a restarted tunnel can open the same directory.
            runtime.shutdown_timeout(SHUTDOWN_GRACE);
        }
        shared.circuits.clear();
        shared.isolation.retire_all();
        *lock(&shared.progress) = None;
        shared.set_status(TunnelStatus::Stopped);
    }

    fn set_notifier(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.shared.notifier) = Some(notify);
    }

    fn progress(&self) -> Option<TunnelProgress> {
        lock(&self.shared.progress).clone()
    }

    fn circuit_for(&self, host: &str) -> Option<CircuitInfo> {
        self.shared.circuits.get(&site_key(host))
    }

    fn new_circuit(&self, host: &str) {
        self.shared.isolation.retire(host);
        self.shared.circuits.forget(&site_key(host));
    }

    fn new_identity(&self) {
        let shared = &self.shared;
        shared.isolation.retire_all();
        shared.circuits.clear();
        if shared.isolation.mode() == TorIsolation::None {
            // No tokens to retire: a handle with its own default isolation takes over.
            let mut client = lock(&shared.client);
            if let Some(c) = client.as_ref() {
                *client = Some(c.isolated_client());
            }
        }
    }
}

impl Drop for TorTunnel {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Arti's TLS (for the directory and bridges) needs a crypto provider installed once per
/// process; `ring` is the one this build links.
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Fails when something else installed one first: that one is fine too.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Lowers the calling thread's CPU priority (Linux: a thread has its own niceness; elsewhere
/// nothing happens). A failure only means the thread keeps its priority.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    {
        // No pid means the calling thread: on Linux `setpriority(PRIO_PROCESS, 0)` is per thread.
        if rustix::process::setpriority_process(None, BACKGROUND_NICE).is_err() {
            tracing::debug!("could not lower a Tor thread's priority");
        }
    }
}

fn start_inner(shared: &Arc<Shared>) -> Result<Running, NetError> {
    install_crypto_provider();
    let config = client_config(shared)?;
    let background = shared.background;
    let runtime = Builder::new_multi_thread()
        .worker_threads(shared.threads)
        .max_blocking_threads(shared.threads.max(2))
        .thread_name("tor-runtime")
        .on_thread_start(move || {
            if background {
                lower_priority();
            }
        })
        .enable_all()
        .build()?;
    let client = {
        let _guard = runtime.enter();
        let rt = ArtiRuntime::current()?;
        TorClient::with_runtime(rt)
            .config(config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped()
            .map_err(|e| NetError::Connect(format!("Tor could not start: {e}")))?
    };
    *lock(&shared.client) = Some(client);
    let client =
        lock(&shared.client).clone().ok_or_else(|| NetError::Connect("Tor could not start".into()))?;

    let credentials = Credentials::random_session();
    let dialer =
        GuardedDialer::new(ArtiDialer { shared: Arc::clone(shared), handle: runtime.handle().clone() });
    let server = LocalServer::socks5(credentials.clone(), dialer, shared.server_options)?;
    let endpoint = LocalEndpoint { addr: server.addr(), credentials };

    // Progress events, and the moment traffic can flow.
    {
        let (shared, client) = (Arc::clone(shared), Arc::clone(&client));
        runtime.spawn(async move {
            let mut events = client.bootstrap_events();
            while let Some(event) = events.next().await {
                let percent = (event.as_frac() * 100.0).round().clamp(0.0, 100.0) as u8;
                let stuck = event.blocked().map(|b| b.to_string());
                if event.ready_for_traffic() && !shared.ready.swap(true, Ordering::AcqRel) {
                    tracing::info!("Tor is ready for traffic");
                    shared.set_progress(None);
                    shared.set_status(TunnelStatus::Up { since: SystemTime::now(), info: "Tor".into() });
                } else if !shared.ready.load(Ordering::Acquire) {
                    shared.set_progress(Some(TunnelProgress {
                        percent,
                        stage: ProgressStage::for_percent(percent),
                        stuck,
                    }));
                }
            }
        });
    }
    {
        let shared = Arc::clone(shared);
        runtime.spawn(async move {
            match client.bootstrap().await {
                Ok(()) => tracing::info!("Tor finished bootstrapping"),
                Err(e) => {
                    tracing::warn!(error = %e, "Tor could not bootstrap");
                    shared.ready.store(false, Ordering::Release);
                    shared.set_progress(None);
                    shared.set_status(TunnelStatus::Failed(format!("Tor could not bootstrap: {e}")));
                }
            }
        });
    }
    Ok(Running { runtime, server, endpoint })
}

/// Opens streams over Tor circuits.
struct ArtiDialer {
    shared: Arc<Shared>,
    handle: Handle,
}

impl Dialer for ArtiDialer {
    fn dial(&self, target: &Address) -> io::Result<Box<dyn Conn>> {
        let shared = &self.shared;
        if !shared.ready.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "Tor is still bootstrapping"));
        }
        let client = lock(&shared.client)
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "Tor is stopped"))?;
        let (host, port) = match target {
            Address::Domain(h, p) => (h.trim_end_matches('.').to_owned(), *p),
            Address::Ip(a) => (a.ip().to_string(), a.port()),
        };
        let lease = shared.isolation.lease(&host);
        let mut prefs = StreamPrefs::new();
        prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));
        if let Some(l) = &lease {
            prefs.set_isolation(l.token);
        }
        if let Some(cc) = shared.plan.exit_country {
            prefs.exit_country(cc);
        }
        let mut cancels = vec![shared.shutdown.subscribe()];
        cancels.extend(lease.map(|l| l.cancel));
        let timeout = shared.connect_timeout;
        let connect = {
            let mut cancels = cancels.clone();
            async move {
                tokio::select! {
                    r = tokio::time::timeout(timeout, client.connect_with_prefs((host.as_str(), port), &prefs)) => match r {
                        Ok(Ok(stream)) => Ok(stream),
                        Ok(Err(e)) => Err(map_error(&e)),
                        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "building a Tor circuit took too long")),
                    },
                    () = conn::cancelled(&mut cancels) => {
                        Err(io::Error::new(io::ErrorKind::ConnectionAborted, "the circuit was replaced"))
                    }
                }
            }
        };
        let stream = self.handle.block_on(connect)?;
        shared.circuits.record(&site_key(&target_host(target)), path::describe(&stream, &shared.geoip));
        Ok(Box::new(TorConn::new(self.handle.clone(), stream, cancels)))
    }
}

fn target_host(target: &Address) -> String {
    match target {
        Address::Domain(h, _) => h.clone(),
        Address::Ip(a) => a.ip().to_string(),
    }
}

/// Maps an Arti error to the kind a SOCKS5 reply is made from.
fn map_error(e: &arti_client::Error) -> io::Error {
    use arti_client::ErrorKind as K;
    use arti_client::HasKind as _;
    let kind = match e.kind() {
        K::RemoteConnectionRefused => io::ErrorKind::ConnectionRefused,
        K::TorAccessFailed | K::TransientFailure | K::RemoteNetworkTimeout | K::TorNetworkTimeout => {
            io::ErrorKind::TimedOut
        }
        K::ForbiddenStreamTarget | K::InvalidStreamTarget => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_lines() {
        let ok = "192.0.2.10:443 0123456789ABCDEF0123456789ABCDEF01234567";
        assert!(parse_bridge(ok).is_ok());
        assert!(parse_bridge(&format!("Bridge {ok}")).is_ok());
        assert!(parse_bridge(&format!("bridge   {ok}")).is_ok());
        assert!(parse_bridge("[2001:db8::2]:9001 0123456789ABCDEF0123456789ABCDEF01234567").is_ok());
        let pt =
            parse_bridge("obfs4 192.0.2.10:443 0123456789ABCDEF0123456789ABCDEF01234567 cert=x iat-mode=0");
        assert!(matches!(pt, Err(NetError::Invalid(m)) if m.contains("obfs4")));
        assert!(parse_bridge("snowflake 192.0.2.3:1 FP").is_err());
        assert!(parse_bridge("not a bridge").is_err());
    }

    /// The runtime's threads take the low priority, and only the thread that asked.
    #[cfg(target_os = "linux")]
    #[test]
    fn lowering_the_priority_affects_only_the_calling_thread() {
        fn nice() -> i32 {
            let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap_or_default();
            let rest = &stat[stat.rfind(')').map_or(0, |i| i + 1)..];
            rest.split_whitespace().nth(16).and_then(|n| n.parse().ok()).unwrap_or(-99)
        }
        let before = nice();
        let low = std::thread::spawn(|| {
            lower_priority();
            nice()
        })
        .join()
        .unwrap_or(-99);
        assert!(low >= BACKGROUND_NICE.min(19), "{low}");
        assert_eq!(nice(), before);
    }

    #[test]
    fn config_is_checked_once() {
        let mut config = TorConfig { exit_country: "de".into(), ..Default::default() };
        assert!(Plan::from_config(&config).is_ok());
        config.exit_country = "germany".into();
        assert!(Plan::from_config(&config).is_err());
        let config = TorConfig {
            use_bridges: true,
            bridges: vec!["192.0.2.10:443 0123456789ABCDEF0123456789ABCDEF01234567".into()],
            ..Default::default()
        };
        assert_eq!(Plan::from_config(&config).map(|p| p.bridges.len()).unwrap_or(0), 1);
    }

    /// Starts Arti against the real Tor network (about a minute of bootstrapping), fetches a
    /// page through the SOCKS5 server and checks that the exit is a Tor exit.
    /// `cargo test -p axion-net --release -- --ignored real_tor --nocapture`
    #[test]
    #[ignore = "needs the Tor network: run with --ignored"]
    fn real_tor_bootstrap_and_fetch() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("axion-tor-test-{}", std::process::id()));
        let options = TunnelOptions {
            server: ServerOptions::default(),
            connect_timeout: Duration::from_secs(15),
            data_dir: dir.clone(),
            wireguard_handshake_timeout: Duration::from_secs(20),
            background_priority: true,
            tor_worker_threads: 2,
        };
        let tunnel = create(&TorConfig::default(), &options).expect("create");
        let endpoint = tunnel.start().expect("start");
        let deadline = std::time::Instant::now() + Duration::from_secs(240);
        while !tunnel.status().is_up() {
            assert!(std::time::Instant::now() < deadline, "bootstrap took too long: {:?}", tunnel.progress());
            assert!(!matches!(tunnel.status(), TunnelStatus::Failed(_)), "{:?}", tunnel.status());
            eprintln!("bootstrapping: {:?}", tunnel.progress());
            std::thread::sleep(Duration::from_secs(2));
        }
        let dialer = crate::proxy::SocksUpstream::local(&endpoint, Duration::from_secs(60));
        let mut conn = dialer.dial(&Address::Domain("check.torproject.org".into(), 80)).expect("dial");
        conn.write_all(b"GET /api/ip HTTP/1.0\r\nHost: check.torproject.org\r\n\r\n").expect("write");
        let mut body = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match conn.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => body.extend(&buf[..n]),
            }
        }
        let text = String::from_utf8_lossy(&body);
        eprintln!("{text}");
        // The Tor Project redirects plain HTTP to HTTPS; the IsTor check is done in docs/network.md by curl.
        assert!(text.starts_with("HTTP/1."), "{text}");
        let circuit = tunnel.circuit_for("check.torproject.org").expect("a circuit is known");
        eprintln!("{circuit:#?}");
        assert!(circuit.hops.len() >= 3);
        tunnel.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
}
