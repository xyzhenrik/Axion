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
//! Private tabs get a private variant: no top sites, history, Shields numbers or background.
//!
//! Background images live in the account's backgrounds directory (see [`backgrounds`]); the
//! page only ever learns their generated file names, and `/api/background` and `/api/thumb`
//! serve nothing but listed files from that directory.

mod backgrounds;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use axion_config::{ClockFormat, Color, Config, Home, START_PAGE_URL};
use axion_engine::internal::split_internal_url;
use axion_engine::{InternalPages, InternalRequest, InternalResponse};
use axion_store::{Store, site_host};
use serde_json::{Value, json};

pub use backgrounds::{Arrived, Image, Textures};
use backgrounds::{Kind, LEGACY_FILE, Library, Thumbnails, valid_name};

use crate::downloads::{Choose, choose_portal, spawn};
use crate::settings::{BACKGROUND_SLOTS, BackgroundCard, SEARCH_ENGINES};
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

/// What the network thread serves, kept ready by [`HomePage::update`].
#[derive(Default)]
struct Served {
    /// `/api/state` for normal and private tabs.
    state: String,
    private_state: String,
    /// The selected background image and its MIME type, while one is set.
    background: Option<(PathBuf, String)>,
    /// Every background (file name, MIME type) and their thumbnails' directory.
    backgrounds: Vec<(String, String)>,
    backgrounds_dir: PathBuf,
    thumbs_dir: PathBuf,
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
            // The selected background, or (for the first frame of a video that has no
            // thumbnail yet) a listed one by `name`.
            "/api/background" if !request.private => {
                let (path, mime) = match query_value(request.query, "name").filter(|n| valid_name(n)) {
                    Some(name) => {
                        let (_, mime) = served.backgrounds.iter().find(|(n, _)| *n == name)?;
                        (served.backgrounds_dir.join(&name), mime)
                    }
                    None => {
                        let (path, mime) = served.background.as_ref()?;
                        (path.clone(), mime)
                    }
                };
                InternalResponse::ok(mime, std::fs::read(path).ok()?)
            }
            // A background's thumbnail by its file name (only listed ones), or the image
            // itself while it has none.
            "/api/thumb" if !request.private => {
                let name = query_value(request.query, "name").filter(|n| valid_name(n))?;
                let (_, mime) = served.backgrounds.iter().find(|(n, _)| *n == name)?;
                match std::fs::read(served.thumbs_dir.join(format!("{name}.png"))) {
                    Ok(bytes) => InternalResponse::ok("image/png", bytes),
                    // Videos have no picture of their own; the page makes their thumbnail.
                    Err(_) if mime.starts_with("video/") => return None,
                    Err(_) => {
                        InternalResponse::ok(mime, std::fs::read(served.backgrounds_dir.join(&name)).ok()?)
                    }
                }
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
    /// Top sites or the backgrounds changed: open start pages should refresh.
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
    accent: Color,
    reduce_motion: bool,
}

/// The start page's side in the browser (main thread).
pub struct HomePage {
    service: Arc<HomeService>,
    /// `<profile>/home`.
    dir: PathBuf,
    inputs: Option<Inputs>,
    /// The Shields total last shown, and when.
    blocked_shown: Option<(u64, Instant)>,
    /// The background images, and their thumbnails for Settings.
    library: Library,
    thumbs: Thumbnails,
    /// Bumped when backgrounds are added or removed.
    generation: u64,
    /// A background picked in the desktop's file chooser, waiting to be taken.
    chosen: Arc<Mutex<Option<PathBuf>>>,
    /// The top sites or the backgrounds changed: rebuild at the next update.
    stale: bool,
    /// Bumped when site icons arrive (the page's icon URLs change with it).
    icons_version: u64,
}

impl HomePage {
    pub fn new(profile: &Path, favicons: Option<PathBuf>) -> Self {
        let service = Arc::new(HomeService::default());
        service.0.write().unwrap_or_else(PoisonError::into_inner).favicons = favicons;
        let dir = profile.join("home");
        let library = Library::new(&dir);
        let mut thumbs = Thumbnails::start();
        // Thumbnails are made in the background, so the start page's picker has them soon.
        thumbs.request(&library);
        Self {
            service,
            dir,
            inputs: None,
            blocked_shown: None,
            library,
            thumbs,
            generation: 1,
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

    /// Moves the single background image of older versions into the backgrounds directory,
    /// selected if it was shown. Returns whether the config changed.
    pub fn migrate(&mut self, config: &mut Config) -> bool {
        let Some(name) = self.library.migrate(&self.dir.join(LEGACY_FILE)) else { return false };
        let _ = std::fs::remove_file(self.dir.join("background.tmp"));
        self.backgrounds_changed();
        if config.home.background.is_empty() && config.home.legacy_background_shown() {
            config.home.background = name;
        }
        true
    }

    /// Lets Settings show the thumbnails as textures.
    pub fn set_gpu(&mut self, textures: Textures) {
        self.thumbs.set_gpu(textures);
        self.thumbs.request(&self.library);
    }

    fn backgrounds_changed(&mut self) {
        self.generation += 1;
        self.stale = true;
        self.thumbs.request(&self.library);
    }

    /// Bumped whenever backgrounds are added or removed.
    pub fn backgrounds_generation(&self) -> u64 {
        self.generation
    }

    /// The selected background, if it (still) exists.
    fn selected<'a>(&self, config: &'a Config) -> Option<&'a str> {
        let name = config.home.background.as_str();
        self.library.contains(name).then_some(name)
    }

