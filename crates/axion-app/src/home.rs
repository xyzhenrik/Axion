//! Axion's start page, `axion://home/`: the homepage and new tab page.
//!
//! The page is served by Axion itself through the engine's `axion://` scheme handler (see
//! `axion_engine::internal` for the security model): its HTML, CSS and script are embedded
//! here, and its data — top sites, Shields numbers, search engines, which widgets show — is a
//! JSON snapshot that [`HomePage::update`] keeps ready for Chromium's network thread
//! ([`HomeService`]). The page pulls it from `/api/state` and posts what the user does to
//! `/api/message` (only its own top-level document can; the engine checks), which arrives as
//! [`axion_engine::EngineEvent::InternalMessage`] and goes through [`HomePage::handle`].
//! When the data changes, open start pages are asked to pull again with a script that carries
//! no data ([`REFRESH_SCRIPT`]).
//!
//! Private tabs get a private variant: no top sites, history or Shields numbers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use axion_config::{ClockFormat, Config, Home, START_PAGE_URL};
use axion_engine::internal::split_internal_url;
use axion_engine::{InternalPages, InternalRequest, InternalResponse};
use axion_store::{Store, site_host};
use serde_json::{Value, json};

use crate::downloads::{Choose, choose_portal, spawn};
use crate::settings::SEARCH_ENGINES;
use crate::url::{is_address, resolve_input};

/// The start page's address.
#[allow(dead_code, reason = "the one place other code names the start page by")]
pub const HOME_URL: &str = START_PAGE_URL;
/// What the address bar shows on the start page instead of its address.
pub const ADDRESS_PLACEHOLDER: &str = "Search or enter website";
/// Its host under the `axion://` scheme.
const HOST: &str = "home";

/// Whether `url` is the start page (any path, query or fragment).
pub fn is_home_url(url: &str) -> bool {
    split_internal_url(url).is_some_and(|(host, _, _)| host.eq_ignore_ascii_case(HOST))
}

/// Run in an open start page to make it pull its data again (carries none itself, so it is
/// harmless should the tab have navigated elsewhere meanwhile).
pub const REFRESH_SCRIPT: &str = "window.axionHome && window.axionHome.refresh()";

const PAGE_HTML: &str = include_str!("home/index.html");
const PAGE_CSS: &str = include_str!("home/home.css");
const PAGE_JS: &str = include_str!("home/home.js");

/// Brave-style estimates for the Shields card: an average blocked ad or tracker request
/// would have downloaded about this much …
pub const BYTES_PER_BLOCKED_REQUEST: u64 = 50 * 1024;
/// … and cost about this much time to load (Brave uses 50 ms as well).
pub const SECONDS_PER_BLOCKED_REQUEST: f64 = 0.05;

/// The background image in the account's profile (`<profile>/home/background`).
const BACKGROUND_FILE: &str = "background";
/// Larger images are refused as backgrounds.
const MAX_BACKGROUND_BYTES: u64 = 40 * 1024 * 1024;

/// What the network thread serves, kept ready by [`HomePage::update`].
#[derive(Default)]
struct Served {
    /// `/api/state` for normal and private tabs.
    state: String,
    private_state: String,
    /// The background image and its MIME type, while one is set.
    background: Option<(PathBuf, String)>,
    favicons: Option<PathBuf>,
    dark: bool,
}

/// Serves the start page (on Chromium's network thread).
#[derive(Default)]
pub struct HomeService(RwLock<Served>);

