//! Chrome extensions in the browser: the installed list (the store's rows plus each manifest),
//! installing from a folder or a `.crx` / `.zip` in the background, installing from the Chrome
//! Web Store (download, signature check and unpack in the background, then a confirmation with
//! the permissions), background update checks for those, switching on and off and removing. Chromium loads extensions at start (`--load-extension`, see `axion-engine`), so
//! every change takes effect after a restart; [`Extensions::needs_restart`] says when one is
//! pending. The popover is in `extensions_popover.rs`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axion_config::Extensions as ExtensionsConfig;
use axion_extensions::{ExtensionError, Installed, Manifest, Permissions, StoreClient};
use axion_i18n::{tr, tr_n};
use axion_store::{ExtensionRow, Store};

use crate::downloads::{Choose, choose_portal, spawn};

/// What the user picks to install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// An unpacked extension: a folder with a `manifest.json`.
    Folder,
    /// A `.crx` or `.zip` package.
    Package,
}

/// The `source` of an extension installed from the Chrome Web Store (followed by its id).
const STORE_SOURCE: &str = "webstore:";
/// Meta key: unix seconds of the last update check.
const LAST_CHECK_KEY: &str = "extensions.update_checked";

/// What happened in the background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Installed (the name); it runs after the next start.
    Installed(String),
    /// Could not install (why).
    Failed(String),
    /// A Web Store extension was downloaded and checked: [`Extensions::pending`] waits for the
    /// user's yes.
    StoreReady,
    /// Web Store extensions were updated in the background (their names); they run in the new
    /// version after the next start.
    Updated(Vec<String>),
}

/// A Web Store install (or an update that asks for more permissions than before) that waits
/// for the user's yes. Its files are unpacked already, but nothing is recorded yet.
pub struct PendingInstall {
    pub view: ExtensionView,
    installed: Installed,
    /// The version installed now, if this replaces an extension.
    pub replaces: Option<String>,
    /// An update, not a first install.
    pub update: bool,
}

impl PendingInstall {
    /// Permissions Axion switched off in the copy (they would stall page loads).
    pub fn unsupported(&self) -> &[String] {
        &self.installed.stripped
    }
}

/// One installed extension as the popover shows it.
#[derive(Clone, Debug)]
pub struct ExtensionView {
    pub id: String,
    pub dir: PathBuf,
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub permissions: Permissions,
    /// Relative path of the popup page, if the toolbar button has one.
    pub popup: Option<String>,
    pub options_page: Option<String>,
    pub icon: Option<PathBuf>,
    /// The folder or its manifest is gone: it can only be removed.
    pub broken: bool,
    /// Installed from the Chrome Web Store (so it is updated from there).
    pub store: bool,
    /// Chromium was started with it: its pages work (a change needs a restart).
    pub running: bool,
}

impl ExtensionView {
    /// Whether its popup can be opened now.
    pub fn has_popup(&self) -> bool {
        self.popup.is_some() && self.enabled && self.running && !self.broken
    }
}

#[derive(Default)]
struct Jobs {
    picked: Option<(Pick, PathBuf)>,
    finished: Option<Result<String, String>>,
    /// A Web Store package, downloaded, verified and unpacked.
    staged: Option<Result<Installed, String>>,
    updated: Option<Updated>,
}

/// The result of a background update check.
#[derive(Default)]
struct Updated {
    /// Names of the extensions updated.
    applied: Vec<String>,
    /// Updates that ask for more permissions: unpacked, waiting for approval.
    approval: Vec<Installed>,
}

pub struct Extensions {
    root: PathBuf,
    store_path: Option<PathBuf>,
    jobs: Arc<Mutex<Jobs>>,
    views: Vec<ExtensionView>,
    /// The `(id, folder)` pairs Chromium was started with.
    started_with: Vec<(String, PathBuf)>,
    installing: bool,
    downloading: bool,
    checking: bool,
    /// `extensions.block_request_apis`, for installs that start later.
    block_request_apis: bool,
    /// Unix seconds of the last update check.
    last_check: Option<i64>,
    pending: Option<PendingInstall>,
}

