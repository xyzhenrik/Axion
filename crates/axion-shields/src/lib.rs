//! Axion Shields: blocks ads and trackers with Brave's adblock engine and the usual filter
//! lists, blocks popups and redirects a page makes on its own, and removes click-tracking
//! parameters from links.
//!
//! [`Shields`] is thread-safe: the web engine asks it about every request from its network
//! thread while the UI changes per-site levels. Decisions take the *page* URL (the tab's
//! top-level page), because levels are per site.

mod lists;
pub mod site;
pub mod tracking;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub use lists::ListError;

/// How hard Shields blocks on a site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Level {
    Off,
    /// Third-party ads and trackers, ad popups and ad redirects.
    #[default]
    Standard,
    /// Also first-party ads, every cross-site redirect without a click, and `utm_` tags.
    Aggressive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub default_level: Level,
    pub hide_ad_elements: bool,
    pub block_popups: bool,
    pub block_redirects: bool,
    pub strip_tracking_parameters: bool,
    pub block_third_party_cookies: bool,
    pub lists: Vec<String>,
    pub update_interval: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_level: Level::Standard,
            hide_ad_elements: true,
            block_popups: true,
            block_redirects: true,
            strip_tracking_parameters: true,
            block_third_party_cookies: true,
            lists: Vec::new(),
            update_interval: Duration::from_secs(4 * 24 * 3600),
        }
    }
}

/// What a request is for, as far as filter rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// The top-level page.
    Document,
    /// An iframe.
    Subdocument,
    Stylesheet,
    Script,
    Image,
    Font,
    Media,
    Object,
    Xhr,
    Ping,
    Websocket,
    Other,
}

impl ResourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Subdocument => "subdocument",
            Self::Stylesheet => "stylesheet",
            Self::Script => "script",
            Self::Image => "image",
            Self::Font => "font",
            Self::Media => "media",
            Self::Object => "object",
            Self::Xhr => "xmlhttprequest",
            Self::Ping => "ping",
            Self::Websocket => "websocket",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
    /// Load this URL instead (tracking parameters removed).
    Rewrite(String),
}

/// A top-level navigation, for [`Shields::allow_navigation`].
#[derive(Clone, Copy, Debug)]
pub struct Navigation<'a> {
    /// The page being left (empty for a new tab).
    pub from: &'a str,
    pub to: &'a str,
    /// Caused by a click or key press in the page.
    pub user_gesture: bool,
    /// An HTTP redirect of a navigation already under way.
    pub server_redirect: bool,
    /// Started by the browser itself: typed, bookmark, back/forward, reload.
    pub browser_initiated: bool,
}

pub struct Shields {
    engines: RwLock<Arc<lists::Engines>>,
    ready: AtomicBool,
    updating: AtomicBool,
    settings: RwLock<Settings>,
    /// Per-site levels, keyed by [`site::host`].
    sites: RwLock<HashMap<String, Level>>,
    cache: lists::ListCache,
}

impl Shields {
    /// Loads the cached filter engine from `dir` (if any) and, when the lists are missing or
    /// stale, downloads them in the background.
    pub fn new(dir: PathBuf, settings: Settings) -> Arc<Self> {
        let cache = lists::ListCache::new(dir);
        let cached = cache.load(&settings.lists);
        let shields = Arc::new(Self {
            ready: AtomicBool::new(cached.is_some()),
            engines: RwLock::new(Arc::new(cached.unwrap_or_else(lists::Engines::empty))),
            updating: AtomicBool::new(false),
            settings: RwLock::new(settings),
            sites: RwLock::new(HashMap::new()),
            cache,
        });
        shields.update_if_stale();
        shields
    }