impl InternalPages for HomeService {
    fn serve(&self, request: &InternalRequest<'_>) -> Option<InternalResponse> {
        if request.host != HOST {
            return None;
        }
        let served = self.0.read().unwrap_or_else(PoisonError::into_inner);
        Some(match request.path {
            "/" | "/index.html" => InternalResponse::ok("text/html", PAGE_HTML),
            "/home.css" => InternalResponse::ok("text/css", PAGE_CSS),
            "/home.js" => InternalResponse::ok("text/javascript", PAGE_JS),
            "/api/state" => InternalResponse::ok(
                "application/json",
                if request.private { served.private_state.as_str() } else { served.state.as_str() },
            ),
            "/api/background" if !request.private => {
                let (path, mime) = served.background.as_ref()?;
                InternalResponse::ok(mime, std::fs::read(path).ok()?)
            }
            "/api/favicon" => {
                let host = query_value(request.query, "host")?;
                let dir = served.favicons.as_ref()?;
                let files = [
                    crate::favicons::cache_file(dir, &host, served.dark),
                    crate::favicons::cache_file(dir, &host, !served.dark),
                ];
                let bytes = files.iter().find_map(|f| std::fs::read(f).ok())?;
                InternalResponse::ok("image/png", bytes)
            }
            _ => return None,
        })
    }
}

/// The decoded value of `key` in a URL query.
fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a message from the start page asks the browser to do.
#[derive(Clone, Debug, PartialEq)]
pub enum HomeAction {
    Nothing,
    /// Load what was typed into the page's search field (an address or a search).
    Go(String),
    /// Settings changed (already applied to the config): save them, update the UI.
    ConfigChanged,
    /// Top sites or the background changed: open start pages should refresh.
    Refresh,
    /// Open Settings › General.
    OpenSettings,
}

/// Inputs the snapshot was last built from.
#[derive(Clone, PartialEq)]
struct Inputs {
    home: Home,
    search_url: String,
    name: String,
    dark: bool,
}

/// The start page's side in the browser (main thread).
pub struct HomePage {
    service: Arc<HomeService>,
    /// `<profile>/home`.
    dir: PathBuf,
    inputs: Option<Inputs>,
    /// The Shields total last shown, and when.
    blocked_shown: Option<(u64, Instant)>,
    /// Bumped when the background image changes (the page's URL for it changes with it).
    background_version: u64,
    /// A background picked in the desktop's file chooser, waiting to be taken.
    chosen: Arc<Mutex<Option<PathBuf>>>,
    /// The top sites or the background changed: rebuild at the next update.
    stale: bool,
    /// Bumped when site icons arrive (the page's icon URLs change with it).
    icons_version: u64,
}

impl HomePage {
    pub fn new(profile: &Path, favicons: Option<PathBuf>) -> Self {
        let service = Arc::new(HomeService::default());
        service.0.write().unwrap_or_else(PoisonError::into_inner).favicons = favicons;
        Self {
            service,
            dir: profile.join("home"),
            inputs: None,
            blocked_shown: None,
            background_version: 1,
            chosen: Arc::default(),
            stale: true,
            icons_version: 1,
        }
    }

    /// A site icon was cached: open start pages show it at the next update.
    pub fn icons_changed(&mut self) {
        self.icons_version += 1;
        self.stale = true;
    }

    pub fn service(&self) -> Arc<dyn InternalPages> {
        self.service.clone()
    }

    fn background_path(&self) -> PathBuf {
        self.dir.join(BACKGROUND_FILE)
    }

    /// Rebuilds what the page shows when its inputs changed (the Shields numbers at most every
    /// `home.stats_refresh_seconds`). Returns whether open start pages should pull again.
    pub fn update(
        &mut self,
        config: &Config,
        store: Option<&Store>,
        blocked_total: u64,
        account_name: &str,
        dark: bool,
    ) -> bool {
        let inputs = Inputs {
            home: config.home.clone(),
            search_url: config.general.search_url.clone(),
            name: account_name.to_owned(),
            dark,
        };
        let interval = Duration::from_secs_f32(config.home.stats_refresh_seconds.max(0.0));
        let stats_due =
            self.blocked_shown.is_none_or(|(shown, at)| shown != blocked_total && at.elapsed() >= interval);
        if !self.stale && self.inputs.as_ref() == Some(&inputs) && !stats_due {
            return false;
        }
        self.stale = false;
        self.blocked_shown = Some((blocked_total, Instant::now()));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let background = self.background_file();
        let state = state_json(
            self.icons_version,
            config,
            store,
            blocked_total,
            account_name,
            false,
            background.is_some(),
            self.background_version,
            now,
        );
        let private_state =
            state_json(self.icons_version, config, None, 0, account_name, true, false, 0, now);
        let mut served = self.service.0.write().unwrap_or_else(PoisonError::into_inner);
        served.state = state.to_string();
        served.private_state = private_state.to_string();
        served.background = background;
        served.dark = dark;
        self.inputs = Some(inputs);
        true
    }

