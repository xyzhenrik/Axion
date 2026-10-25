//! Glue between Shields and the browser: the engine's request policy, settings from the
//! config, per-site levels in the profile, and the blocked counters.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axion_config::{Config, ShieldLevel};
use axion_engine::{NavigationRequest, RequestPolicy, RequestVerdict, ResourceKind, ResourceRequest};
use axion_shields::{Level, Navigation, Shields, Verdict};
use axion_store::Store;

/// Key of a site's level in the store's per-site settings.
const SITE_KEY: &str = "shields";
/// Meta key of the lifetime blocked counter.
const TOTAL_KEY: &str = "shields.blocked_total";
/// The total is written after this many new blocks (and on exit).
const SAVE_EVERY: u32 = 50;

pub fn level(level: ShieldLevel) -> Level {
    match level {
        ShieldLevel::Off => Level::Off,
        ShieldLevel::Standard => Level::Standard,
        ShieldLevel::Aggressive => Level::Aggressive,
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Off => "off",
        Level::Standard => "standard",
        Level::Aggressive => "aggressive",
    }
}

fn parse_level(name: &str) -> Option<Level> {
    [Level::Off, Level::Standard, Level::Aggressive].into_iter().find(|l| level_name(*l) == name)
}

fn settings(config: &Config) -> axion_shields::Settings {
    let s = &config.shields;
    axion_shields::Settings {
        default_level: level(s.default_level),
        hide_ad_elements: s.hide_ad_elements,
        block_popups: s.block_popups,
        block_redirects: s.block_redirects,
        strip_tracking_parameters: s.strip_tracking_parameters,
        block_third_party_cookies: s.block_third_party_cookies,
        lists: s.lists.clone(),
        update_interval: Duration::from_secs_f32(s.update_interval_hours.max(1.0) * 3600.0),
    }
}

/// Lets Shields decide the engine's requests.
struct Policy(Arc<Shields>);

impl RequestPolicy for Policy {
    fn check_request(&self, request: &ResourceRequest<'_>) -> RequestVerdict {
        let kind = match request.kind {
            ResourceKind::Document => axion_shields::ResourceKind::Document,
            ResourceKind::Subdocument => axion_shields::ResourceKind::Subdocument,
            ResourceKind::Stylesheet => axion_shields::ResourceKind::Stylesheet,
            ResourceKind::Script => axion_shields::ResourceKind::Script,
            ResourceKind::Image => axion_shields::ResourceKind::Image,
            ResourceKind::Font => axion_shields::ResourceKind::Font,
            ResourceKind::Media => axion_shields::ResourceKind::Media,
            ResourceKind::Object => axion_shields::ResourceKind::Object,
            ResourceKind::Xhr => axion_shields::ResourceKind::Xhr,
            ResourceKind::Ping => axion_shields::ResourceKind::Ping,
            ResourceKind::Other => axion_shields::ResourceKind::Other,
        };
        match self.0.check_request(request.url, request.page_url, kind) {
            Verdict::Allow => RequestVerdict::Allow,
            Verdict::Block => RequestVerdict::Block,
            Verdict::Rewrite(url) => RequestVerdict::Redirect(url),
        }
    }

    fn allow_cookies(&self, url: &str, page_url: &str) -> bool {
        self.0.allow_cookies(url, page_url)
    }

    fn allow_navigation(&self, n: &NavigationRequest<'_>) -> bool {
        self.0.allow_navigation(&Navigation {
            from: n.from,
            to: n.to,
            user_gesture: n.user_gesture,
            server_redirect: n.server_redirect,
            browser_initiated: n.browser_initiated,
        })
    }

    fn page_script(&self, url: &str) -> Option<String> {
        self.0.page_script(url)
    }
}

pub struct ShieldsState {
    shields: Arc<Shields>,
    total: u64,
    unsaved: u32,
}

impl ShieldsState {
    /// Loads the filter cache from `<profile>/shields` and the per-site levels from the store.
    pub fn new(profile: &Path, config: &Config, store: Option<&Store>) -> Self {
        let shields = Shields::new(profile.join("shields"), settings(config));
        let mut total = 0;
        if let Some(store) = store {
            let levels = store.site_settings(SITE_KEY).unwrap_or_default();
            shields.set_site_levels(levels.into_iter().filter_map(|(h, v)| Some((h, parse_level(&v)?))));
            total = store.meta(TOTAL_KEY).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        }
        Self { shields, total, unsaved: 0 }
    }

    pub fn policy(&self) -> Arc<dyn RequestPolicy> {
        Arc::new(Policy(Arc::clone(&self.shields)))
    }

    pub fn apply_config(&self, config: &Config) {
        self.shields.set_settings(settings(config));
    }

    pub fn update_lists(&self) {
        self.shields.update_now();
    }

    pub fn is_ready(&self) -> bool {
        self.shields.is_ready()
    }

    pub fn allow_popup(&self, opener_url: &str, url: &str, user_gesture: bool) -> bool {
        self.shields.allow_popup(opener_url, url, user_gesture)
    }

    /// The level on the page's site.
    pub fn level(&self, page_url: &str) -> Level {
        self.shields.level(page_url)
    }

    /// Gives the page's site its own level (remembered in the profile).
    pub fn set_level(&self, store: Option<&Store>, page_url: &str, level: Level, config: &Config) {
        let Some(host) = axion_shields::site::host(page_url) else { return };
        // A level equal to the default isn't stored, so changing the default still reaches it.
        let own = (level != self::level(config.shields.default_level)).then_some(level);
        self.shields.set_site_level(&host, own);
        if let Some(store) = store
            && let Err(e) = store.set_site_setting(&host, SITE_KEY, own.map(level_name))
        {
            tracing::warn!("saving the Shields level failed: {e}");
        }
    }

    pub fn count_blocked(&mut self, store: Option<&Store>) {
        self.total += 1;
        self.unsaved += 1;
        if self.unsaved >= SAVE_EVERY {
            self.save(store);
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn save(&mut self, store: Option<&Store>) {
        if self.unsaved == 0 {
            return;
        }
        self.unsaved = 0;
        if let Some(store) = store
            && let Err(e) = store.set_meta(TOTAL_KEY, Some(&self.total.to_string()))
        {
            tracing::warn!("saving the Shields counter failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_names_round_trip() {
        for l in [Level::Off, Level::Standard, Level::Aggressive] {
            assert_eq!(parse_level(level_name(l)), Some(l));
        }
    }

    #[test]
    fn site_levels_persist() {
        let store = Store::in_memory().unwrap();
        let config = Config::default();
        let dir = std::env::temp_dir().join(format!("axion-shields-app-{}", std::process::id()));
        let mut empty = config.clone();
        empty.shields.lists.clear();
        let state = ShieldsState::new(&dir, &empty, Some(&store));
        state.set_level(Some(&store), "https://www.news.site/a", Level::Off, &config);
        assert_eq!(state.level("https://news.site/b"), Level::Off);
        let again = ShieldsState::new(&dir, &empty, Some(&store));
        assert_eq!(again.level("https://news.site/"), Level::Off);
        // Back to the default: nothing stored.
        again.set_level(Some(&store), "https://news.site/", Level::Standard, &config);
        assert!(store.site_settings(SITE_KEY).unwrap().is_empty());
    }
}