    /// The backgrounds as Settings shows them.
    pub fn background_cards(&self) -> Vec<BackgroundCard> {
        self.library
            .names()
            .take(BACKGROUND_SLOTS)
            .map(|name| {
                let thumb = self.thumbs.get(name);
                BackgroundCard { name: name.to_owned(), loading: thumb.is_none(), image: thumb.flatten() }
            })
            .collect()
    }

    /// Settings' preview of the selected background: blurred like the page would be (for a
    /// page `page_width` logical px wide), or its plain thumbnail until that is made.
    pub fn background_preview(&mut self, config: &Config, page_width: f32) -> Option<(String, Image)> {
        let name = self.selected(config)?.to_owned();
        let plain = self.thumbs.get(&name).flatten();
        if let Some((_, (width, _))) = &plain {
            let sigma = config.home.background_blur * *width as f32 / page_width.max(1.0);
            self.thumbs.request_preview(&self.library, &name, sigma);
        }
        let image = self.thumbs.preview(&name).or(plain)?;
        Some((name, image))
    }

    /// Takes the thumbnails made meanwhile (call every frame).
    pub fn poll_thumbnails(&mut self) -> Arrived {
        self.thumbs.poll(&self.library)
    }

    /// Copies `path` into the backgrounds and selects it. Returns whether the config changed.
    pub fn add_background(&mut self, path: &Path, config: &mut Config) -> bool {
        let max = (config.home.max_backgrounds as usize).min(BACKGROUND_SLOTS);
        let max_bytes = u64::from(config.home.max_background_mb) * 1024 * 1024;
        let Some(name) = self.library.add(path, max, max_bytes) else { return false };
        self.backgrounds_changed();
        config.home.background = name;
        true
    }

    /// Deletes a background (and selects none if it was selected). Returns whether the
    /// config changed.
    pub fn remove_background(&mut self, name: &str, config: &mut Config) -> bool {
        if !valid_name(name) || !self.library.remove(name) {
            return false;
        }
        self.backgrounds_changed();
        if config.home.background == name {
            config.home.background.clear();
            return true;
        }
        false
    }

    /// Selects a background by name (empty: none). Returns whether the config changed.
    pub fn select_background(&mut self, name: &str, config: &mut Config) -> bool {
        if !name.is_empty() && !self.library.contains(name) || config.home.background == name {
            return false;
        }
        config.home.background = name.to_owned();
        true
    }