    /// The background image file and its MIME type, if one is set.
    fn background_file(&self) -> Option<(PathBuf, String)> {
        let path = self.background_path();
        let mime = image_mime(&path)?;
        Some((path, mime))
    }

    /// Takes a background picked in the file chooser. Returns whether the config changed.
    pub fn poll_chooser(&mut self, config: &mut Config) -> bool {
        let chosen = self.chosen.lock().unwrap_or_else(PoisonError::into_inner).take();
        chosen.is_some_and(|path| self.set_background(&path, config))
    }

    /// Copies `path` into the profile as the background image and shows it.
    fn set_background(&mut self, path: &Path, config: &mut Config) -> bool {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size == 0 || size > MAX_BACKGROUND_BYTES || image_mime(path).is_none() {
            tracing::warn!(path = %path.display(), "not a usable background image");
            return false;
        }
        if let Err(e) = std::fs::create_dir_all(&self.dir).and_then(|()| {
            let tmp = self.dir.join("background.tmp");
            std::fs::copy(path, &tmp)?;
            std::fs::rename(&tmp, self.background_path())
        }) {
            tracing::warn!("saving the background image failed: {e}");
            return false;
        }
        self.background_version += 1;
        self.stale = true;
        config.home.show_background_image = true;
        true
    }

    /// Acts on a message the start page posted (see `home.js`).
    pub fn handle(&mut self, message: &str, config: &mut Config, store: Option<&Store>) -> HomeAction {
        let Ok(message) = serde_json::from_str::<Value>(message) else {
            tracing::debug!("unreadable start page message");
            return HomeAction::Nothing;
        };
        let text = |key: &str| message.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        let kind = text("type");
        let home = &mut config.home;
        match kind.as_str() {
            "go" => {
                let query = text("text");
                return if query.is_empty() { HomeAction::Nothing } else { HomeAction::Go(query) };
            }
            "engine" => {
                let Some(engine) = message
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|i| SEARCH_ENGINES.get(usize::try_from(i).ok()?))
                else {
                    return HomeAction::Nothing;
                };
                config.general.search_url = engine.1.to_owned();
            }
            "widget" => {
                let on = message.get("on").and_then(Value::as_bool).unwrap_or(true);
                match text("name").as_str() {
                    "clock" => home.show_clock = on,
                    "greeting" => home.show_greeting = on,
                    "search" => home.show_search = on,
                    "topSites" => home.show_top_sites = on,
                    "stats" => home.show_stats = on,
                    "background" => home.show_background_image = on,
                    _ => return HomeAction::Nothing,
                }
            }
            "clockFormat" => {
                home.clock_format = match text("format").as_str() {
                    "12h" => ClockFormat::H12,
                    "24h" => ClockFormat::H24,
                    _ => ClockFormat::Auto,
                };
            }
            "backgroundLook" => {
                if let Some(dim) = message.get("dim").and_then(Value::as_f64) {
                    home.background_dim = (dim as f32).clamp(0.0, 0.9);
                }
                if let Some(blur) = message.get("blur").and_then(Value::as_f64) {
                    home.background_blur = (blur as f32).clamp(0.0, 40.0);
                }
            }
            "chooseBackground" => {
                let chosen = Arc::clone(&self.chosen);
                spawn("choose-background", move || match choose_portal("Background Image", Choose::Image) {
                    Ok(Some(path)) => *chosen.lock().unwrap_or_else(PoisonError::into_inner) = Some(path),
                    Ok(None) => {}
                    Err(e) => tracing::warn!("file chooser unavailable: {e}"),
                });
                return HomeAction::Nothing;
            }
            "backgroundPath" => {
                let path = text("path");
                let path = match path.strip_prefix("~/") {
                    Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
                    None => PathBuf::from(path),
                };
                return if self.set_background(&path, config) {
                    HomeAction::ConfigChanged
                } else {
                    HomeAction::Refresh
                };
            }
            "clearBackground" => {
                if let Err(e) = std::fs::remove_file(self.background_path())
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!("removing the background image failed: {e}");
                }
                home.show_background_image = false;
                self.background_version += 1;
                self.stale = true;
            }
            "removeSite" | "unpinSite" | "pinSite" | "editSite" | "restoreSites" => {
                let Some(store) = store else { return HomeAction::Nothing };
                let search_url = config.general.search_url.clone();
                if let Err(e) = edit_top_sites(store, &kind, &text, &search_url) {
                    tracing::warn!("changing the top sites failed: {e}");
                }
                self.stale = true;
                return HomeAction::Refresh;
            }
            "openSettings" => return HomeAction::OpenSettings,
            _ => {
                tracing::debug!(kind, "unknown start page message");
                return HomeAction::Nothing;
            }
        }
        HomeAction::ConfigChanged
    }
}