impl Extensions {
    /// Reads the installed extensions. `AXION_INSTALL_EXTENSION=<folder or .crx>` installs one
    /// first (a test aid, so a run can start with an extension already loaded).
    pub fn new(profile: &Path, store: Option<&Store>, config: &ExtensionsConfig) -> Self {
        let mut this = Self {
            root: profile.join("extensions"),
            store_path: store.map(|_| profile.join("axion.sqlite")),
            jobs: Arc::default(),
            views: Vec::new(),
            started_with: Vec::new(),
            installing: false,
            downloading: false,
            checking: false,
            block_request_apis: config.block_request_apis,
            last_check: store
                .and_then(|s| s.meta(LAST_CHECK_KEY).ok().flatten())
                .and_then(|v| v.parse().ok()),
            pending: None,
        };
        if let (Some(store), Ok(path)) = (store, std::env::var("AXION_INSTALL_EXTENSION")) {
            match install(store, &this.root, Path::new(&path), config.block_request_apis) {
                Ok(name) => tracing::info!(%name, "extension installed from AXION_INSTALL_EXTENSION"),
                Err(e) => tracing::warn!("AXION_INSTALL_EXTENSION: {e}"),
            }
        }
        this.refresh(store);
        this.sweep();
        if config.enabled && config.block_request_apis {
            this.block_request_apis();
        }
        if config.enabled && config.allow_user_scripts {
            this.allow_user_scripts(profile);
        }
        this.started_with = this.enabled_pairs();
        this.mark_running();
        this
    }

    /// Marks the extensions Chromium was started with.
    fn mark_running(&mut self) {
        let started = &self.started_with;
        for view in &mut self.views {
            view.running = started.iter().any(|(id, _)| *id == view.id);
        }
    }

    /// Switches the request APIs off in every installed extension (see `axion_extensions::compat`),
    /// including the ones installed before Axion did that.
    fn block_request_apis(&mut self) {
        let mut changed = false;
        for view in self.views.iter().filter(|v| !v.broken) {
            match axion_extensions::strip_request_apis(&view.dir) {
                Ok(removed) => changed |= !removed.is_empty(),
                Err(e) => tracing::warn!(id = %view.id, "switching off the request APIs failed: {e}"),
            }
        }
        if changed {
            self.views.iter_mut().for_each(|v| {
                if let Ok(manifest) = Manifest::read(&v.dir) {
                    v.permissions = manifest.permissions;
                }
            });
        }
    }

    /// Switches Chromium's "Allow User Scripts" on for the extensions that ask for the
    /// `userScripts` permission (Tampermonkey …): without it `chrome.userScripts` does not
    /// exist for them. Written into the profile's preferences, so before Chromium starts.
    fn allow_user_scripts(&self, profile: &Path) {
        let ids: Vec<String> = self
            .views
            .iter()
            .filter(|v| v.enabled && !v.broken)
            .filter(|v| v.permissions.apis.iter().chain(&v.permissions.optional).any(|p| p == "userScripts"))
            .map(|v| v.id.clone())
            .collect();
        if let Err(e) =
            axion_extensions::allow_user_scripts(&profile.join("Default").join("Preferences"), &ids)
        {
            tracing::warn!("allowing user scripts failed: {e}");
        }
    }

