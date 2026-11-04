//! The browser's side of the privacy network (`axion-net`): the connection manager, the
//! secrets cache fed from the vault, what the engine is told, the address field's chip state
//! and the "connection is down" page.
//!
//! Secrets (proxy passwords, WireGuard keys) are encrypted with the device key (TPM-sealed,
//! no master password) in a file of the profile, by connection id ([`AppSecrets`]), so a
//! connection starts with the password vault locked. Secrets an older version left in the
//! vault are read through and moved over on the next unlock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use axion_config::{Config, Network};
use axion_engine::{
    Engine, InternalPages, InternalRequest, InternalResponse, ProxyLogin, ProxySetting, ViewId,
};
use axion_i18n::{tr, tr_fmt};
use axion_net::{
    Connection, ConnectionKind, ConnectionSecrets, ConnectionStatus, NetEvent, NetManager, NetOptions, Plan,
    ProgressStage, ProxyTimings, RouteQuery, Secret, SecretError, SecretStore, ServerOptions, Target,
    WgSecrets,
};
use axion_vault::{SealedStore, SystemDeviceKey};

/// The host of the "connection is down" page (`axion://network/blocked?…`).
pub const HOST: &str = "network";

/// Work for the secrets thread: the sealed file is only touched there (the first use of the
/// device key may wait for the TPM), never on the UI thread.
enum Job {
    Warm,
    Set(String, String),
    Remove(String),
    /// Secrets that so far lived in the password vault: moved into the sealed store.
    Migrate(Vec<(String, String)>),
}

/// What the secrets thread reports back to the main thread.
#[derive(Debug, PartialEq, Eq)]
pub enum SecretsEvent {
    /// These connections' secrets are in the sealed store now; the vault's copy can go.
    Migrated(Vec<String>),
    /// Writing to the sealed store failed (the secrets are still in memory for this session).
    WriteFailed(String),
}

#[derive(Default)]
struct SecretsInner {
    /// `None`: no place for the device key at all (no home directory).
    store: Option<Arc<SealedStore>>,
    /// What was read from or written to the sealed store.
    cache: Mutex<HashMap<String, ConnectionSecrets>>,
    /// Secrets still in the password vault (older versions), while it is unlocked.
    legacy: Mutex<Option<HashMap<String, ConnectionSecrets>>>,
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SecretsInner {
    fn load(&self, id: &str) -> Result<ConnectionSecrets, SecretError> {
        if let Some(found) = guard(&self.cache).get(id) {
            return Ok(found.clone());
        }
        if let Some(store) = &self.store {
            match store.get(id) {
                Ok(Some(text)) => {
                    return match secrets_from_json(&text) {
                        Some(found) => {
                            guard(&self.cache).insert(id.to_owned(), found.clone());
                            Ok(found)
                        }
                        None => Err(SecretError::Unavailable("The stored secrets are damaged.".into())),
                    };
                }
                Ok(None) => {}
                Err(e) => return Err(SecretError::Unavailable(e.to_string())),
            }
        }
        // Read-through for secrets an older version left in the vault.
        match guard(&self.legacy).as_ref() {
            Some(map) => Ok(map.get(id).cloned().unwrap_or_default()),
            None => Err(SecretError::Locked),
        }
    }

    fn run(&self, job: Job) -> Option<SecretsEvent> {
        let Some(store) = &self.store else {
            return match job {
                Job::Set(..) | Job::Migrate(_) => {
                    Some(SecretsEvent::WriteFailed("no place for the device key".into()))
                }
                _ => None,
            };
        };
        match job {
            Job::Warm => {
                if let Err(e) = store.warm_up() {
                    tracing::warn!("network secrets: {e}");
                    return None;
                }
                match store.all() {
                    Ok(all) => {
                        let mut cache = guard(&self.cache);
                        for (id, text) in all {
                            if let Some(s) = secrets_from_json(&text) {
                                cache.entry(id).or_insert(s);
                            }
                        }
                    }
                    Err(e) => tracing::warn!("network secrets: {e}"),
                }
                None
            }
            Job::Set(id, text) => store.set(&id, &text).err().map(|e| {
                tracing::error!(error = %e, "storing connection secrets failed");
                SecretsEvent::WriteFailed(e.to_string())
            }),
            Job::Remove(id) => {
                // After an earlier warm-up job may have read the entry back into the cache.
                guard(&self.cache).remove(&id);
                if let Err(e) = store.remove(&id) {
                    tracing::warn!("removing connection secrets failed: {e}");
                }
                None
            }
            Job::Migrate(items) => {
                let mut done = Vec::new();
                for (id, text) in items {
                    let Some(parsed) = secrets_from_json(&text) else { continue };
                    // What the sealed store already holds wins (it is newer than the vault's copy).
                    let present = matches!(store.get(&id), Ok(Some(_)));
                    if present || store.set(&id, &text).is_ok() {
                        guard(&self.cache).entry(id.clone()).or_insert(parsed);
                        done.push(id);
                    } else {
                        tracing::warn!("could not move a connection's secrets out of the vault");
                    }
                }
                (!done.is_empty()).then_some(SecretsEvent::Migrated(done))
            }
        }
    }
}

/// Secrets of connections, as the manager's worker threads see them.
///
/// They live in a file encrypted with the device key (`axion_vault::SealedStore`), so a
/// connection starts without the password vault. Secrets an older version put in the vault are
/// read through while it is unlocked and moved over on that unlock.
pub struct AppSecrets {
    inner: Arc<SecretsInner>,
    jobs: Mutex<mpsc::Sender<Job>>,
    events: Mutex<mpsc::Receiver<SecretsEvent>>,
}

impl Default for AppSecrets {
    /// Without a sealed store (tests): nothing is kept across restarts.
    fn default() -> Self {
        Self::new(None)
    }
}

impl AppSecrets {
    pub fn new(store: Option<Arc<SealedStore>>) -> Self {
        let inner = Arc::new(SecretsInner { store, ..Default::default() });
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = Arc::clone(&inner);
        let spawned = std::thread::Builder::new().name("net-secrets".into()).spawn(move || {
            while let Ok(job) = job_rx.recv() {
                if let Some(event) = worker.run(job) {
                    let _ = event_tx.send(event);
                }
            }
        });
        if let Err(e) = spawned {
            tracing::error!("could not start the secrets thread: {e}");
        }
        let secrets = Self { inner, jobs: Mutex::new(job_tx), events: Mutex::new(event_rx) };
        secrets.submit(Job::Warm);
        secrets
    }