/// Pins, edits, removes or restores top sites.
fn edit_top_sites(
    store: &Store,
    kind: &str,
    text: &dyn Fn(&str) -> String,
    search_url: &str,
) -> axion_store::Result<()> {
    let host_of = |url: &str| site_host(url);
    match kind {
        "removeSite" => {
            if let Some(host) = host_of(&text("url")) {
                store.hide_top_site(&host)?;
            }
        }
        "unpinSite" => {
            if let Some(host) = host_of(&text("url")) {
                store.unpin_top_site(&host)?;
            }
        }
        "pinSite" | "editSite" => {
            // Only web addresses (a typed "example.com" becomes https://example.com).
            let input = text("url");
            let url =
                resolve_input(&input, search_url).filter(|u| is_address(&input) && site_host(u).is_some());
            let Some(url) = url else {
                return Ok(());
            };
            let Some(host) = host_of(&url) else { return Ok(()) };
            let title = text("title");
            if let Some(old) = host_of(&text("old")).filter(|old| *old != host) {
                // Edited into another site: the old one doesn't come back from history.
                store.hide_top_site(&old)?;
            }
            store.pin_top_site(&host, &url, &title)?;
        }
        "restoreSites" => store.restore_hidden_top_sites()?,
        _ => {}
    }
    Ok(())
}

/// One tile of the top sites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub url: String,
    pub title: String,
    pub host: String,
    pub pinned: bool,
}

/// The start page's sites: pinned ones in their order, then the most visited (without removed
/// and pinned hosts), `limit` at most. Also returns how many sites were removed.
pub fn top_sites(store: &Store, limit: usize, now: i64) -> (Vec<Site>, usize) {
    let overrides = store.top_site_overrides().unwrap_or_else(|e| {
        tracing::warn!("reading the top sites failed: {e}");
        Vec::new()
    });
    let hidden = overrides.iter().filter(|o| o.hidden).count();
    let mut sites: Vec<Site> = overrides
        .iter()
        .filter(|o| !o.hidden && !o.url.is_empty())
        .map(|o| Site { url: o.url.clone(), title: o.title.clone(), host: o.host.clone(), pinned: true })
        .collect();
    let skip = |host: &str| overrides.iter().any(|o| o.host == host);
    let visited = store.top_sites(limit + overrides.len(), now).unwrap_or_default();
    for site in visited {
        if sites.len() >= limit {
            break;
        }
        if !skip(&site.host) {
            sites.push(Site { url: site.url, title: site.title, host: site.host, pinned: false });
        }
    }
    sites.truncate(limit);
    (sites, hidden)
}