    /// Deletes the folders below the extension root that no extension uses: unpacked packages
    /// that were never confirmed, and the old copy of an updated extension (it stayed while
    /// Chromium still ran it). Only at start, when nothing uses them.
    fn sweep(&self) {
        let Ok(entries) = std::fs::read_dir(&self.root) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let ours =
                name.len() == 13 && name.starts_with('x') && name[1..].bytes().all(|b| b.is_ascii_hexdigit());
            if ours && path.is_dir() && !self.views.iter().any(|v| v.dir == path) {
                tracing::info!(dir = %path.display(), "removing an unused extension folder");
                if let Err(e) = axion_extensions::remove_folder(&path, &self.root) {
                    tracing::warn!("removing it failed: {e}");
                }
            }
        }
    }

    fn enabled_pairs(&self) -> Vec<(String, PathBuf)> {
        let mut pairs: Vec<_> = self
            .views
            .iter()
            .filter(|v| v.enabled && !v.broken)
            .map(|v| (v.id.clone(), v.dir.clone()))
            .collect();
        pairs.sort();
        pairs
    }

    /// The folders Chromium should load.
    pub fn load_dirs(&self) -> Vec<PathBuf> {
        self.enabled_pairs().into_iter().map(|(_, dir)| dir).collect()
    }

    /// Reads the rows and manifests again.
    pub fn refresh(&mut self, store: Option<&Store>) {
        let rows = store.map(|s| s.extensions()).transpose().unwrap_or_else(|e| {
            tracing::warn!("reading the extensions failed: {e}");
            None
        });
        self.views = rows.unwrap_or_default().iter().map(view_of).collect();
        self.mark_running();
    }

    pub fn views(&self) -> &[ExtensionView] {
        &self.views
    }

    pub fn get(&self, id: &str) -> Option<&ExtensionView> {
        self.views.iter().find(|v| v.id == id)
    }

    /// Whether the extensions Chromium runs differ from the ones switched on now.
    pub fn needs_restart(&self) -> bool {
        self.enabled_pairs() != self.started_with
    }

    pub fn is_installing(&self) -> bool {
        self.installing || self.downloading
    }

    pub fn is_checking(&self) -> bool {
        self.checking
    }

    /// The ids of all installed extensions.
    pub fn ids(&self) -> Vec<String> {
        self.views.iter().map(|v| v.id.clone()).collect()
    }

    /// Whether any extension comes from the Web Store (so there is something to update).
    pub fn has_store_extensions(&self) -> bool {
        self.views.iter().any(|v| v.store && !v.broken)
    }

    /// The Web Store install waiting for the user's yes.
    pub fn pending(&self) -> Option<&PendingInstall> {
        self.pending.as_ref()
    }

    /// Starts installing extension `id` from the Chrome Web Store: download, signature check
    /// and unpacking in the background; [`Event::StoreReady`] follows. `Err` is a message for
    /// the user.
    pub fn request_store_install(
        &mut self,
        id: &str,
        cef_version: &str,
        config: &ExtensionsConfig,
    ) -> Result<(), String> {
        if !config.enabled || !config.web_store {
            return Err(tr("Installing from the Chrome Web Store is switched off in Settings.").to_owned());
        }
        if !axion_extensions::is_extension_id(id) {
            return Err(tr("That is not a valid extension.").to_owned());
        }
        if self.store_path.is_none() {
            return Err(tr("Extensions cannot be installed without a profile.").to_owned());
        }
        if self.is_installing() {
            return Err(tr("Another extension is being installed.").to_owned());
        }
        self.cancel_pending();
        self.downloading = true;
        let (jobs, root, id) = (Arc::clone(&self.jobs), self.root.clone(), id.to_owned());
        let client = client(config);
        let block = config.block_request_apis;
        let product = axion_extensions::product_version(cef_version);
        spawn("download-extension", move || {
            let result = client
                .download(&id, &product)
                .and_then(|data| axion_extensions::install_store_package(&data, &id, &root))
                .and_then(|installed| prepare(installed, block))
                .map_err(|e| explain(&e));
            jobs.lock().unwrap_or_else(PoisonError::into_inner).staged = Some(result);
        });
        Ok(())
    }

    /// Unpacked and checked: waits for the user's yes.
    fn stage(&mut self, installed: Installed, update: bool) {
        self.cancel_pending();
        let replaces = self.get(&installed.id).map(|v| v.version.clone());
        let view = view_from(&installed.id, installed.dir.clone(), true, Some(&installed.manifest));
        self.pending = Some(PendingInstall { view, installed, replaces, update });
    }

    /// The user said yes: records the pending extension. It runs after the next start.
    pub fn confirm_pending(&mut self, store: Option<&Store>) -> Option<Event> {
        let pending = self.pending.take()?;
        let Some(store) = store else {
            let _ = axion_extensions::remove_folder(&pending.installed.dir, &self.root);
            return None;
        };
        let source = format!("{STORE_SOURCE}{}", pending.installed.id);
        let result = record(store, &self.root, &pending.installed, source, true);
        if result.is_err() {
            let _ = axion_extensions::remove_folder(&pending.installed.dir, &self.root);
        }
        self.refresh(Some(store));
        Some(match result {
            Ok(name) => Event::Installed(name),
            Err(why) => Event::Failed(why),
        })
    }

    /// The user said no: the unpacked files go.
    pub fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.take()
            && let Err(e) = axion_extensions::remove_folder(&pending.installed.dir, &self.root)
        {
            tracing::warn!("removing the unconfirmed extension failed: {e}");
        }
    }

    /// Starts the background update check when it is due (`extensions.auto_update`, every
    /// `extensions.update_interval_hours`).
    pub fn tick_updates(
        &mut self,
        store: Option<&Store>,
        config: &ExtensionsConfig,
        cef_version: &str,
        now: i64,
    ) {
        let interval = i64::from(config.update_interval_hours.max(1)) * 3600;
        let due = self.last_check.is_none_or(|t| now - t >= interval || t > now);
        if config.enabled && config.auto_update && due {
            self.check_updates(store, config, cef_version, now);
        }
    }

    /// Checks the Web Store extensions for newer versions now. Returns whether a check started.
    pub fn check_updates(
        &mut self,
        store: Option<&Store>,
        config: &ExtensionsConfig,
        cef_version: &str,
        now: i64,
    ) -> bool {
        let Some(store_path) = self.store_path.clone() else { return false };
        if self.checking || !config.enabled {
            return false;
        }
        self.last_check = Some(now);
        if let Some(store) = store
            && let Err(e) = store.set_meta(LAST_CHECK_KEY, Some(&now.to_string()))
        {
            tracing::warn!("remembering the update check failed: {e}");
        }
        let installed: Vec<(String, String)> = self
            .views
            .iter()
            .filter(|v| v.store && !v.broken && !v.version.is_empty())
            .map(|v| (v.id.clone(), v.version.clone()))
            .collect();
        if installed.is_empty() {
            return false;
        }
        self.checking = true;
        let (jobs, root) = (Arc::clone(&self.jobs), self.root.clone());
        let client = client(config);
        let block = config.block_request_apis;
        let product = axion_extensions::product_version(cef_version);
        spawn("update-extensions", move || {
            let outcome = run_update_check(&client, &installed, &product, &store_path, &root, block);
            jobs.lock().unwrap_or_else(PoisonError::into_inner).updated = Some(outcome);
        });
        true
    }

    /// Switches an extension on or off (after the next start).
    pub fn set_enabled(&mut self, id: &str, enabled: bool, store: Option<&Store>) {
        let Some(store) = store else { return };
        if let Err(e) = store.set_extension_enabled(id, enabled) {
            tracing::warn!("switching the extension failed: {e}");
        }
        self.refresh(Some(store));
    }

    /// Removes an extension and its files (it stops after the next start).
    pub fn remove(&mut self, id: &str, store: Option<&Store>) {
        let Some(store) = store else { return };
        if let Some(view) = self.get(id)
            && let Err(e) = axion_extensions::remove_folder(&view.dir, &self.root)
        {
            tracing::warn!("removing the extension's files failed: {e}");
        }
        if let Err(e) = store.remove_extension(id) {
            tracing::warn!("removing the extension failed: {e}");
        }
        self.refresh(Some(store));
    }

    /// Asks for the folder or package with the desktop's file chooser; the install follows
    /// in [`Self::poll`].
    pub fn choose(&self, pick: Pick) {
        let jobs = Arc::clone(&self.jobs);
        let (title, what) = match pick {
            Pick::Folder => (tr("Extension Folder"), Choose::Folder),
            Pick::Package => (tr("Extension Package"), Choose::ExtensionPackage),
        };
        spawn("choose-extension", move || match choose_portal(title, what) {
            Ok(Some(path)) => jobs.lock().unwrap_or_else(PoisonError::into_inner).picked = Some((pick, path)),
            Ok(None) => {}
            Err(e) => tracing::warn!("file chooser unavailable: {e}"),
        });
    }

    /// Installs `path` (a folder or package) in the background.
    pub fn install_path(&mut self, path: PathBuf) {
        let Some(store_path) = self.store_path.clone() else { return };
        if self.installing {
            return;
        }
        self.installing = true;
        let (jobs, root) = (Arc::clone(&self.jobs), self.root.clone());
        let block = self.block_request_apis;
        spawn("install-extension", move || {
            let result = Store::open(&store_path)
                .map_err(|e| e.to_string())
                .and_then(|store| install(&store, &root, &path, block));
            jobs.lock().unwrap_or_else(PoisonError::into_inner).finished = Some(result);
        });
    }

    /// Results of the file chooser and the installs since the last call.
    pub fn poll(&mut self, store: Option<&Store>) -> Vec<Event> {
        let (picked, finished, staged, updated) = {
            let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
            (jobs.picked.take(), jobs.finished.take(), jobs.staged.take(), jobs.updated.take())
        };
        let mut events = Vec::new();
        if let Some(result) = staged {
            self.downloading = false;
            events.push(match result {
                Ok(installed) => {
                    self.stage(installed, false);
                    Event::StoreReady
                }
                Err(why) => Event::Failed(why),
            });
        }
        if let Some(updated) = updated {
            self.checking = false;
            self.refresh(store);
            if !updated.applied.is_empty() {
                events.push(Event::Updated(updated.applied));
            }
            for installed in updated.approval {
                if self.pending.is_none() {
                    self.stage(installed, true);
                    events.push(Event::StoreReady);
                } else {
                    // The next check finds it again.
                    let _ = axion_extensions::remove_folder(&installed.dir, &self.root);
                }
            }
        }
        if let Some((_, path)) = picked {
            self.install_path(path);
        }
        if let Some(result) = finished {
            self.installing = false;
            self.refresh(store);
            events.push(match result {
                Ok(name) => Event::Installed(name),
                Err(why) => Event::Failed(why),
            });
        }
        events
    }
}