    fn submit(&self, job: Job) {
        if guard(&self.jobs).send(job).is_err() {
            tracing::error!("the secrets thread is gone");
        }
    }

    /// The sealed store in its usual place (`AXION_DEVICE_KEY_DIR` or the data directory), the
    /// file in the profile's `network` directory.
    pub fn system(profile: &std::path::Path) -> Self {
        let store = match SystemDeviceKey::system() {
            Ok(device) => Some(Arc::new(SealedStore::new(
                profile.join("network").join(SEALED_SECRETS_FILE),
                Arc::new(device),
            ))),
            Err(e) => {
                tracing::warn!("network secrets can't be kept: {e}");
                None
            }
        };
        Self::new(store)
    }

    fn fill_legacy(&self, secrets: HashMap<String, ConnectionSecrets>) {
        *guard(&self.inner.legacy) = Some(secrets);
    }

    fn clear_legacy(&self) {
        *guard(&self.inner.legacy) = None;
    }

    /// Keeps `secrets` in memory and writes them to the sealed store on the secrets thread.
    fn put(&self, id: &str, secrets: &ConnectionSecrets) {
        guard(&self.inner.cache).insert(id.to_owned(), secrets.clone());
        self.submit(Job::Set(id.to_owned(), secrets_to_json(secrets)));
    }

    fn forget(&self, id: &str) {
        guard(&self.inner.cache).remove(id);
        if let Some(map) = guard(&self.inner.legacy).as_mut() {
            map.remove(id);
        }
        self.submit(Job::Remove(id.to_owned()));
    }

    fn migrate(&self, items: Vec<(String, String)>) {
        if !items.is_empty() {
            self.submit(Job::Migrate(items));
        }
    }

    fn take_events(&self) -> Vec<SecretsEvent> {
        guard(&self.events).try_iter().collect()
    }
}

impl SecretStore for AppSecrets {
    fn load(&self, id: &str) -> Result<ConnectionSecrets, SecretError> {
        self.inner.load(id)
    }

    fn save(&self, id: &str, secrets: &ConnectionSecrets) -> Result<(), SecretError> {
        self.put(id, secrets);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        self.forget(id);
        Ok(())
    }
}

/// The sealed secrets file's name in `<profile>/network/`.
const SEALED_SECRETS_FILE: &str = "secrets.sealed.json";

/// The text kept in the vault for a connection's secrets.
pub fn secrets_to_json(s: &ConnectionSecrets) -> String {
    let mut v = serde_json::json!({ "password": s.password.expose() });
    if let Some(wg) = &s.wireguard {
        v["wg_private"] = wg.private_key.expose().into();
        v["wg_preshared"] =
            wg.preshared_keys.iter().map(|k| k.expose().to_owned()).collect::<Vec<_>>().into();
    }
    v.to_string()
}

pub fn secrets_from_json(text: &str) -> Option<ConnectionSecrets> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let text_of = |key: &str| v.get(key).and_then(|x| x.as_str()).map(str::to_owned);
    let wireguard = text_of("wg_private").map(|private| WgSecrets {
        private_key: Secret::new(private),
        preshared_keys: v
            .get("wg_preshared")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|k| k.as_str()).map(Secret::new).collect())
            .unwrap_or_default(),
    });
    Some(ConnectionSecrets { password: Secret::new(text_of("password").unwrap_or_default()), wireguard })
}