    /// Opens the desktop's file chooser for a new background (taken by [`Self::poll_chooser`]).
    pub fn choose_background(&self) {
        let chosen = Arc::clone(&self.chosen);
        spawn("choose-background", move || match choose_portal("Background", Choose::Image) {
            Ok(Some(path)) => *chosen.lock().unwrap_or_else(PoisonError::into_inner) = Some(path),
            Ok(None) => {}
            Err(e) => tracing::warn!("file chooser unavailable: {e}"),
        });
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
            accent: config.appearance.accent,
            reduce_motion: config.glass.reduce_motion,
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
        let selected = self.selected(config);
        let names: Vec<&str> = self.library.names().collect();
        let mimes: Vec<&str> = self.library.entries().iter().map(|(_, m)| m.as_str()).collect();
        let kinds: Vec<&str> =
            names.iter().map(|n| self.library.kind(n).map_or("image", Kind::as_str)).collect();
        let needs_thumb: Vec<&str> = names
            .iter()
            .zip(&kinds)
            .filter(|(n, k)| **k == "video" && !self.library.thumb_path(n).exists())
            .map(|(n, _)| *n)
            .collect();
        let backgrounds = Backgrounds {
            selected,
            names: &names,
            kinds: &kinds,
            mimes: &mimes,
            needs_thumb: &needs_thumb,
            live: config.home.live_wallpapers && !config.glass.reduce_motion,
            live_setting: config.home.live_wallpapers,
        };
        let state = state_json(
            self.icons_version,
            config,
            store,
            blocked_total,
            account_name,
            false,
            &backgrounds,
            now,
        );
        let private_state =
            state_json(self.icons_version, config, None, 0, account_name, true, &Backgrounds::PRIVATE, now);
        let background = selected.and_then(|name| self.library.file(name));
        let mut served = self.service.0.write().unwrap_or_else(PoisonError::into_inner);
        served.state = state.to_string();
        served.private_state = private_state.to_string();
        served.background = background;
        served.backgrounds = self.library.entries().to_vec();
        served.backgrounds_dir = self.library.dir().to_path_buf();
        served.thumbs_dir = self.library.thumbs_dir();
        served.dark = dark;
        self.inputs = Some(inputs);
        true
    }

    /// Takes a background picked in the file chooser. Returns whether the config changed.
    pub fn poll_chooser(&mut self, config: &mut Config) -> bool {
        let chosen = self.chosen.lock().unwrap_or_else(PoisonError::into_inner).take();
        chosen.is_some_and(|path| self.add_background(&path, config))
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
                    "liveWallpapers" => home.live_wallpapers = on,
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
            // The first frame of a video the page decoded (a data URL), as its thumbnail.
            "backgroundThumb" => {
                let data = text("data");
                let bytes = data.split_once(";base64,").and_then(|(_, b64)| base64_decode(b64));
                if bytes.is_some_and(|b| self.library.save_video_thumb(&text("name"), &b)) {
                    self.thumbs.forget(&text("name"));
                    self.backgrounds_changed();
                }
                return HomeAction::Nothing;
            }
            "chooseBackground" => {
                self.choose_background();
                return HomeAction::Nothing;
            }
            "backgroundPath" | "selectBackground" | "removeBackground" => {
                let changed = match kind.as_str() {
                    "backgroundPath" => self.add_background(&expand_home(&text("path")), config),
                    "selectBackground" => self.select_background(&text("name"), config),
                    _ => self.remove_background(&text("name"), config),
                };
                // Unchanged: the page shows the truth again (e.g. after an unusable path).
                return if changed { HomeAction::ConfigChanged } else { HomeAction::Refresh };
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

/// "~/x" → the home directory's "x".
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// The backgrounds `/api/state` lists.
struct Backgrounds<'a> {
    /// The selected one, if any (and it exists).
    selected: Option<&'a str>,
    names: &'a [&'a str],
    /// What each of `names` is (`image`, `animated`, `video`).
    kinds: &'a [&'a str],
    /// Their MIME types.
    mimes: &'a [&'a str],
    /// Videos without a thumbnail: the page makes one from the first frame.
    needs_thumb: &'a [&'a str],
    /// Animated images and videos play (`home.live_wallpapers`, not Reduce Motion).
    live: bool,
    /// The `home.live_wallpapers` setting itself (`live` is also off with Reduce Motion).
    live_setting: bool,
}

impl Backgrounds<'_> {
    /// Private start pages show none and list none.
    const PRIVATE: Backgrounds<'static> = Backgrounds {
        selected: None,
        names: &[],
        kinds: &[],
        mimes: &[],
        needs_thumb: &[],
        live: false,
        live_setting: false,
    };
}