fn view_of(row: &ExtensionRow) -> ExtensionView {
    let dir = PathBuf::from(&row.dir);
    let manifest = Manifest::read(&dir);
    let broken = manifest.is_err();
    let mut view = view_from(&row.id, dir, row.enabled, manifest.ok().as_ref());
    view.broken = broken;
    view.store = row.source.starts_with(STORE_SOURCE);
    view
}

fn view_from(id: &str, dir: PathBuf, enabled: bool, manifest: Option<&Manifest>) -> ExtensionView {
    ExtensionView {
        id: id.to_owned(),
        name: manifest.map_or_else(|| id.to_owned(), |m| m.name.clone()),
        version: manifest.map(|m| m.version.clone()).unwrap_or_default(),
        description: manifest.map(|m| m.description.clone()).unwrap_or_default(),
        enabled,
        permissions: manifest.map(|m| m.permissions.clone()).unwrap_or_default(),
        popup: manifest.and_then(|m| m.popup.clone()),
        options_page: manifest.and_then(|m| m.options_page.clone()),
        icon: manifest.and_then(|m| m.icon.as_ref()).map(|i| dir.join(i)),
        dir,
        broken: false,
        store: false,
        running: false,
    }
}

fn client(config: &ExtensionsConfig) -> StoreClient {
    StoreClient::new(
        Duration::from_secs(u64::from(config.timeout_seconds.max(5))),
        concat!("Axion/", env!("CARGO_PKG_VERSION")),
    )
}