/// The file chooser for WireGuard configs: the portal's dialog runs on a thread, the pick
/// is collected on the next tick.
#[derive(Default)]
pub struct ConfChooser {
    picked: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl ConfChooser {
    pub fn open(&self) {
        let picked = Arc::clone(&self.picked);
        let spawned = std::thread::Builder::new().name("choose-wireguard".into()).spawn(move || {
            match crate::downloads::choose_portal(
                tr("Import WireGuard Config"),
                crate::downloads::Choose::WireGuardConfig,
            ) {
                Ok(Some(path)) => *picked.lock().unwrap_or_else(PoisonError::into_inner) = Some(path),
                Ok(None) => {}
                Err(e) => tracing::warn!("file chooser unavailable: {e}"),
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("could not open the file chooser: {e}");
        }
    }

    pub fn take(&self) -> Option<std::path::PathBuf> {
        self.picked.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// How a route looks in the address field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipState {
    Up,
    Connecting,
    /// Not up and not usable: traffic is blocked (or goes direct with the kill switch off).
    Down,
    /// This site goes direct although the tab's default is a connection.
    Bypassed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub state: ChipState,
    /// The connection's name (the default's, for a bypassed site).
    pub name: String,
    pub target: Target,
    /// What the status says ("Connected", a failure reason, …).
    pub detail: String,
    /// The route is the session route ("Use X for this session"), not the settings' routing.
    pub session: bool,
}

/// A page that failed because its connection is down.
#[derive(Clone, Debug)]
pub struct Blocked {
    pub url: String,
    pub connection: String,
}

pub struct NetworkState {
    pub manager: NetManager,
    secrets: Arc<AppSecrets>,
    applied: Option<Plan>,
    vault_unlocked: bool,
    /// Views showing the "connection is down" page, and what they were loading.
    pub blocked: HashMap<ViewId, Blocked>,
    /// Views whose proxy failed while its connection still looked up: the page shows if the
    /// recheck finds it down.
    pub suspects: HashMap<ViewId, Blocked>,
    /// Private tabs have been used (their route was started).
    private_started: bool,
}

impl NetworkState {
    pub fn new(config: &Config, profile: &std::path::Path) -> Self {
        Self::with_secrets(config, profile, AppSecrets::system(profile))
    }

    /// With a given secrets store (tests use one that never touches the real device key).
    pub fn with_secrets(config: &Config, profile: &std::path::Path, secrets: AppSecrets) -> Self {
        let secrets = Arc::new(secrets);
        let manager = NetManager::new(secrets.clone(), options(&config.network, profile));
        manager.set_routing(config.network.effective_routing());
        Self {
            manager,
            secrets,
            applied: None,
            vault_unlocked: false,
            blocked: HashMap::new(),
            suspects: HashMap::new(),
            private_started: false,
        }
    }

    /// New routing settings from `config.toml` or Settings.
    pub fn apply_config(&mut self, config: &Config) {
        self.manager.set_routing(config.network.effective_routing());
    }

    /// Starts the connections the account's tabs use (when `connect_at_start` is on).
    pub fn start_default_route(&mut self, config: &Config) {
        if config.network.connect_at_start {
            self.manager.ensure_started(false);
        }
    }

    /// Private tabs opened: start their route (once).
    pub fn private_tabs_opened(&mut self) {
        if !self.private_started {
            self.private_started = true;
            self.manager.ensure_started(true);
        }
    }

    /// Follows the vault. Its unlock makes the secrets an older version left in it readable,
    /// moves them into the sealed store (see [`NetworkState::take_secret_events`]) and gives
    /// blocked connections a new try; its lock forgets the vault's copies. Connections whose
    /// secrets are in the sealed store never depend on it. Returns whether anything changed.
    pub fn sync_vault(&mut self, vault: Option<Vec<(String, String)>>) -> bool {
        match vault {
            Some(items) if !self.vault_unlocked => {
                let map = items
                    .iter()
                    .filter_map(|(id, text)| Some((id.clone(), secrets_from_json(text)?)))
                    .collect();
                let migrate = items;
                self.secrets.fill_legacy(map);
                self.secrets.migrate(migrate);
                self.vault_unlocked = true;
                let routing = self.manager.routing();
                for c in &routing.connections {
                    if matches!(self.manager.status(&c.id), Some(ConnectionStatus::Blocked(_))) {
                        self.manager.start(&c.id);
                    }
                }
                true
            }
            None if self.vault_unlocked => {
                self.secrets.clear_legacy();
                self.vault_unlocked = false;
                true
            }
            _ => false,
        }
    }

    pub fn vault_unlocked(&self) -> bool {
        self.vault_unlocked
    }

    /// A connection was saved in Settings: keep its secrets in memory and write them, encrypted
    /// with the device key, on the secrets thread (never the UI thread).
    pub fn store_secrets(&self, id: &str, secrets: &ConnectionSecrets) {
        self.secrets.put(id, secrets);
    }

    /// The stored secrets of a connection (`None` when there are none or they can't be read).
    pub fn secrets_of(&self, id: &str) -> Option<ConnectionSecrets> {
        self.secrets.load(id).ok().filter(|s| !s.password.is_empty() || s.wireguard.is_some())
    }

    /// What the secrets thread finished since the last call.
    pub fn take_secret_events(&self) -> Vec<SecretsEvent> {
        self.secrets.take_events()
    }

    pub fn forget_secrets(&self, id: &str) {
        self.secrets.forget(id);
    }

    /// Looks at the connections; returns what changed.
    pub fn poll(&mut self) -> Vec<NetEvent> {
        self.manager.poll()
    }

    /// Tells the engine what changed since the last time. Returns whether it did.
    pub fn apply_to_engine(&mut self, engine: &mut impl Engine, config: &Config) -> bool {
        let plan = self.manager.plan();
        if self.applied.as_ref() == Some(&plan) {
            return false;
        }
        let protect = config.network.leak_protection;
        let setting = |s: &axion_net::ProxySetting| match s {
            axion_net::ProxySetting::Direct => ProxySetting::Direct,
            axion_net::ProxySetting::Fixed(server) => ProxySetting::Fixed(server.clone()),
            axion_net::ProxySetting::Pac(script) => ProxySetting::Pac(script.clone()),
        };
        // Logins first: a request that reaches the new proxy must find its login.
        engine.set_proxy_logins(
            plan.logins
                .iter()
                .map(|l| ProxyLogin {
                    host: l.host.clone(),
                    port: l.port,
                    username: l.credentials.username.clone(),
                    password: l.credentials.password.expose().to_owned(),
                })
                .collect(),
        );
        engine.set_proxy(false, setting(&plan.normal.setting), plan.normal.leak_protection && protect);
        engine.set_proxy(true, setting(&plan.private.setting), plan.private.leak_protection && protect);
        self.applied = Some(plan);
        true
    }

    /// The route of a page, and how its connection is doing.
    pub fn route_of(&self, config: &Config, url: &str, private: bool) -> Target {
        let Some(host) = axion_net::host_of_url(url) else {
            return config.network.effective_routing().default_for(private).clone();
        };
        self.manager.resolve(&RouteQuery { host: &host, private, tab_override: None })
    }

    /// What the address field's chip shows for a page; `None`: nothing to show (no connection
    /// is set up at all). With connections but a direct page the chip says "Direct", so the
    /// popover (and "Use X for this session") is always one click away.
    pub fn chip_for(&self, config: &Config, url: &str, private: bool) -> Option<Chip> {
        if !config.network.show_chip || !url_is_web(url) {
            return None;
        }
        let target = self.route_of(config, url, private);
        let routing = config.network.effective_routing();
        let session = config.network.session_target().is_some();
        match &target {
            Target::Direct => {
                let default = routing.default_for(private);
                let (name, target) = match default.connection_id() {
                    Some(id) => (target_label(&config.network, default), Target::Connection(id.to_owned())),
                    None if routing.connections.is_empty() => return None,
                    None => (tr("Direct").to_owned(), Target::Direct),
                };
                Some(Chip { state: ChipState::Bypassed, name, target, detail: String::new(), session })
            }
            Target::Connection(id) => {
                let status = self.manager.status(id);
                let (state, detail) = match &status {
                    Some(ConnectionStatus::Up { .. }) => (ChipState::Up, tr("Connected").to_owned()),
                    Some(ConnectionStatus::Connecting) | Some(ConnectionStatus::Stopped) => {
                        // Tor says how far its bootstrap is.
                        let detail = match self.manager.progress(id) {
                            Some(p) => bootstrap_text(Some(p)),
                            None => tr("Connecting…").to_owned(),
                        };
                        (ChipState::Connecting, detail)
                    }
                    Some(ConnectionStatus::Failed(r) | ConnectionStatus::Blocked(r)) => {
                        (ChipState::Down, r.clone())
                    }
                    None => (ChipState::Down, tr("This connection was removed.").to_owned()),
                };
                Some(Chip { state, name: target_label(&config.network, &target), target, detail, session })
            }
        }
    }
}

fn url_is_web(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The manager's options from the settings.
pub fn options(network: &Network, profile: &std::path::Path) -> NetOptions {
    NetOptions {
        timings: ProxyTimings {
            connect_timeout: Duration::from_secs_f32(network.connect_timeout_secs.clamp(1.0, 120.0)),
            health_interval: Duration::from_secs_f32(network.health_check_secs.clamp(1.0, 600.0)),
        },
        server: ServerOptions {
            max_connections: network.max_relay_connections.clamp(8, 8192) as usize,
            trust_own_process_tree: network.front_trust_own_processes,
            ..Default::default()
        },
        data_dir: profile.join("network"),
        wireguard_handshake_timeout: Duration::from_secs_f32(
            network.wireguard_handshake_timeout_secs.clamp(2.0, 300.0),
        ),
        background_priority: network.background_priority,
        tor_worker_threads: network.tor_worker_threads.clamp(1, 8) as usize,
    }
}

/// Chromium switches for the whole process when a route is set at start (they cannot change
/// later): background services off, QUIC off, and optionally no local name resolution.
pub fn startup_switches(network: &Network) -> Vec<(String, Option<String>)> {
    let routing = network.effective_routing();
    if !network.leak_protection || !routing.any_routed() {
        return Vec::new();
    }
    let strict = network.strict_dns && routing.always_routed(false) && routing.always_routed(true);
    let rules = strict.then(|| {
        let hosts: Vec<String> = network
            .connections
            .iter()
            .filter_map(|c| match &c.kind {
                ConnectionKind::Proxy(p) if !matches!(p.host.as_str(), "127.0.0.1" | "::1" | "localhost") => {
                    Some(p.host.clone())
                }
                _ => None,
            })
            .collect();
        axion_engine::proxy::strict_dns_rules(&hosts)
    });
    axion_engine::proxy::hardening_switches(network.disable_quic_when_routed, rules.as_deref())
        .into_iter()
        .map(|(n, v)| (n.to_owned(), v))
        .collect()
}

/// Names a route in the current language ("Direct", or the connection's name).
pub fn target_label(network: &Network, target: &Target) -> String {
    match target {
        Target::Direct => tr("Direct").to_owned(),
        Target::Connection(id) => {
            network.connection(id).map_or_else(|| tr("Unknown connection").to_owned(), |c| c.name.clone())
        }
    }
}

/// A connection's one-line description for lists: "SOCKS5 · proxy.example:1080".
pub fn describe(c: &Connection) -> String {
    match &c.kind {
        ConnectionKind::Direct => tr("Direct").to_owned(),
        ConnectionKind::Proxy(p) => format!("{} · {}", p.scheme.label(), p.host_port()),
        ConnectionKind::WireGuard(w) => {
            let endpoint =
                w.peers.iter().find(|p| !p.endpoint.is_empty()).map(|p| p.endpoint.as_str()).unwrap_or("");
            format!("WireGuard · {endpoint}")
        }
        ConnectionKind::Tor(t) => {
            if t.use_bridges {
                "Tor · bridges".to_owned()
            } else {
                "Tor".to_owned()
            }
        }
    }
}

// ---- the "connection is down" page ----

/// `axion://network/blocked?url=…&c=<id>&n=<name>`.
pub fn blocked_url(url: &str, connection: &str, name: &str) -> String {
    format!("axion://{HOST}/blocked?url={}&c={}&n={}", encode(url), encode(connection), encode(name))
}

/// The page this URL stands for, if it is a blocked page.
pub fn parse_blocked_url(url: &str) -> Option<(String, String)> {
    let (host, path, query) = axion_engine::internal::split_internal_url(url)?;
    if host != HOST || path != "/blocked" {
        return None;
    }
    Some((query_value(query, "url")?, query_value(query, "c").unwrap_or_default()))
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn query_value(query: &str, key: &str) -> Option<String> {
    let raw =
        query.split('&').find_map(|pair| pair.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = raw.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// What the page's buttons send (`{"action": …}`), parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockedAction {
    Retry {
        url: String,
        connection: String,
    },
    /// Load the site without the connection (it gets a site rule "direct").
    Direct {
        url: String,
    },
    Settings,
}

pub fn parse_blocked_message(message: &str) -> Option<BlockedAction> {
    let v: serde_json::Value = serde_json::from_str(message).ok()?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
    Some(match v.get("action")?.as_str()? {
        "retry" => BlockedAction::Retry { url: text("url"), connection: text("c") },
        "direct" => BlockedAction::Direct { url: text("url") },
        "settings" => BlockedAction::Settings,
        _ => return None,
    })
}

/// Serves `axion://network/…`.
#[derive(Default)]
pub struct NetworkPages;

impl InternalPages for NetworkPages {
    fn serve(&self, request: &InternalRequest<'_>) -> Option<InternalResponse> {
        if request.host != HOST {
            return None;
        }
        Some(match request.path {
            "/blocked" => InternalResponse::ok("text/html", blocked_html()),
            "/blocked.css" => InternalResponse::ok("text/css", BLOCKED_CSS),
            "/blocked.js" => InternalResponse::ok("text/javascript", BLOCKED_JS),
            _ => return None,
        })
    }
}

fn blocked_html() -> String {
    let t = |s: &str| escape(s);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light dark\">\
<title>{title}</title><link rel=\"stylesheet\" href=\"/blocked.css\"></head><body>\
<main><div class=\"icon\" aria-hidden=\"true\"><svg viewBox=\"0 0 24 24\" width=\"44\" height=\"44\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M12 20h.01M8.5 16.429a5 5 0 0 1 7 0M5 12.859a10 10 0 0 1 5.17-2.69M19 12.859a10 10 0 0 0-2.007-1.523M2 8.82a15 15 0 0 1 4.177-2.643M22 8.82a15 15 0 0 0-11.288-3.764M2 2l20 20\"/></svg></div>\
<h1 id=\"title\" data-template=\"{heading}\"></h1>\
<p class=\"lead\">{lead}</p><p class=\"url\" id=\"url\"></p>\
<div class=\"buttons\"><button id=\"retry\" class=\"primary\">{retry}</button>\
<button id=\"direct\">{direct}</button><button id=\"settings\">{settings}</button></div>\
<div id=\"confirm\" hidden><p id=\"confirm-text\" data-template=\"{confirm}\"></p>\
<div class=\"buttons\"><button id=\"confirm-yes\" class=\"danger\">{confirm_yes}</button><button id=\"confirm-no\">{cancel}</button></div></div>\
</main><script src=\"/blocked.js\"></script></body></html>",
        title = t(tr("Connection is down")),
        heading = t(tr("Connection to {name} is down")),
        lead = t(tr(
            "Axion blocked this request to protect you. It was not sent without the connection, so the site never saw your real address."
        )),
        retry = t(tr("Retry")),
        direct = t(tr("Use direct connection")),
        settings = t(tr("Open network settings")),
        confirm = t(tr(
            "Load this site without {name}? The site will see your real IP address, and this is remembered for the site until you change it in the network settings."
        )),
        confirm_yes = t(tr("Connect directly")),
        cancel = t(tr("Cancel")),
    )
}

const BLOCKED_CSS: &str = "\
:root{color-scheme:light dark;--fg:#1d1d1f;--bg:#f5f5f7;--sec:#6e6e73;--card:#fff;--line:#d2d2d7;--accent:#0a84ff;--red:#d0362b}\
@media (prefers-color-scheme:dark){:root{--fg:#f5f5f7;--bg:#1c1c1e;--sec:#98989d;--card:#2c2c2e;--line:#3a3a3c}}\
html,body{margin:0;height:100%;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}\
main{max-width:34em;margin:0 auto;padding:16vh 1.5em 2em;text-align:center}\
.icon{color:var(--red);margin-bottom:.5em}h1{font-size:1.6em;margin:.2em 0 .4em;font-weight:600}\
.lead{color:var(--sec);margin:0 0 1em}.url{font-family:ui-monospace,monospace;font-size:.85em;color:var(--sec);word-break:break-all;margin:0 0 1.6em}\
.buttons{display:flex;gap:.6em;justify-content:center;flex-wrap:wrap}\
button{font:inherit;padding:.55em 1.1em;border-radius:999px;border:1px solid var(--line);background:var(--card);color:var(--fg);cursor:pointer}\
button:hover{border-color:var(--accent)}button.primary{background:var(--accent);border-color:var(--accent);color:#fff}\
button.danger{background:var(--red);border-color:var(--red);color:#fff}#confirm{margin-top:1.6em}#confirm p{color:var(--sec)}";

const BLOCKED_JS: &str = "\
(function(){\
var q=new URLSearchParams(location.search);var url=q.get('url')||'';var c=q.get('c')||'';var n=q.get('n')||'';\
function fill(el){el.textContent=el.dataset.template.split('{name}').join(n);}\
fill(document.getElementById('title'));fill(document.getElementById('confirm-text'));\
document.getElementById('url').textContent=url;\
function send(m){return fetch('/api/message',{method:'POST',body:JSON.stringify(m)});}\
document.getElementById('retry').onclick=function(){send({action:'retry',url:url,c:c});};\
document.getElementById('settings').onclick=function(){send({action:'settings'});};\
document.getElementById('direct').onclick=function(){document.getElementById('confirm').hidden=false;};\
document.getElementById('confirm-no').onclick=function(){document.getElementById('confirm').hidden=true;};\
document.getElementById('confirm-yes').onclick=function(){send({action:'direct',url:url});};\
})();";

/// "Bootstrapping 40%: loading relays" (the progress of a starting Tor connection).
pub fn bootstrap_text(progress: Option<axion_net::TunnelProgress>) -> String {
    let Some(p) = progress else { return tr("Starting Tor…").to_owned() };
    if let Some(stuck) = &p.stuck {
        return tr_fmt!("Bootstrapping {percent}%: {why}", percent = p.percent, why = stuck);
    }
    let stage = match p.stage {
        ProgressStage::Connecting => tr("connecting to the Tor network"),
        ProgressStage::Consensus => tr("fetching the list of relays"),
        ProgressStage::Relays => tr("loading relays"),
        ProgressStage::Ready => tr("almost ready"),
    };
    tr_fmt!("Bootstrapping {percent}%: {stage}", percent = p.percent, stage = stage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_json_roundtrip() {
        let s = ConnectionSecrets {
            password: Secret::new("pw"),
            wireguard: Some(WgSecrets {
                private_key: Secret::new("k"),
                preshared_keys: vec![Secret::new(""), Secret::new("p")],
            }),
        };
        assert_eq!(secrets_from_json(&secrets_to_json(&s)), Some(s));
        let only_pw = ConnectionSecrets { password: Secret::new("x"), wireguard: None };
        assert_eq!(secrets_from_json(&secrets_to_json(&only_pw)), Some(only_pw));
        assert_eq!(secrets_from_json("not json"), None);
    }

    /// A sealed store on a throwaway device key (file only, never the TPM or the real key).
    fn temp_store(name: &str) -> (std::path::PathBuf, Arc<SealedStore>) {
        let dir = std::env::temp_dir().join(format!("axion-net-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let device = Arc::new(SystemDeviceKey::file_only(dir.join("keys")));
        (dir.clone(), Arc::new(SealedStore::new(dir.join("network").join(SEALED_SECRETS_FILE), device)))
    }

    fn wg_secrets() -> ConnectionSecrets {
        ConnectionSecrets {
            password: Secret::new("pw"),
            wireguard: Some(WgSecrets {
                private_key: Secret::new("priv"),
                preshared_keys: vec![Secret::new("psk")],
            }),
        }
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("timed out waiting for the secrets thread");
    }

    #[test]
    fn without_a_store_and_a_locked_vault_nothing_is_available() {
        let s = AppSecrets::default();
        assert_eq!(s.load("c-1"), Err(SecretError::Locked));
        s.fill_legacy(HashMap::new());
        assert_eq!(s.load("c-1"), Ok(ConnectionSecrets::default()));
        s.clear_legacy();
        assert_eq!(s.load("c-1"), Err(SecretError::Locked));
    }

    #[test]
    fn secrets_load_without_the_vault_and_survive_a_restart() {
        let (dir, store) = temp_store("restart");
        let s = AppSecrets::new(Some(Arc::clone(&store)));
        // The vault was never unlocked: saving and loading still work.
        s.save("c-1", &wg_secrets()).expect("save");
        assert_eq!(s.load("c-1"), Ok(wg_secrets()));
        wait_for(|| store.names().is_ok_and(|n| n == ["c-1"]));
        // A new process (fresh cache) with the same device key reads it from the file.
        let device = Arc::new(SystemDeviceKey::file_only(dir.join("keys")));
        let again = AppSecrets::new(Some(Arc::new(SealedStore::new(store.path().to_owned(), device))));
        assert_eq!(again.load("c-1"), Ok(wg_secrets()));
        again.delete("c-1").expect("delete");
        wait_for(|| store.names().is_ok_and(|n| n.is_empty()));
        assert_eq!(again.load("c-1"), Err(SecretError::Locked), "nothing sealed, vault locked");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vault_secrets_are_read_through_and_migrated_on_unlock() {
        let (dir, store) = temp_store("migrate");
        let s = AppSecrets::new(Some(Arc::clone(&store)));
        // Locked vault, nothing sealed yet: the connection is blocked, as before.
        assert_eq!(s.load("old"), Err(SecretError::Locked));
        let text = secrets_to_json(&wg_secrets());
        let items = vec![("old".to_owned(), text.clone()), ("junk".to_owned(), "not json".to_owned())];
        s.fill_legacy(items.iter().filter_map(|(i, t)| Some((i.clone(), secrets_from_json(t)?))).collect());
        assert_eq!(s.load("old"), Ok(wg_secrets()), "read-through while unlocked");
        s.migrate(items);
        let mut events = Vec::new();
        wait_for(|| {
            events.extend(s.take_events());
            !events.is_empty()
        });
        assert_eq!(events, [SecretsEvent::Migrated(vec!["old".to_owned()])], "junk stays in the vault");
        // Locking the vault no longer matters.
        s.clear_legacy();
        assert_eq!(s.load("old"), Ok(wg_secrets()));
        assert_eq!(store.names().expect("names"), ["old"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_never_overwrites_newer_sealed_secrets() {
        let (dir, store) = temp_store("newer");
        let s = AppSecrets::new(Some(Arc::clone(&store)));
        let newer = ConnectionSecrets { password: Secret::new("new"), wireguard: None };
        s.save("c-1", &newer).expect("save");
        wait_for(|| store.names().is_ok_and(|n| !n.is_empty()));
        let old = secrets_to_json(&ConnectionSecrets { password: Secret::new("old"), wireguard: None });
        s.migrate(vec![("c-1".to_owned(), old)]);
        wait_for(|| !s.take_events().is_empty());
        assert_eq!(s.load("c-1"), Ok(newer));
        let sealed = store.get("c-1").expect("get").expect("present");
        assert_eq!(secrets_from_json(&sealed).expect("json").password.expose(), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_store_reports_instead_of_panicking() {
        let s = AppSecrets::default();
        s.save("c-1", &wg_secrets()).expect("kept in memory");
        let mut events = Vec::new();
        wait_for(|| {
            events.extend(s.take_events());
            !events.is_empty()
        });
        assert!(matches!(events[0], SecretsEvent::WriteFailed(_)));
        assert_eq!(s.load("c-1"), Ok(wg_secrets()), "still usable this session");
    }

    fn proxy(id: &str) -> Connection {
        Connection {
            id: id.into(),
            name: format!("Proxy {id}"),
            kind: ConnectionKind::Proxy(axion_net::ProxyConfig {
                host: "127.0.0.1".into(),
                port: 9,
                ..Default::default()
            }),
        }
    }

    fn state_with(config: &Config) -> NetworkState {
        let dir = std::env::temp_dir().join(format!("axion-net-state-{}", std::process::id()));
        NetworkState::with_secrets(config, &dir, AppSecrets::default())
    }

    #[test]
    fn the_session_route_changes_every_tab_and_restores_on_off() {
        let mut config = Config::default();
        config.network.connections = vec![proxy("a"), proxy("b")];
        config.network.default_connection = Target::Connection("a".into());
        config.network.site_rules =
            vec![axion_net::SiteRule { host: "example.com".into(), connection: Target::Direct }];
        let mut state = state_with(&config);
        let (a, b) = (Target::Connection("a".into()), Target::Connection("b".into()));
        let url = "https://www.example.com/";
        assert_eq!(state.route_of(&config, url, false), Target::Direct, "site rule");
        assert_eq!(state.route_of(&config, "https://other.org/", true), a);

        // On: every tab, private ones too, and the site rule is set aside.
        config.network.session_route = Some(b.clone());
        state.apply_config(&config);
        assert_eq!(state.route_of(&config, url, false), b);
        assert_eq!(state.route_of(&config, "https://other.org/", false), b);
        assert_eq!(state.route_of(&config, "https://other.org/", true), b);
        let chip = state.chip_for(&config, "https://other.org/", false).expect("chip");
        assert!(chip.session && chip.target == b && chip.name == "Proxy b");

        // The settings were not touched, so switching off brings the old routes back.
        assert_eq!(config.network.default_connection, a);
        assert_eq!(config.network.site_rules.len(), 1);
        config.network.session_route = None;
        state.apply_config(&config);
        assert_eq!(state.route_of(&config, url, false), Target::Direct);
        assert_eq!(state.route_of(&config, "https://other.org/", false), a);
        let chip = state.chip_for(&config, "https://other.org/", false).expect("chip");
        assert!(!chip.session);
    }

    #[test]
    fn a_direct_page_still_gets_a_chip_when_connections_exist() {
        let mut config = Config::default();
        let state = state_with(&config);
        assert!(state.chip_for(&config, "https://a.org/", false).is_none(), "no connection: no chip");
        config.network.connections = vec![proxy("a")];
        let state = state_with(&config);
        let chip = state.chip_for(&config, "https://a.org/", false).expect("a chip to open the popover");
        assert_eq!((chip.state, chip.target.is_direct(), chip.session), (ChipState::Bypassed, true, false));
        assert!(state.chip_for(&config, "axion://home/", false).is_none());
        config.network.show_chip = false;
        assert!(state.chip_for(&config, "https://a.org/", false).is_none());
    }

    #[test]
    fn the_startup_switches_follow_a_persisted_session_route() {
        let mut config = Config::default();
        config.network.connections = vec![proxy("a")];
        assert!(startup_switches(&config.network).is_empty());
        config.network.session_route = Some(Target::Connection("a".into()));
        assert!(!startup_switches(&config.network).is_empty());
    }

    #[test]
    fn blocked_urls_roundtrip() {
        let u = blocked_url("https://a.example/p?x=1&y=é", "c-1", "My VPN & co");
        assert_eq!(parse_blocked_url(&u), Some(("https://a.example/p?x=1&y=é".to_owned(), "c-1".to_owned())));
        assert_eq!(parse_blocked_url("axion://home/"), None);
        assert_eq!(
            parse_blocked_message(r#"{"action":"retry","url":"https://a/","c":"c-1"}"#),
            Some(BlockedAction::Retry { url: "https://a/".into(), connection: "c-1".into() })
        );
        assert_eq!(parse_blocked_message(r#"{"action":"nope"}"#), None);
    }

    #[test]
    fn the_page_is_served_only_for_its_host() {
        let pages = NetworkPages;
        let req = |host, path| InternalRequest { host, path, query: "", private: true, from_own_page: true };
        let page = pages.serve(&req(HOST, "/blocked")).expect("page");
        let html = String::from_utf8(page.body).expect("utf8");
        assert!(html.contains("blocked.js") && !html.contains("<script>"), "no inline script (CSP)");
        assert!(pages.serve(&req("home", "/blocked")).is_none());
        assert!(pages.serve(&req(HOST, "/other")).is_none());
    }

    #[test]
    fn switches_only_with_a_route() {
        let mut n = Network::default();
        assert!(startup_switches(&n).is_empty());
        n.connections.push(Connection {
            id: "c-1".into(),
            name: "x".into(),
            kind: ConnectionKind::Proxy(axion_net::ProxyConfig {
                host: "p.example".into(),
                port: 1,
                ..Default::default()
            }),
        });
        n.default_connection = Target::Connection("c-1".into());
        let s = startup_switches(&n);
        assert!(s.iter().any(|(k, _)| k == "disable-quic"));
        assert!(!s.iter().any(|(k, _)| k == "host-resolver-rules"), "strict DNS is opt-in");
        n.strict_dns = true;
        n.private_connection = Some(Target::Connection("c-1".into()));
        let s = startup_switches(&n);
        let rules = s
            .iter()
            .find(|(k, _)| k == "host-resolver-rules")
            .and_then(|(_, v)| v.clone())
            .unwrap_or_default();
        assert!(rules.contains("EXCLUDE p.example"), "{rules}");
        n.leak_protection = false;
        assert!(startup_switches(&n).is_empty());
    }
}