/// Decodes standard base64 (padding optional); `None` for anything else.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in text.trim_end_matches('=').bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `#RRGGBB` for CSS (the accent is opaque).
fn css_color(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
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
    backgrounds: &Backgrounds<'_>,
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
        "accent": css_color(config.appearance.accent),
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
            "show": backgrounds.selected.is_some() && !private,
            "selected": backgrounds.selected.unwrap_or_default(),
            "list": backgrounds.names,
            "kinds": backgrounds.kinds,
            "mimes": backgrounds.mimes,
            "liveSetting": backgrounds.live_setting,
            "needsThumb": backgrounds.needs_thumb,
            "live": backgrounds.live,
            "canAdd": backgrounds.names.len() < (home.max_backgrounds as usize).min(BACKGROUND_SLOTS),
            "dim": home.background_dim,
            "blur": home.background_blur,
        },
    })
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
    fn base64() {
        assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("").as_deref(), Some(&b""[..]));
        assert!(base64_decode("a b").is_none());
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
        let mut config = Config::default();
        config.appearance.accent = Color::rgb(0xE2457A);
        let names = ["bg-1.png"];
        let backgrounds = Backgrounds {
            selected: Some("bg-1.png"),
            names: &names,
            kinds: &[],
            mimes: &[],
            needs_thumb: &[],
            live: true,
            live_setting: true,
        };
        let state = state_json(1, &config, Some(&store), 7, "Ada Lovelace", true, &Backgrounds::PRIVATE, 100);
        assert_eq!(state["topSites"]["sites"].as_array().map(Vec::len), Some(0));
        assert_eq!(state["stats"]["show"], false);
        assert_eq!(state["background"]["show"], false);
        assert_eq!(state["background"]["list"].as_array().map(Vec::len), Some(0));
        let state = state_json(1, &config, Some(&store), 7, "Ada Lovelace", false, &backgrounds, 100);
        assert_eq!(state["accent"], "#E2457A");
        assert_eq!(state["background"]["show"], true);
        assert_eq!(state["background"]["selected"], "bg-1.png");
        assert_eq!(state["background"]["list"][0], "bg-1.png");
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

    #[test]
    fn backgrounds_from_the_page() {
        let tmp = std::env::temp_dir().join(format!("axion-home-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("home")).unwrap();
        let picture = |path: &Path| {
            let image = image::RgbaImage::from_pixel(32, 20, image::Rgba([10, 120, 200, 255]));
            image.save_with_format(path, image::ImageFormat::Png).unwrap();
        };
        // An old single background, shown: migrated and selected.
        picture(&tmp.join("home").join(LEGACY_FILE));
        std::fs::write(tmp.join("config.toml"), "[home]\nshow_background_image = true\n").unwrap();
        let mut config = axion_config::load_config(&axion_config::ConfigPaths::in_dir(tmp.clone())).unwrap();
        let mut page = HomePage::new(&tmp, None);
        assert!(page.migrate(&mut config));
        let migrated = config.home.background.clone();
        assert!(migrated.starts_with("bg-") && migrated.ends_with(".png"), "{migrated}");
        assert!(!page.migrate(&mut config));

        let outside = tmp.join("outside.png");
        picture(&outside);
        let mut send = |v: Value| page.handle(&v.to_string(), &mut config, None);
        let path = outside.display().to_string();
        assert_eq!(send(json!({"type": "backgroundPath", "path": path})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "backgroundPath", "path": "/nonexistent.png"})), HomeAction::Refresh);
        assert_eq!(send(json!({"type": "selectBackground", "name": "../outside.png"})), HomeAction::Refresh);
        assert_eq!(send(json!({"type": "removeBackground", "name": "../outside.png"})), HomeAction::Refresh);
        assert!(outside.exists());
        assert_eq!(send(json!({"type": "selectBackground", "name": migrated})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "selectBackground", "name": ""})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "selectBackground", "name": migrated})), HomeAction::ConfigChanged);
        assert_eq!(send(json!({"type": "removeBackground", "name": migrated})), HomeAction::ConfigChanged);
        assert!(config.home.background.is_empty());
        assert_eq!(page.library.entries().len(), 1);

        // The service serves only listed files, by name.
        page.update(&config, None, 0, "", false);
        let added = page.library.entries()[0].0.clone();
        let service = Arc::clone(&page.service);
        let query = format!("name={added}");
        let request = |path: &'static str, query: &str| {
            service
                .serve(&InternalRequest { host: HOST, path, query, private: false, from_own_page: true })
                .map(|r| r.mime)
        };
        assert_eq!(request("/api/thumb", &query), Some("image/png".into()));
        assert_eq!(request("/api/thumb", "name=..%2Foutside.png"), None);
        assert_eq!(request("/api/thumb", "name=.thumbs"), None);
        assert_eq!(request("/api/background", ""), None, "none selected");
        config.home.background = added;
        page.update(&config, None, 0, "", false);
        assert_eq!(request("/api/background", ""), Some("image/png".into()));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