/// Asks for newer versions of `installed` and applies them: an update that asks for nothing
/// new is recorded at once (it runs after the next start); one that asks for more is
/// unpacked and left for the user to approve.
fn run_update_check(
    client: &StoreClient,
    installed: &[(String, String)],
    product: &str,
    store_path: &Path,
    root: &Path,
    block: bool,
) -> Updated {
    let mut out = Updated::default();
    let updates = match client.check_updates(installed, product) {
        Ok(updates) => updates,
        Err(e) => {
            tracing::warn!("checking the extensions for updates failed: {e}");
            return out;
        }
    };
    let store = match Store::open(store_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!("opening the store for extension updates failed: {e}");
            return out;
        }
    };
    for update in updates {
        let package = client
            .download_update(&update)
            .and_then(|data| axion_extensions::install_store_package(&data, &update.id, root))
            .and_then(|installed| prepare(installed, block));
        let package = match package {
            Ok(package) => package,
            Err(e) => {
                tracing::warn!(id = %update.id, "updating the extension failed: {e}");
                continue;
            }
        };
        let old = store
            .extensions()
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r.id == update.id))
            .and_then(|row| Manifest::read(Path::new(&row.dir)).ok());
        let more = old.is_none_or(|old| package.manifest.permissions.grows_over(&old.permissions));
        if more {
            tracing::info!(id = %update.id, "the update asks for more permissions: waiting for approval");
            out.approval.push(package);
            continue;
        }
        let source = format!("{STORE_SOURCE}{}", update.id);
        match record(&store, root, &package, source, true) {
            Ok(name) => out.applied.push(name),
            Err(e) => {
                tracing::warn!(id = %update.id, "recording the update failed: {e}");
                let _ = axion_extensions::remove_folder(&package.dir, root);
            }
        }
    }
    out
}

/// Installs `path` (a folder, or a package file) below `root` and records it. An extension
/// installed from the same place before is replaced. Returns its name.
fn install(store: &Store, root: &Path, path: &Path, block: bool) -> Result<String, String> {
    let installed = if path.is_dir() {
        axion_extensions::install_folder(path, root)
    } else {
        axion_extensions::install_file(path, root)
    }
    .and_then(|installed| prepare(installed, block))
    .map_err(|e| explain(&e))?;
    record(store, root, &installed, path.to_string_lossy().into_owned(), false)
}