    /// A Shields with these filter rules and no cache, for tests.
    pub fn with_rules(rules: &str, settings: Settings) -> Arc<Self> {
        Arc::new(Self {
            engines: RwLock::new(Arc::new(lists::Engines::from_rules(rules))),
            ready: AtomicBool::new(true),
            updating: AtomicBool::new(false),
            settings: RwLock::new(settings),
            sites: RwLock::new(HashMap::new()),
            cache: lists::ListCache::new(std::env::temp_dir().join("axion-shields-unused")),
        })
    }

    /// Whether filter lists are loaded (until then only popups, redirects without lists and
    /// tracking parameters are handled).
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// Starts a background download when the lists are missing, stale or changed.
    pub fn update_if_stale(self: &Arc<Self>) {
        let (lists, interval) = {
            let s = read(&self.settings);
            (s.lists.clone(), s.update_interval)
        };
        if lists.is_empty() || !self.cache.is_stale(&lists, interval) {
            return;
        }
        self.update_now();
    }

    /// Downloads the lists again in the background, unless an update is already running.
    pub fn update_now(self: &Arc<Self>) {
        if self.updating.swap(true, Ordering::AcqRel) {
            return;
        }
        let shields = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("shields-update".into()).spawn(move || {
            let lists = read(&shields.settings).lists.clone();
            match shields.cache.update(&lists) {
                Ok(engines) => {
                    *write(&shields.engines) = Arc::new(engines);
                    shields.ready.store(true, Ordering::Relaxed);
                }
                Err(e) => tracing::warn!("Shields could not update its filter lists: {e}"),
            }
            shields.updating.store(false, Ordering::Release);
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the filter list update: {e}");
            self.updating.store(false, Ordering::Release);
        }
    }

    pub fn settings(&self) -> Settings {
        read(&self.settings).clone()
    }

    pub fn set_settings(self: &Arc<Self>, settings: Settings) {
        let lists_changed = read(&self.settings).lists != settings.lists;
        *write(&self.settings) = settings;
        if lists_changed {
            self.update_if_stale();
        }
    }

    /// The level on the page's site.
    pub fn level(&self, page_url: &str) -> Level {
        let own = site::host(page_url).and_then(|h| read(&self.sites).get(&h).copied());
        own.unwrap_or_else(|| read(&self.settings).default_level)
    }

    /// The site's own level, if it has one (`host` as from [`site::host`]).
    pub fn site_level(&self, host: &str) -> Option<Level> {
        read(&self.sites).get(host).copied()
    }

    /// Sets (or with `None` clears) a site's own level.
    pub fn set_site_level(&self, host: &str, level: Option<Level>) {
        let mut sites = write(&self.sites);
        match level {
            Some(level) => sites.insert(host.to_owned(), level),
            None => sites.remove(host),
        };
    }

    /// Replaces all per-site levels (e.g. loaded from the profile).
    pub fn set_site_levels(&self, levels: impl IntoIterator<Item = (String, Level)>) {
        *write(&self.sites) = levels.into_iter().collect();
    }

    fn engines(&self) -> Arc<lists::Engines> {
        Arc::clone(&read(&self.engines))
    }

    /// Whether the lists name `url` as an ad or tracker page (for popups and redirects).
    fn is_ad_page(&self, url: &str, page_url: &str) -> bool {
        let Ok(request) = adblock::request::Request::new(url, page_url, "document", "GET") else {
            return false;
        };
        let engines = self.engines();
        engines.popups.check_network_request(&request).should_block()
            || engines.network.check_network_request(&request).should_block()
    }

    /// Decides one network request from a page (any thread).
    pub fn check_request(&self, url: &str, page_url: &str, kind: ResourceKind) -> Verdict {
        let page_url = if page_url.is_empty() { url } else { page_url };
        let level = self.level(page_url);
        if level == Level::Off {
            return Verdict::Allow;
        }
        if kind == ResourceKind::Document {
            // Whether to load the page at all is decided in `allow_navigation`.
            return match read(&self.settings).strip_tracking_parameters {
                true => {
                    tracking::strip(url, level == Level::Aggressive).map_or(Verdict::Allow, Verdict::Rewrite)
                }
                false => Verdict::Allow,
            };
        }
        let Ok(request) = adblock::request::Request::new(url, page_url, kind.as_str(), "GET") else {
            return Verdict::Allow;
        };
        let result = self.engines().network.check_network_request(&request);
        // Standard leaves first-party requests alone unless a rule insists ($important):
        // blocking a site's own resources breaks it far more often than third-party ones.
        if result.should_block() && (level == Level::Aggressive || request.is_third_party || result.important)
        {
            return Verdict::Block;
        }
        result.rewritten_url.map_or(Verdict::Allow, Verdict::Rewrite)
    }

    /// Whether a top-level navigation may happen. Pages may navigate wherever the user
    /// clicks; without a click they may not go to listed ad and tracker sites (and, on
    /// Aggressive, not to any other site).
    pub fn allow_navigation(&self, nav: &Navigation<'_>) -> bool {
        if nav.browser_initiated && !nav.server_redirect {
            return true;
        }
        let page = if nav.from.is_empty() { nav.to } else { nav.from };
        let level = self.level(page);
        if level == Level::Off || !read(&self.settings).block_redirects || nav.user_gesture {
            return true;
        }
        if !nav.from.is_empty() && self.is_ad_page(nav.to, nav.from) {
            return false;
        }
        let aggressive_block = level == Level::Aggressive
            && !nav.server_redirect
            && !nav.browser_initiated
            && site::is_cross_site(nav.from, nav.to);
        !aggressive_block
    }

    /// Whether a request may send and store cookies: not when it goes to another site than
    /// the page's (third-party cookies follow you across sites).
    pub fn allow_cookies(&self, url: &str, page_url: &str) -> bool {
        if !read(&self.settings).block_third_party_cookies || self.level(page_url) == Level::Off {
            return true;
        }
        !site::is_cross_site(url, page_url)
    }

    /// Whether a page may open a new tab or window. It needs a click or key press, and the
    /// target must not be a listed ad site (ad popups usually come with a click).
    pub fn allow_popup(&self, opener_url: &str, url: &str, user_gesture: bool) -> bool {
        if self.level(opener_url) == Level::Off || !read(&self.settings).block_popups {
            return true;
        }
        user_gesture && !self.is_ad_page(url, opener_url)
    }

    /// JavaScript that hides the page's ad placeholders (cosmetic filters) and runs its
    /// scriptlets, or `None` when there is nothing to do.
    pub fn page_script(&self, url: &str) -> Option<String> {
        if self.level(url) == Level::Off || !read(&self.settings).hide_ad_elements {
            return None;
        }
        let resources = self.engines().network.url_cosmetic_resources(url);
        if resources.hide_selectors.is_empty() && resources.injected_script.is_empty() {
            return None;
        }
        let mut css = String::new();
        // One rule per selector: an invalid selector then only drops its own rule.
        for selector in &resources.hide_selectors {
            css.push_str(selector);
            css.push_str("{display:none!important}\n");
        }
        let mut script = String::new();
        if !css.is_empty() {
            // The script may run before the document has a root element; then it waits for one.
            script.push_str("(function(){var css=");
            script.push_str(&js_string(&css));
            script.push_str(
                ";function add(){var r=document.head||document.documentElement;if(!r)return false;\
                 var s=document.createElement('style');s.id='axion-shields';s.textContent=css;\
                 r.appendChild(s);return true}\
                 if(!add()){var o=new MutationObserver(function(){if(add())o.disconnect()});\
                 o.observe(document,{childList:true,subtree:true})}})();\n",
            );
        }
        if !resources.injected_script.is_empty() {
            script.push_str("try{");
            script.push_str(&resources.injected_script);
            script.push_str("}catch(e){}\n");
        }
        Some(script)
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `s` as a double-quoted JavaScript string literal, safe to embed anywhere.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "||ads.example^\n||tracker.net^$third-party\n/banner/*$image\n\
                         news.site##.ad-slot\n||popads.biz^$popup";

    fn shields() -> Arc<Shields> {
        Shields::with_rules(RULES, Settings::default())
    }

    #[test]
    fn blocks_third_party_ads() {
        let s = shields();
        let page = "https://news.site/article";
        assert_eq!(s.check_request("https://ads.example/x.js", page, ResourceKind::Script), Verdict::Block);
        assert_eq!(s.check_request("https://tracker.net/p.gif", page, ResourceKind::Image), Verdict::Block);
        assert_eq!(s.check_request("https://cdn.site/app.js", page, ResourceKind::Script), Verdict::Allow);
    }

    #[test]
    fn standard_spares_first_party() {
        let s = shields();
        let banner = "https://news.site/banner/top.png";
        assert_eq!(s.check_request(banner, "https://news.site/", ResourceKind::Image), Verdict::Allow);
        s.set_site_level("news.site", Some(Level::Aggressive));
        assert_eq!(s.check_request(banner, "https://news.site/", ResourceKind::Image), Verdict::Block);
    }

    #[test]
    fn off_allows_everything() {
        let s = shields();
        s.set_site_level("news.site", Some(Level::Off));
        let page = "https://www.news.site/";
        assert_eq!(s.level(page), Level::Off);
        assert_eq!(s.check_request("https://ads.example/x.js", page, ResourceKind::Script), Verdict::Allow);
        assert!(s.allow_popup(page, "https://popads.biz/", false));
        assert!(s.page_script(page).is_none());
    }

    #[test]
    fn strips_tracking_from_pages() {
        let s = shields();
        assert_eq!(
            s.check_request("https://shop.com/item?id=4&fbclid=abc", "", ResourceKind::Document),
            Verdict::Rewrite("https://shop.com/item?id=4".into())
        );
    }

    #[test]
    fn popups_need_a_click_and_a_clean_target() {
        let s = shields();
        let page = "https://news.site/";
        assert!(!s.allow_popup(page, "https://other.org/", false));
        assert!(s.allow_popup(page, "https://other.org/", true));
        assert!(!s.allow_popup(page, "https://popads.biz/landing", true));
    }

    #[test]
    fn redirects() {
        let s = shields();
        let nav = |to, user_gesture| Navigation {
            from: "https://news.site/a",
            to,
            user_gesture,
            server_redirect: false,
            browser_initiated: false,
        };
        assert!(!s.allow_navigation(&nav("https://popads.biz/x", false)));
        assert!(s.allow_navigation(&nav("https://popads.biz/x", true)));
        assert!(s.allow_navigation(&nav("https://elsewhere.org/", false)));
        s.set_site_level("news.site", Some(Level::Aggressive));
        assert!(!s.allow_navigation(&nav("https://elsewhere.org/", false)));
        assert!(s.allow_navigation(&nav("https://m.news.site/", false)));
        let typed = Navigation { browser_initiated: true, ..nav("https://popads.biz/", false) };
        assert!(s.allow_navigation(&typed));
    }

    #[test]
    fn third_party_cookies() {
        let s = shields();
        assert!(s.allow_cookies("https://cdn.news.site/x", "https://news.site/"));
        assert!(!s.allow_cookies("https://tracker.net/p", "https://news.site/"));
        s.set_site_level("news.site", Some(Level::Off));
        assert!(s.allow_cookies("https://tracker.net/p", "https://news.site/"));
    }

    #[test]
    fn hides_ad_elements() {
        let s = shields();
        let script = s.page_script("https://news.site/a").unwrap_or_default();
        assert!(script.contains(".ad-slot{display:none!important}"), "{script}");
        assert!(s.page_script("https://other.org/").is_none());
    }

    #[test]
    fn js_strings_are_escaped() {
        let s = js_string("a\"b\\c\n</style>");
        assert!(s.starts_with(r#""a\"b\\c\n"#) && !s.contains('<'), "{s}");
    }
}
