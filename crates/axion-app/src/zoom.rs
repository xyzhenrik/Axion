//! Page zoom, remembered per site. Zoom is kept as Chromium's zoom level (0 = 100 %, each level
//! is x1.2); a site's own level lives in the profile's site settings (`axion-store`, key
//! `zoom`) and applies to every page of that site, in every tab. Sites without one get
//! `zoom.default_percent`. Private tabs read the remembered zoom but never write it.

use std::collections::HashMap;

use axion_config::Config;
use axion_store::Store;

const SITE_KEY: &str = "zoom";
/// Chromium's zoom levels are powers of this factor.
const BASE: f64 = 1.2;
/// Levels closer than this are the same zoom.
const EPSILON: f64 = 0.01;

/// The zoom as a percentage.
pub fn percent(level: f64) -> f64 {
    BASE.powf(level) * 100.0
}

/// The Chromium level of a percentage.
pub fn level_of_percent(percent: f64) -> f64 {
    (percent.max(1.0) / 100.0).ln() / BASE.ln()
}

/// "125%".
pub fn label(level: f64) -> String {
    format!("{:.0}%", percent(level))
}

#[derive(Default)]
pub struct SiteZoom {
    levels: HashMap<String, f64>,
}

impl SiteZoom {
    pub fn load(store: Option<&Store>) -> Self {
        let levels = store
            .and_then(|s| s.site_settings(SITE_KEY).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(host, value)| Some((host, value.parse::<f64>().ok()?)))
            .collect();
        Self { levels }
    }

    /// The level every page without a zoom of its own gets.
    pub fn default_level(config: &Config) -> f64 {
        level_of_percent(config.zoom.default_percent)
    }

    /// The zoom of the page at `url`: its site's own, else the default.
    pub fn level(&self, config: &Config, url: &str) -> f64 {
        let own = config
            .zoom
            .remember_per_site
            .then(|| axion_shields::site::host(url))
            .flatten()
            .and_then(|host| self.levels.get(&host).copied());
        own.unwrap_or_else(|| Self::default_level(config))
    }

    /// The level one step up or down (`direction` 1 / -1) from `current`, within the limits.
    pub fn stepped(config: &Config, current: f64, direction: i32) -> f64 {
        let (min, max) =
            (level_of_percent(config.zoom.min_percent), level_of_percent(config.zoom.max_percent));
        // Snaps to the step grid so repeated steps land on the same stops.
        let step = config.zoom.step.max(0.05);
        let dir = f64::from(direction.signum());
        let snapped = (current / step).round() * step;
        let next = if (snapped - current).abs() > EPSILON && (snapped - current).signum() == dir {
            snapped
        } else {
            snapped + dir * step
        };
        next.clamp(min, max)
    }

    /// Remembers `level` for the site of `url` (a level equal to the default is forgotten, so
    /// changing the default still reaches the site). `persist` is false for private tabs.
    pub fn set(&mut self, store: Option<&Store>, config: &Config, url: &str, level: f64, persist: bool) {
        if !config.zoom.remember_per_site || !persist {
            return;
        }
        let Some(host) = axion_shields::site::host(url) else { return };
        let own = ((level - Self::default_level(config)).abs() > EPSILON).then_some(level);
        match own {
            Some(level) => self.levels.insert(host.clone(), level),
            None => self.levels.remove(&host),
        };
        if let Some(store) = store
            && let Err(e) = store.set_site_setting(&host, SITE_KEY, own.map(|l| format!("{l:.4}")).as_deref())
        {
            tracing::warn!("saving the zoom failed: {e}");
        }
    }

    /// Whether `level` is the default zoom.
    pub fn is_default(config: &Config, level: f64) -> bool {
        (level - Self::default_level(config)).abs() <= EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_round_trips() {
        assert_eq!(label(0.0), "100%");
        assert!((level_of_percent(percent(1.5)) - 1.5).abs() < 1e-9);
        assert_eq!(label(level_of_percent(150.0)), "150%");
    }

    #[test]
    fn steps_stay_in_limits_and_on_the_grid() {
        let config = Config::default();
        assert_eq!(SiteZoom::stepped(&config, 0.0, 1), 0.5);
        assert_eq!(SiteZoom::stepped(&config, 0.5, -1), 0.0);
        assert_eq!(SiteZoom::stepped(&config, 0.2, 1), 0.5);
        assert_eq!(SiteZoom::stepped(&config, 0.2, -1), 0.0);
        let max = level_of_percent(config.zoom.max_percent);
        assert_eq!(SiteZoom::stepped(&config, max, 1), max);
        let min = level_of_percent(config.zoom.min_percent);
        assert_eq!(SiteZoom::stepped(&config, min, -1), min);
    }

    #[test]
    fn site_levels_are_per_host_and_default_is_not_stored() {
        let config = Config::default();
        let store = Store::in_memory().unwrap();
        let mut zoom = SiteZoom::default();
        zoom.set(Some(&store), &config, "https://a.com/x", 1.0, true);
        assert_eq!(zoom.level(&config, "https://a.com/y"), 1.0);
        assert_eq!(zoom.level(&config, "https://b.com/"), 0.0);
        assert_eq!(SiteZoom::load(Some(&store)).level(&config, "https://a.com/"), 1.0);
        zoom.set(Some(&store), &config, "https://a.com/", 0.0, true);
        assert!(store.site_settings(SITE_KEY).unwrap().is_empty());
        // Private tabs never write.
        zoom.set(Some(&store), &config, "https://c.com/", 1.0, false);
        assert_eq!(zoom.level(&config, "https://c.com/"), 0.0);
    }
}