/// Records an unpacked extension as installed (replacing the one with its id or its source,
/// which stays switched on or off). The old copy's folder goes, unless `keep_old`: Chromium
/// still runs it until the next start, and [`Extensions::new`] clears it then. Returns the name.
fn record(
    store: &Store,
    root: &Path,
    installed: &Installed,
    source: String,
    keep_old: bool,
) -> Result<String, String> {
    let existing = store.extensions().map_err(|e| e.to_string())?;
    let mut enabled = true;
    for old in existing.iter().filter(|r| r.id == installed.id || r.source == source) {
        enabled = old.enabled;
        if !keep_old
            && Path::new(&old.dir) != installed.dir
            && let Err(e) = axion_extensions::remove_folder(Path::new(&old.dir), root)
        {
            tracing::warn!("removing the old copy failed: {e}");
        }
        if old.id != installed.id {
            store.remove_extension(&old.id).map_err(|e| e.to_string())?;
        }
    }
    store
        .upsert_extension(&ExtensionRow {
            id: installed.id.clone(),
            dir: installed.dir.to_string_lossy().into_owned(),
            enabled,
            source,
            installed: crate::browser::unix_now(),
        })
        .map_err(|e| e.to_string())?;
    tracing::info!(id = %installed.id, name = %installed.manifest.name, "extension installed");
    Ok(installed.manifest.name.clone())
}

/// Switches the request APIs off in a fresh install (when `block`); a failure removes it again.
fn prepare(mut installed: Installed, block: bool) -> axion_extensions::Result<Installed> {
    if block && let Err(e) = axion_extensions::disable_request_apis(&mut installed) {
        let _ = std::fs::remove_dir_all(&installed.dir);
        return Err(e);
    }
    Ok(installed)
}

/// What went wrong, for the user.
fn explain(error: &ExtensionError) -> String {
    match error {
        ExtensionError::UnsupportedManifest(version) => axion_i18n::tr_fmt!(
            "It uses Manifest V{version}, which current Chromium no longer runs. Only Manifest V3 extensions work.",
            version = version
        ),
        other => other.to_string(),
    }
}

/// `chrome-extension://<id>/<page>`, where the popup and options pages are opened.
pub fn page_url(id: &str, page: &str) -> String {
    format!("chrome-extension://{id}/{page}")
}

/// What an API permission lets the extension do, in words.
pub fn api_label(permission: &str) -> String {
    let text = match permission {
        "activeTab" => tr("Use the tab you open it on"),
        "tabs" => tr("See the addresses and titles of your tabs"),
        "storage" | "unlimitedStorage" => tr("Store its own data"),
        "cookies" => tr("Read and change cookies"),
        "webRequest" | "webNavigation" => tr("See the pages you visit and load"),
        "declarativeNetRequest" | "declarativeNetRequestWithHostAccess" | "declarativeNetRequestFeedback" => {
            tr("Block or change what pages load")
        }
        "scripting" | "userScripts" => tr("Run scripts in pages"),
        "history" => tr("Read and change your browsing history"),
        "bookmarks" => tr("Read and change your bookmarks"),
        "downloads" => tr("Manage your downloads"),
        "clipboardRead" => tr("Read your clipboard"),
        "clipboardWrite" => tr("Write to your clipboard"),
        "notifications" => tr("Show notifications"),
        "contextMenus" => tr("Add items to the context menu"),
        "nativeMessaging" => tr("Talk to programs on your computer"),
        "geolocation" => tr("Know your location"),
        "management" => tr("Manage other extensions"),
        "proxy" => tr("Control the proxy settings"),
        "privacy" => tr("Change privacy settings"),
        "identity" => tr("Use your signed-in accounts"),
        "sidePanel" => tr("Show a side panel"),
        "idle" => tr("Know when you are away from the computer"),
        "alarms" => tr("Run at set times in the background"),
        "offscreen" => tr("Use hidden pages to do its work"),
        "webRequestBlocking" => tr("Change what pages load before they load"),
        "favicon" => tr("See the icons of sites"),
        "topSites" => tr("See the sites you visit most"),
        "tabGroups" => tr("Manage your tab groups"),
        "windows" => tr("Open and arrange windows"),
        "webRequestAuthProvider" => tr("Answer sign-in requests of sites"),
        "background" => tr("Keep running in the background"),
        "declarativeContent" => tr("Act on pages by their content"),
        "downloads.open" => tr("Open your downloads"),
        "pageCapture" => tr("Save pages as MHTML"),
        "tabCapture" | "desktopCapture" => tr("Record your screen or tabs"),
        "system.cpu" | "system.memory" | "system.storage" | "system.display" => {
            tr("See details of your computer")
        }
        "fontSettings" => tr("Change font settings"),
        "readingList" => tr("Read and change your reading list"),
        other => return other.to_owned(),
    };
    text.to_owned()
}

