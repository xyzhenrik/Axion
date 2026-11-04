//! What the start page shows, as plain data: the model both front ends (the HTML page's JSON and
//! the native page) are built from, plus the pure helpers the native page needs (greeting,
//! clock text, Shields number formats, paging).

use axion_config::{ClockFormat, Color, Config};
use axion_store::site_host;

use super::backgrounds::Kind;
use super::{BYTES_PER_BLOCKED_REQUEST, SECONDS_PER_BLOCKED_REQUEST, Site};
use crate::settings::SEARCH_ENGINES;

/// One search engine of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineModel {
    pub name: &'static str,
    /// Its site, whose cached icon stands for it.
    pub host: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchModel {
    pub show: bool,
    pub engines: Vec<EngineModel>,
    /// Index into `engines`; `None` for a custom search address.
    pub current: Option<usize>,
    pub custom_host: String,
}

impl SearchModel {
    /// "DuckDuckGo", or the custom engine's host.
    pub fn engine_name(&self) -> Option<String> {
        match self.current {
            Some(i) => self.engines.get(i).map(|e| e.name.to_owned()),
            None => Some(self.custom_host.clone()).filter(|h| !h.is_empty()),
        }
    }

    /// The host whose icon the engine button shows.
    pub fn icon_host(&self) -> &str {
        match self.current.and_then(|i| self.engines.get(i)) {
            Some(engine) => &engine.host,
            None => &self.custom_host,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SitesModel {
    pub show: bool,
    pub columns: u32,
    pub rows: u32,
    /// How many tiles at most (the Add Site tile shows while there are fewer).
    pub count: u32,
    pub sites: Vec<Site>,
    /// Removed sites that Restore brings back.
    pub hidden: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatsModel {
    pub show: bool,
    pub blocked: u64,
    pub bytes: u64,
    pub seconds: f64,
}

/// One background of the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundEntry {
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundModel {
    /// The selected background (it exists), if any and not private.
    pub selected: Option<BackgroundEntry>,
    pub list: Vec<BackgroundEntry>,
    /// `home.live_wallpapers` itself.
    pub live_setting: bool,
    /// Animated images and videos play (see `wallpapers_animated`).
    pub live: bool,
    pub can_add: bool,
    pub dim: f32,
    pub blur: f32,
    /// Why the last file could not be added (empty: nothing to report).
    pub error: String,
}

/// Everything the start page shows.
#[derive(Clone, Debug, PartialEq)]
pub struct StartModel {
    pub private: bool,
    pub accent: Color,
    pub clock_show: bool,
    /// The clock shows 24 hours.
    pub clock_24h: bool,
    pub clock_format: ClockFormat,
    pub greeting_show: bool,
    /// The first word of the account's name.
    pub greeting_name: String,
    pub search: SearchModel,
    pub sites: SitesModel,
    pub stats: StatsModel,
    pub background: BackgroundModel,
}

/// The library's view for [`StartModel::build`].
pub struct BackgroundFacts<'a> {
    pub selected: Option<&'a str>,
    pub entries: Vec<BackgroundEntry>,
    pub live: bool,
    pub error: &'a str,
}

impl StartModel {
    /// Private start pages show no top sites, history, Shields numbers or background. Reads the
    /// top sites from `store` (the browser passes a cached reading to [`StartModel::with_sites`]).
    #[cfg(test)]
    pub fn build(
        config: &Config,
        store: Option<&axion_store::Store>,
        blocked: u64,
        account_name: &str,
        private: bool,
        backgrounds: &BackgroundFacts<'_>,
        now: i64,
    ) -> Self {
        let home = &config.home;
        let sites = store.filter(|_| !private).map(|store| {
            super::top_sites(store, home.top_sites_count as usize, home.top_sites_window_days, now)
        });
        Self::with_sites(config, sites, blocked, account_name, private, backgrounds)
    }

    /// The model with the top sites read already (and how many were removed). Private start
    /// pages show no top sites, history, Shields numbers or background.
    pub fn with_sites(
        config: &Config,
        sites: Option<(Vec<Site>, usize)>,
        blocked: u64,
        account_name: &str,
        private: bool,
        backgrounds: &BackgroundFacts<'_>,
    ) -> Self {
        let home = &config.home;
        let clock_24h = match home.clock_format {
            ClockFormat::Auto => !super::locale_uses_12h(),
            ClockFormat::H12 => false,
            ClockFormat::H24 => true,
        };
        let engines = SEARCH_ENGINES
            .iter()
            .map(|(name, url)| EngineModel { name, host: site_host(url).unwrap_or_default() })
            .collect();
        let current = SEARCH_ENGINES.iter().position(|(_, url)| *url == config.general.search_url);
        let (sites, hidden) = sites.filter(|_| !private).unwrap_or_default();
        let selected = backgrounds
            .selected
            .and_then(|name| backgrounds.entries.iter().find(|e| e.name == name))
            .filter(|_| !private)
            .cloned();
        let (list, live, error) = if private {
            (Vec::new(), false, String::new())
        } else {
            (backgrounds.entries.clone(), backgrounds.live, backgrounds.error.to_owned())
        };
        Self {
            private,
            accent: config.appearance.accent,
            clock_show: home.show_clock,
            clock_24h,
            clock_format: home.clock_format,
            greeting_show: home.show_greeting && !private,
            greeting_name: account_name.split_whitespace().next().unwrap_or_default().to_owned(),
            search: SearchModel {
                show: home.show_search,
                engines,
                current,
                custom_host: site_host(&config.general.search_url).unwrap_or_default(),
            },
            sites: SitesModel {
                show: home.show_top_sites && !private,
                columns: home.top_sites_columns.max(1),
                rows: home.top_sites_rows.max(1),
                count: home.top_sites_count,
                sites,
                hidden,
            },
            stats: StatsModel {
                show: home.show_stats && !private,
                blocked: if private { 0 } else { blocked },
                bytes: if private { 0 } else { blocked.saturating_mul(BYTES_PER_BLOCKED_REQUEST) },
                seconds: if private { 0.0 } else { blocked as f64 * SECONDS_PER_BLOCKED_REQUEST },
            },
            background: BackgroundModel {
                selected,
                can_add: backgrounds.entries.len()
                    < (home.max_backgrounds as usize).min(crate::settings::BACKGROUND_SLOTS),
                list,
                live_setting: home.live_wallpapers,
                live,
                dim: home.background_dim,
                blur: home.background_blur,
                error,
            },
        }
    }
}

// ---- clock and greeting ----

/// "Good morning" (5-12 h), "Good afternoon" (12-18 h) or "Good evening", with the name when
/// there is one.
pub fn greeting_for_hour(hour: u32, name: &str) -> String {
    use axion_i18n::{tr, tr_fmt};
    let named = !name.is_empty();
    if (5..12).contains(&hour) {
        if named { tr_fmt!("Good morning, {name}", name = name) } else { tr("Good morning").to_owned() }
    } else if (12..18).contains(&hour) {
        if named { tr_fmt!("Good afternoon, {name}", name = name) } else { tr("Good afternoon").to_owned() }
    } else if named {
        tr_fmt!("Good evening, {name}", name = name)
    } else {
        tr("Good evening").to_owned()
    }
}

/// The clock's digits for `hour:minute`, and the AM/PM mark in 12-hour mode.
pub fn clock_text(hour: u32, minute: u32, h24: bool) -> (String, Option<&'static str>) {
    if h24 {
        (format!("{hour:02}:{minute:02}"), None)
    } else {
        let h = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
        let mark = if hour < 12 { axion_i18n::tr("AM") } else { axion_i18n::tr("PM") };
        (format!("{h}:{minute:02}"), Some(mark))
    }
}

// ---- Shields numbers ----

/// The group and decimal separators of the language.
fn separators(tag: &str) -> (&'static str, &'static str) {
    match tag.split('-').next().unwrap_or(tag) {
        "de" | "es" | "it" | "nl" | "pt" | "tr" => (".", ","),
        "fr" | "ru" | "pl" => ("\u{202f}", ","),
        _ => (",", "."),
    }
}

/// A number with `digits` decimals and the language's separators.
pub fn format_number(value: f64, digits: usize, tag: &str) -> String {
    let (group, decimal) = separators(tag);
    let text = format!("{:.*}", digits, value.abs());
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push_str(group);
        }
        grouped.push(c);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    if frac.is_empty() { format!("{sign}{grouped}") } else { format!("{sign}{grouped}{decimal}{frac}") }
}

/// "12,345".
pub fn format_count(n: u64, tag: &str) -> String {
    format_number(n as f64, 0, tag)
}

/// "512 bytes", "48 KB", "1.5 MB".
pub fn format_bytes(bytes: u64, tag: &str) -> String {
    let units = [axion_i18n::tr("bytes"), "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut i = 0;
    while value >= 1024.0 && i < units.len() - 1 {
        value /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", format_number(value.round(), 0, tag), units[0])
    } else if value < 10.0 {
        format!("{} {}", format_number(value, 1, tag), units[i])
    } else {
        format!("{} {}", format_number(value.round(), 0, tag), units[i])
    }
}

/// "3 s", "12 min", "1.5 h", "2 days".
pub fn format_seconds(seconds: f64, tag: &str) -> String {
    use axion_i18n::tr_fmt;
    let n = |v: f64, digits| format_number(v, digits, tag);
    if seconds < 60.0 {
        tr_fmt!("{n} s", n = n(seconds.round(), 0))
    } else if seconds < 3600.0 {
        tr_fmt!("{n} min", n = n((seconds / 60.0).round(), 0))
    } else if seconds < 86_400.0 {
        let h = seconds / 3600.0;
        tr_fmt!("{n} h", n = if h < 10.0 { n(h, 1) } else { n(h.round(), 0) })
    } else {
        let d = seconds / 86_400.0;
        tr_fmt!("{n} days", n = if d < 10.0 { n(d, 1) } else { n(d.round(), 0) })
    }
}

// ---- paging ----

/// Tiles per row: as many as fit `width` (logical px) at `cell` px each, between 2 and `max`.
pub fn columns_for(width: f32, cell: f32, max: u32) -> u32 {
    let fit = (width / cell).floor().max(0.0) as u32;
    fit.clamp(2, max.max(2))
}

/// How many pages `items` tiles (including the Add Site tile) take.
pub fn page_count(items: usize, per_page: usize) -> usize {
    items.div_ceil(per_page.max(1)).max(1)
}

/// The monogram's colour for a host: a stable hue from its name (saturation and lightness fixed).
pub fn monogram_color(host: &str) -> Color {
    let mut hash: u32 = 2_166_136_261;
    for b in host.bytes() {
        hash = (hash ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    let hue = (hash % 360) as f32;
    hsl(hue, 0.55, 0.42)
}

fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::rgb(u32::from(byte(r)) << 16 | u32::from(byte(g)) << 8 | u32::from(byte(b)))
}

/// The monogram letter: the host's first letter (without `www.`), upper case.
pub fn monogram(host: &str) -> String {
    host.trim_start_matches("www.").chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axion_store::Store;

    #[test]
    fn greeting_follows_the_hour() {
        assert_eq!(greeting_for_hour(4, ""), "Good evening");
        assert_eq!(greeting_for_hour(5, ""), "Good morning");
        assert_eq!(greeting_for_hour(11, "Ada"), "Good morning, Ada");
        assert_eq!(greeting_for_hour(12, ""), "Good afternoon");
        assert_eq!(greeting_for_hour(17, ""), "Good afternoon");
        assert_eq!(greeting_for_hour(18, "Ada"), "Good evening, Ada");
    }

    #[test]
    fn clock_text_in_both_formats() {
        assert_eq!(clock_text(0, 5, true), ("00:05".into(), None));
        assert_eq!(clock_text(0, 5, false), ("12:05".into(), Some("AM")));
        assert_eq!(clock_text(13, 30, false), ("1:30".into(), Some("PM")));
        assert_eq!(clock_text(12, 0, false), ("12:00".into(), Some("PM")));
    }

    #[test]
    fn shields_numbers() {
        assert_eq!(format_count(1_234_567, "en"), "1,234,567");
        assert_eq!(format_count(1_234_567, "de"), "1.234.567");
        assert_eq!(format_bytes(512, "en"), "512 bytes");
        assert_eq!(format_bytes(1536, "en"), "1.5 KB");
        assert_eq!(format_bytes(48 * 1024, "en"), "48 KB");
        assert_eq!(format_bytes(1536, "de"), "1,5 KB");
        assert_eq!(format_seconds(3.0, "en"), "3 s");
        assert_eq!(format_seconds(720.0, "en"), "12 min");
        assert_eq!(format_seconds(5400.0, "en"), "1.5 h");
        // Like the HTML page (`toLocaleString` with one fixed decimal).
        assert_eq!(format_seconds(3.0 * 86_400.0, "en"), "3.0 days");
        assert_eq!(format_seconds(12.0 * 86_400.0, "en"), "12 days");
    }

    #[test]
    fn paging_math() {
        // The HTML page: clamp(floor(min(880, innerWidth - 48) / 118), 2, columns).
        assert_eq!(columns_for(880.0, 118.0, 6), 6);
        assert_eq!(columns_for(400.0, 118.0, 6), 3);
        assert_eq!(columns_for(100.0, 118.0, 6), 2);
        assert_eq!(columns_for(2000.0, 118.0, 4), 4);
        assert_eq!(page_count(0, 12), 1);
        assert_eq!(page_count(12, 12), 1);
        assert_eq!(page_count(13, 12), 2);
        assert_eq!(page_count(5, 0), 5);
    }

    #[test]
    fn monograms_are_stable() {
        assert_eq!(monogram("www.example.com"), "E");
        assert_eq!(monogram(""), "");
        assert_eq!(monogram_color("example.com"), monogram_color("example.com"));
        assert_ne!(monogram_color("example.com"), monogram_color("rust-lang.org"));
    }

    #[test]
    fn private_model_has_nothing_personal() {
        let store = Store::in_memory().unwrap();
        store.record_visit("https://often.example/", "Often", 100).unwrap();
        let config = Config::default();
        let entry = BackgroundEntry { name: "bg-1.png".into(), kind: Kind::Image };
        let facts =
            BackgroundFacts { selected: Some("bg-1.png"), entries: vec![entry], live: true, error: "x" };
        let model = StartModel::build(&config, Some(&store), 7, "Ada Lovelace", true, &facts, 100);
        assert!(model.sites.sites.is_empty() && !model.sites.show);
        assert!(!model.stats.show && model.stats.blocked == 0);
        assert!(model.background.selected.is_none() && model.background.list.is_empty());
        assert!(!model.greeting_show);
        let model = StartModel::build(&config, Some(&store), 7, "Ada Lovelace", false, &facts, 100);
        assert_eq!(model.sites.sites[0].host, "often.example");
        assert_eq!(model.greeting_name, "Ada");
        assert_eq!(model.stats.bytes, 7 * BYTES_PER_BLOCKED_REQUEST);
        assert_eq!(model.background.selected.as_ref().map(|e| e.name.as_str()), Some("bg-1.png"));
    }
}