/// Whether the locale's clock shows 12 hours (en_US and a few others).
fn locale_uses_12h() -> bool {
    let locale = ["LC_ALL", "LC_TIME", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    uses_12h(&locale)
}

fn uses_12h(locale: &str) -> bool {
    let region = locale.split(['.', '@']).next().unwrap_or_default();
    [
        "en_US", "en_CA", "en_AU", "en_NZ", "en_IN", "en_PH", "es_US", "es_MX", "hi_IN", "ar_EG", "ar_SA",
        "ko_KR", "fil_PH",
    ]
    .contains(&region)
}

/// `/api/state`: everything the page shows.
#[allow(clippy::too_many_arguments)]
fn state_json(
    icons_version: u64,
    config: &Config,
    store: Option<&Store>,
    blocked: u64,
    name: &str,
    private: bool,
    has_background: bool,
    background_version: u64,
    now: i64,
) -> Value {
    let home = &config.home;
    let h24 = match home.clock_format {
        ClockFormat::Auto => !locale_uses_12h(),
        ClockFormat::H12 => false,
        ClockFormat::H24 => true,
    };
    let current = SEARCH_ENGINES.iter().position(|(_, url)| *url == config.general.search_url);
    let engines: Vec<Value> = SEARCH_ENGINES
        .iter()
        .map(|(name, url)| json!({ "name": name, "host": site_host(url).unwrap_or_default() }))
        .collect();
    let custom_host = site_host(&config.general.search_url).unwrap_or_default();
    let limit =
        (home.top_sites_columns.max(1) * home.top_sites_rows.max(1) * home.top_sites_pages.max(1)) as usize;
    let (sites, hidden) = match store {
        Some(store) if !private => top_sites(store, limit, now),
        _ => (Vec::new(), 0),
    };
    let sites: Vec<Value> = sites
        .iter()
        .map(|s| json!({ "url": s.url, "title": s.title, "host": s.host, "pinned": s.pinned }))
        .collect();
    let first_name = name.split_whitespace().next().unwrap_or_default();
    json!({
        "private": private,
        "icons": icons_version,
        "clock": { "show": home.show_clock, "h24": h24, "format": match home.clock_format {
            ClockFormat::Auto => "auto", ClockFormat::H12 => "12h", ClockFormat::H24 => "24h" } },
        "greeting": { "show": home.show_greeting && !private, "name": first_name },
        "search": {
            "show": home.show_search,
            "engines": engines,
            "current": current.map_or(-1, |i| i as i64),
            "customHost": custom_host,
        },
        "topSites": {
            "show": home.show_top_sites && !private,
            "columns": home.top_sites_columns.max(1),
            "rows": home.top_sites_rows.max(1),
            "sites": sites,
            "hidden": hidden,
        },
        "stats": {
            "show": home.show_stats && !private,
            "blocked": blocked,
            "bytes": blocked.saturating_mul(BYTES_PER_BLOCKED_REQUEST),
            "seconds": blocked as f64 * SECONDS_PER_BLOCKED_REQUEST,
        },
        "background": {
            "show": home.show_background_image && has_background && !private,
            "enabled": home.show_background_image,
            "has": has_background,
            "version": background_version,
            "dim": home.background_dim,
            "blur": home.background_blur,
        },
    })
}

/// The MIME type of an image file, by its first bytes (`None`: not an image Chromium shows).
fn image_mime(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = [0u8; 32];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    let format = image::guess_format(&head[..n]).ok()?;
    use image::ImageFormat as F;
    matches!(format, F::Png | F::Jpeg | F::Gif | F::WebP | F::Avif | F::Bmp)
        .then(|| format.to_mime_type().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_urls() {
        assert!(is_home_url("axion://home/"));
        assert!(is_home_url("axion://home"));
        assert!(is_home_url("axion://HOME/?x#y"));
        assert!(!is_home_url("axion://settings/"));
        assert!(!is_home_url("https://home/"));
    }

    #[test]
    fn queries() {
        assert_eq!(query_value("host=a.example&x=1", "host").as_deref(), Some("a.example"));
        assert_eq!(query_value("q=a%20b+c%2", "q").as_deref(), Some("a b c%2"));
        assert_eq!(query_value("", "host"), None);
    }

    #[test]
    fn clock_by_locale() {
        assert!(uses_12h("en_US.UTF-8"));
        assert!(!uses_12h("de_DE.UTF-8"));
        assert!(!uses_12h("en_GB.UTF-8"));
        assert!(!uses_12h(""));
    }

    #[test]
    fn pinned_first_then_visited_without_removed() {
        let store = Store::in_memory().unwrap();
        let now = 1_800_000_000;
        for _ in 0..3 {
            store.record_visit("https://often.example/", "Often", now).unwrap();
        }
        store.record_visit("https://gone.example/", "Gone", now).unwrap();
        store.record_visit("https://pinned.example/x", "P", now).unwrap();
        let mut config = Config::default();
        let mut page = HomePage::new(Path::new("/nonexistent"), None);
        let msg = |v: Value| v.to_string();
        let action = page.handle(
            &msg(json!({"type": "pinSite", "url": "pinned.example", "title": "Pinned"})),
            &mut config,
            Some(&store),
        );
        assert_eq!(action, HomeAction::Refresh);
        page.handle(
            &msg(json!({"type": "removeSite", "url": "https://gone.example/"})),
            &mut config,
            Some(&store),
        );
        let (sites, hidden) = top_sites(&store, 10, now);
        let hosts: Vec<(&str, bool)> = sites.iter().map(|s| (s.host.as_str(), s.pinned)).collect();
        assert_eq!(hosts, [("pinned.example", true), ("often.example", false)]);
        assert_eq!(sites[0].url, "https://pinned.example");
        assert_eq!(hidden, 1);
        // Not a web address: nothing pinned.
        page.handle(
            &msg(json!({"type": "pinSite", "url": "javascript:alert(1)"})),
            &mut config,
            Some(&store),
        );
        assert_eq!(top_sites(&store, 10, now).0.len(), 2);
        page.handle(&msg(json!({"type": "restoreSites"})), &mut config, Some(&store));
        assert_eq!(top_sites(&store, 10, now).0.len(), 3);
        assert_eq!(top_sites(&store, 1, now).0.len(), 1);
    }

    #[test]
    fn settings_messages() {
        let mut config = Config::default();
        let mut page = HomePage::new(Path::new("/nonexistent"), None);
        let mut send = |v: Value| page.handle(&v.to_string(), &mut config, None);
        assert_eq!(send(json!({"type": "widget", "name": "clock", "on": false})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "engine", "index": 1})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "engine", "index": 99})), HomeAction::Nothing);
        assert_eq!(send(json!({"type": "clockFormat", "format": "24h"})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "go", "text": " rust "})), HomeAction::Go("rust".into()));
        assert_eq!(send(json!({"type": "nonsense"})), HomeAction::Nothing);
        assert_eq!(page.handle("not json", &mut config, None), HomeAction::Nothing);
        assert!(!config.home.show_clock);
        assert_eq!(config.general.search_url, SEARCH_ENGINES[1].1);
        assert_eq!(config.home.clock_format, ClockFormat::H24);
    }

    #[test]
    fn private_state_has_no_history() {
        let store = Store::in_memory().unwrap();
        store.record_visit("https://often.example/", "Often", 100).unwrap();
        let config = Config::default();
        let state = state_json(1, &config, Some(&store), 7, "Ada Lovelace", true, true, 1, 100);
        assert_eq!(state["topSites"]["sites"].as_array().map(Vec::len), Some(0));
        assert_eq!(state["stats"]["show"], false);
        assert_eq!(state["background"]["show"], false);
        let state = state_json(1, &config, Some(&store), 7, "Ada Lovelace", false, false, 1, 100);
        assert_eq!(state["topSites"]["sites"][0]["host"], "often.example");
        assert_eq!(state["stats"]["bytes"], 7 * BYTES_PER_BLOCKED_REQUEST);
        assert_eq!(state["greeting"]["name"], "Ada");
    }

    #[test]
    fn the_service_serves_only_its_host() {
        let service = HomeService::default();
        let request = |path: &'static str| InternalRequest {
            host: HOST,
            path,
            query: "",
            private: false,
            from_own_page: true,
        };
        assert_eq!(service.serve(&request("/")).map(|r| r.mime), Some("text/html".into()));
        assert!(service.serve(&request("/nope")).is_none());
        assert!(service.serve(&InternalRequest { host: "other", ..request("/") }).is_none());
    }
}