/// The one line under an extension's name: how much of the web it can reach.
pub fn access_summary(permissions: &Permissions) -> String {
    if permissions.broad() {
        tr("Can read and change all websites").to_owned()
    } else if permissions.hosts.is_empty() {
        tr("No access to websites").to_owned()
    } else {
        tr_n("Can read and change {n} site", "Can read and change {n} sites", permissions.hosts.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_extension(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{"manifest_version":3,"name":"{name}","version":"1","host_permissions":["<all_urls>"]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn installing_lists_toggles_and_removes() {
        let source = tempfile::tempdir().unwrap();
        write_extension(source.path(), "One");
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        let mut ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        assert!(ext.views().is_empty());
        assert!(!ext.needs_restart());

        assert_eq!(install(&store, &ext.root, source.path(), false).unwrap(), "One");
        ext.refresh(Some(&store));
        assert_eq!(ext.views().len(), 1);
        assert!(ext.views()[0].permissions.broad());
        assert!(ext.needs_restart(), "a new extension runs after the next start");
        assert_eq!(ext.load_dirs().len(), 1);

        // Installing from the same folder again replaces the copy.
        write_extension(source.path(), "One v2");
        install(&store, &ext.root, source.path(), false).unwrap();
        ext.refresh(Some(&store));
        assert_eq!(ext.views().len(), 1);
        assert_eq!(ext.views()[0].name, "One v2");
        assert_eq!(std::fs::read_dir(&ext.root).unwrap().count(), 1);

        let id = ext.views()[0].id.clone();
        ext.set_enabled(&id, false, Some(&store));
        assert!(ext.load_dirs().is_empty());
        ext.remove(&id, Some(&store));
        assert!(ext.views().is_empty());
        assert_eq!(std::fs::read_dir(&ext.root).unwrap().count(), 0);
    }

    #[test]
    fn restart_is_needed_only_when_the_running_set_differs() {
        let source = tempfile::tempdir().unwrap();
        write_extension(source.path(), "One");
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        install(&store, &profile.path().join("extensions"), source.path(), false).unwrap();
        let mut ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        assert!(!ext.needs_restart());
        let id = ext.views()[0].id.clone();
        ext.set_enabled(&id, false, Some(&store));
        assert!(ext.needs_restart());
        ext.set_enabled(&id, true, Some(&store));
        assert!(!ext.needs_restart(), "switching back needs nothing");
    }

    #[test]
    fn a_missing_folder_is_broken_not_fatal() {
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        store
            .upsert_extension(&ExtensionRow {
                id: "abc".into(),
                dir: "/nonexistent/ext".into(),
                enabled: true,
                source: String::new(),
                installed: 1,
            })
            .unwrap();
        let ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        assert!(ext.views()[0].broken);
        assert!(ext.load_dirs().is_empty());
    }

    /// A folder with a manifest whose `key` fixes the extension id, like a store package's.
    fn write_keyed(dir: &Path, version: &str, permissions: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{"manifest_version":3,"name":"Keyed","version":"{version}","key":"AAAA","permissions":[{permissions}]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_store_install_waits_for_the_yes() {
        let source = tempfile::tempdir().unwrap();
        write_keyed(source.path(), "1.0", "");
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        let mut ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());

        let staged = axion_extensions::install_folder(source.path(), &ext.root).unwrap();
        let dir = staged.dir.clone();
        ext.stage(staged, false);
        assert!(ext.views().is_empty(), "nothing is recorded before the yes");
        assert!(ext.pending().is_some_and(|p| p.replaces.is_none() && p.view.name == "Keyed"));

        // A no removes the unpacked files.
        ext.cancel_pending();
        assert!(ext.pending().is_none());
        assert!(!dir.exists());

        // A yes records it as a store extension.
        let staged = axion_extensions::install_folder(source.path(), &ext.root).unwrap();
        ext.stage(staged, false);
        assert_eq!(ext.confirm_pending(Some(&store)), Some(Event::Installed("Keyed".into())));
        assert_eq!(ext.views().len(), 1);
        assert!(ext.views()[0].store);
        assert!(ext.needs_restart());
        assert!(ext.has_store_extensions());
        assert!(ext.confirm_pending(Some(&store)).is_none(), "answered once");
    }

    #[test]
    fn an_update_keeps_the_running_copy_until_the_next_start() {
        let source = tempfile::tempdir().unwrap();
        write_keyed(source.path(), "1.0", "");
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        let mut ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        let first = axion_extensions::install_folder(source.path(), &ext.root).unwrap();
        ext.stage(first, false);
        ext.confirm_pending(Some(&store));
        let old_dir = ext.views()[0].dir.clone();
        let id = ext.views()[0].id.clone();
        ext.set_enabled(&id, false, Some(&store));

        write_keyed(source.path(), "2.0", "");
        let second = axion_extensions::install_folder(source.path(), &ext.root).unwrap();
        ext.stage(second, true);
        assert_eq!(ext.pending().and_then(|p| p.replaces.clone()).as_deref(), Some("1.0"));
        ext.confirm_pending(Some(&store));
        assert_eq!(ext.views().len(), 1);
        assert_eq!(ext.views()[0].version, "2.0");
        assert!(!ext.views()[0].enabled, "an update keeps the switch");
        assert!(old_dir.exists(), "Chromium still runs the old copy");

        // The next start clears what nothing uses any more.
        let again = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        assert!(!old_dir.exists());
        assert!(again.views()[0].dir.exists());
    }

    #[test]
    fn user_scripts_are_allowed_for_extensions_that_ask() {
        let source = tempfile::tempdir().unwrap();
        write_keyed(source.path(), "1.0", r#""userScripts","storage""#);
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        install(&store, &profile.path().join("extensions"), source.path(), false).unwrap();
        let plain = tempfile::tempdir().unwrap();
        write_extension(plain.path(), "Plain");
        install(&store, &profile.path().join("extensions"), plain.path(), false).unwrap();

        let off = ExtensionsConfig { allow_user_scripts: false, ..ExtensionsConfig::default() };
        Extensions::new(profile.path(), Some(&store), &off);
        assert!(!profile.path().join("Default/Preferences").exists());

        let ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        let text = std::fs::read_to_string(profile.path().join("Default/Preferences")).unwrap();
        let prefs: serde_json::Value = serde_json::from_str(&text).unwrap();
        let settings = prefs["extensions"]["settings"].as_object().unwrap();
        assert_eq!(settings.len(), 1, "only the extension that asked");
        let id = ext.views().iter().find(|v| v.name == "Keyed").unwrap().id.clone();
        assert_eq!(settings[&id]["user_scripts_enabled"], true);
    }

    #[test]
    fn the_request_apis_are_switched_off_at_start() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join("manifest.json"),
            r#"{"manifest_version":3,"name":"Blocker","version":"1","permissions":["storage","webRequest","declarativeNetRequest"],"declarative_net_request":{"rule_resources":[]}}"#,
        )
        .unwrap();
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        // Installed the way an older Axion did: nothing removed.
        install(&store, &profile.path().join("extensions"), source.path(), false).unwrap();

        let off = ExtensionsConfig { block_request_apis: false, ..ExtensionsConfig::default() };
        let ext = Extensions::new(profile.path(), Some(&store), &off);
        assert!(ext.views()[0].permissions.apis.contains(&"webRequest".to_owned()));

        let ext = Extensions::new(profile.path(), Some(&store), &ExtensionsConfig::default());
        assert_eq!(ext.views()[0].permissions.apis, ["storage"]);
        let manifest = std::fs::read_to_string(ext.views()[0].dir.join("manifest.json")).unwrap();
        assert!(!manifest.contains("webRequest") && !manifest.contains("declarative_net_request"));

        // A new install is cleaned before the user even sees it.
        let installed =
            axion_extensions::install_folder(source.path(), &profile.path().join("extensions")).unwrap();
        let installed = prepare(installed, true).unwrap();
        assert_eq!(installed.stripped, ["webRequest", "declarativeNetRequest"]);
        assert_eq!(installed.manifest.permissions.apis, ["storage"]);
    }

    #[test]
    fn labels() {
        assert_eq!(api_label("tabs"), "See the addresses and titles of your tabs");
        assert_eq!(api_label("somethingNew"), "somethingNew");
        let none = Permissions::default();
        assert_eq!(access_summary(&none), "No access to websites");
        let two = Permissions { hosts: vec!["https://a/*".into(), "https://b/*".into()], ..none.clone() };
        assert_eq!(access_summary(&two), "Can read and change 2 sites");
        let all = Permissions { hosts: vec!["<all_urls>".into()], ..none };
        assert_eq!(access_summary(&all), "Can read and change all websites");
    }
}
