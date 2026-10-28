//! Chrome extensions in the browser: the installed list (the store's rows plus each manifest),
//! installing from a folder or a `.crx` / `.zip` in the background, switching on and off and
//! removing. Chromium loads extensions at start (`--load-extension`, see `axion-engine`), so
//! every change takes effect after a restart; [`Extensions::needs_restart`] says when one is
//! pending. The popover is in `extensions_popover.rs`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axion_extensions::{Manifest, Permissions};
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

/// What happened in the background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Installed (the name); it runs after the next start.
    Installed(String),
    /// Could not install (why).
    Failed(String),
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
}

#[derive(Default)]
struct Jobs {
    picked: Option<(Pick, PathBuf)>,
    finished: Option<Result<String, String>>,
}

pub struct Extensions {
    root: PathBuf,
    store_path: Option<PathBuf>,
    jobs: Arc<Mutex<Jobs>>,
    views: Vec<ExtensionView>,
    /// The `(id, folder)` pairs Chromium was started with.
    started_with: Vec<(String, PathBuf)>,
    installing: bool,
}

impl Extensions {
    /// Reads the installed extensions. `AXION_INSTALL_EXTENSION=<folder or .crx>` installs one
    /// first (a test aid, so a run can start with an extension already loaded).
    pub fn new(profile: &Path, store: Option<&Store>) -> Self {
        let mut this = Self {
            root: profile.join("extensions"),
            store_path: store.map(|_| profile.join("axion.sqlite")),
            jobs: Arc::default(),
            views: Vec::new(),
            started_with: Vec::new(),
            installing: false,
        };
        if let (Some(store), Ok(path)) = (store, std::env::var("AXION_INSTALL_EXTENSION")) {
            match install(store, &this.root, Path::new(&path)) {
                Ok(name) => tracing::info!(%name, "extension installed from AXION_INSTALL_EXTENSION"),
                Err(e) => tracing::warn!("AXION_INSTALL_EXTENSION: {e}"),
            }
        }
        this.refresh(store);
        this.started_with = this.enabled_pairs();
        this
    }

    fn enabled_pairs(&self) -> Vec<(String, PathBuf)> {
        let mut pairs: Vec<_> =
            self.views.iter().filter(|v| v.enabled && !v.broken).map(|v| (v.id.clone(), v.dir.clone())).collect();
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
        self.installing
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
        spawn("install-extension", move || {
            let result = Store::open(&store_path).map_err(|e| e.to_string()).and_then(|store| install(&store, &root, &path));
            jobs.lock().unwrap_or_else(PoisonError::into_inner).finished = Some(result);
        });
    }

    /// Results of the file chooser and the installs since the last call.
    pub fn poll(&mut self, store: Option<&Store>) -> Vec<Event> {
        let (picked, finished) = {
            let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
            (jobs.picked.take(), jobs.finished.take())
        };
        let mut events = Vec::new();
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
    let manifest = manifest.ok();
    ExtensionView {
        id: row.id.clone(),
        name: manifest.as_ref().map_or_else(|| row.id.clone(), |m| m.name.clone()),
        version: manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        description: manifest.as_ref().map(|m| m.description.clone()).unwrap_or_default(),
        enabled: row.enabled,
        permissions: manifest.as_ref().map(|m| m.permissions.clone()).unwrap_or_default(),
        popup: manifest.as_ref().and_then(|m| m.popup.clone()),
        options_page: manifest.as_ref().and_then(|m| m.options_page.clone()),
        icon: manifest.as_ref().and_then(|m| m.icon.as_ref()).map(|i| dir.join(i)),
        dir,
        broken,
    }
}

/// Installs `path` (a folder, or a package file) below `root` and records it. An extension
/// installed from the same place before is replaced. Returns its name.
fn install(store: &Store, root: &Path, path: &Path) -> Result<String, String> {
    let installed = if path.is_dir() {
        axion_extensions::install_folder(path, root)
    } else {
        axion_extensions::install_file(path, root)
    }
    .map_err(|e| e.to_string())?;
    let source = path.to_string_lossy().into_owned();
    let existing = store.extensions().map_err(|e| e.to_string())?;
    let mut enabled = true;
    for old in existing.iter().filter(|r| r.id == installed.id || r.source == source) {
        enabled = old.enabled;
        if PathBuf::from(&old.dir) != installed.dir {
            if let Err(e) = axion_extensions::remove_folder(Path::new(&old.dir), root) {
                tracing::warn!("removing the old copy failed: {e}");
            }
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
    Ok(installed.manifest.name)
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
            format!(r#"{{"manifest_version":3,"name":"{name}","version":"1","host_permissions":["<all_urls>"]}}"#),
        )
        .unwrap();
    }

    #[test]
    fn installing_lists_toggles_and_removes() {
        let source = tempfile::tempdir().unwrap();
        write_extension(source.path(), "One");
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(&profile.path().join("axion.sqlite")).unwrap();
        let mut ext = Extensions::new(profile.path(), Some(&store));
        assert!(ext.views().is_empty());
        assert!(!ext.needs_restart());

        assert_eq!(install(&store, &ext.root, source.path()).unwrap(), "One");
        ext.refresh(Some(&store));
        assert_eq!(ext.views().len(), 1);
        assert!(ext.views()[0].permissions.broad());
        assert!(ext.needs_restart(), "a new extension runs after the next start");
        assert_eq!(ext.load_dirs().len(), 1);

        // Installing from the same folder again replaces the copy.
        write_extension(source.path(), "One v2");
        install(&store, &ext.root, source.path()).unwrap();
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
        install(&store, &profile.path().join("extensions"), source.path()).unwrap();
        let mut ext = Extensions::new(profile.path(), Some(&store));
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
        let ext = Extensions::new(profile.path(), Some(&store));
        assert!(ext.views()[0].broken);
        assert!(ext.load_dirs().is_empty());
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
