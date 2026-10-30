//! Importing bookmarks and history from other browsers, the app's side: looking for the
//! browsers and importing on background threads (Settings › Data & Downloads and the
//! first-run introduction share this), the file chooser for bookmark files, and the texts that
//! report the result. The reading itself is `axion-import`.
//!
//! The import writes into the store of the account that is open, through a second connection
//! of its own, never into another account's or a private session's data.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axion_config::Config;
use axion_i18n::{tr, tr_fmt, tr_mark};
use axion_import::{Home, Limits, Report, Source, Wanted};
use axion_store::Store;

use crate::downloads::{Choose, choose_portal, spawn};

const MB: u64 = 1024 * 1024;

/// The caps from `import.*` in the configuration.
pub fn limits(config: &Config) -> Limits {
    let c = &config.import;
    Limits {
        max_history_items: c.max_history_items as usize,
        max_bookmarks: c.max_bookmarks as usize,
        max_db_bytes: u64::from(c.max_database_mb) * MB,
        max_text_bytes: u64::from(c.max_file_mb) * MB,
        favicons: c.favicons,
        max_favicon_bytes: c.max_favicon_bytes as usize,
        max_favicons: c.max_favicons as usize,
    }
}

/// What a finished import reports.
#[derive(Clone, Debug)]
pub struct Finished {
    pub result: Result<Report, String>,
    pub wanted: Wanted,
}

/// What background work finished.
pub enum Event {
    /// The browsers on this machine, with what they hold.
    Scanned(Vec<Source>),
    Finished(Finished),
    /// A bookmarks file was chosen.
    FileChosen(PathBuf),
}

#[derive(Default)]
struct Jobs {
    scanned: Option<Vec<Source>>,
    finished: Option<Finished>,
    file: Option<PathBuf>,
}

pub struct Importer {
    jobs: Arc<Mutex<Jobs>>,
    /// The account's store file; imports go here.
    store_path: Option<PathBuf>,
    /// The account's site icon cache folder (`<profile>/favicons`); imported icons go here.
    icon_dir: Option<PathBuf>,
    home: Option<Home>,
    /// The browsers found by the last scan (what Settings lists).
    sources: Vec<Source>,
    /// The browsers the introduction offered (found by [`Self::offer`]).
    offered: Vec<Source>,
    scanning: bool,
    running: bool,
}

impl Importer {
    /// `store_path`: the open account's `axion.sqlite`, or `None` when it could not be opened.
    pub fn new(store_path: Option<PathBuf>, icon_dir: Option<PathBuf>) -> Self {
        Self {
            jobs: Arc::default(),
            store_path,
            icon_dir,
            home: Home::from_env(),
            sources: Vec::new(),
            offered: Vec::new(),
            scanning: false,
            running: false,
        }
    }

    /// Whether imports can be written at all.
    pub fn available(&self) -> bool {
        self.store_path.is_some()
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// Looks for the browsers on this machine by looking at the file system only (fast; no
    /// counts). The first-run introduction asks this when it opens.
    pub fn offer(&mut self) -> &[Source] {
        self.offered = self.home.as_ref().map(axion_import::detect).unwrap_or_default();
        &self.offered
    }

    /// The browsers [`Self::offer`] found last.
    pub fn offered(&self) -> &[Source] {
        &self.offered
    }

    /// Looks for browsers and counts what they hold, on a thread; the answer arrives as
    /// [`Event::Scanned`].
    pub fn scan(&mut self, config: &Config) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let jobs = Arc::clone(&self.jobs);
        let home = self.home.clone();
        let limits = limits(config);
        spawn("import-scan", move || {
            let sources = home.map(|h| axion_import::scan(&h, &limits)).unwrap_or_default();
            if let Ok(mut jobs) = jobs.lock() {
                jobs.scanned = Some(sources);
            }
        });
    }

    /// Imports `wanted` from these browser profiles, on a thread (with their site icons when
    /// `icons`); the result arrives as [`Event::Finished`].
    pub fn run(&mut self, sources: Vec<Source>, wanted: Wanted, icons: bool, config: &Config) {
        let limits = Limits { favicons: icons, ..limits(config) };
        let icon_dir = self.icon_dir.clone();
        self.start(wanted, move |store| {
            let mut total = Report::default();
            let mut first_error = None;
            let mut worked = false;
            for source in &sources {
                let imported = axion_import::read(source, wanted, &limits)
                    .and_then(|data| apply_all(store, icon_dir.as_deref(), &data));
                match imported {
                    Ok(report) => {
                        worked = true;
                        total.merge(report);
                    }
                    Err(e) => {
                        tracing::warn!(browser = %source.browser, profile = %source.profile, "import failed: {e}");
                        first_error.get_or_insert(e.to_string());
                    }
                }
            }
            match first_error {
                Some(error) if !worked => Err(error),
                _ => Ok(total),
            }
        });
    }

    /// Imports the bookmarks of an exported file.
    pub fn run_file(&mut self, path: PathBuf, config: &Config) {
        let limits = limits(config);
        let icon_dir = self.icon_dir.clone();
        self.start(Wanted { bookmarks: true, history: false }, move |store| {
            let data = axion_import::read_file(&path, &limits).map_err(|e| e.to_string())?;
            apply_all(store, icon_dir.as_deref(), &data).map_err(|e| e.to_string())
        });
    }

    fn start(
        &mut self,
        wanted: Wanted,
        work: impl FnOnce(&Store) -> Result<Report, String> + Send + 'static,
    ) {
        let Some(path) = self.store_path.clone() else { return };
        if self.running {
            return;
        }
        self.running = true;
        let jobs = Arc::clone(&self.jobs);
        spawn("import", move || {
            let result = Store::open(&path).map_err(|e| e.to_string()).and_then(|store| work(&store));
            if let Ok(mut jobs) = jobs.lock() {
                jobs.finished = Some(Finished { result, wanted });
            }
        });
    }

    /// Asks for a bookmarks file with the desktop's file chooser; the choice arrives as
    /// [`Event::FileChosen`].
    pub fn choose_file(&self) {
        let jobs = Arc::clone(&self.jobs);
        spawn("choose-bookmarks", move || match choose_portal(tr("Import Bookmarks"), Choose::Bookmarks) {
            Ok(Some(path)) => {
                if let Ok(mut jobs) = jobs.lock() {
                    jobs.file = Some(path);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("file chooser unavailable: {e}"),
        });
    }

    /// Results of background work since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let Ok(mut jobs) = self.jobs.lock() else { return Vec::new() };
        let mut events = Vec::new();
        if let Some(sources) = jobs.scanned.take() {
            self.scanning = false;
            self.sources.clone_from(&sources);
            events.push(Event::Scanned(sources));
        }
        if let Some(finished) = jobs.finished.take() {
            self.running = false;
            events.push(Event::Finished(finished));
        }
        if let Some(path) = jobs.file.take() {
            events.push(Event::FileChosen(path));
        }
        events
    }
}

/// Writes what was read: bookmarks and history into the store, site icons into the icon cache
/// folder (a host that has an icon keeps it).
fn apply_all(
    store: &Store,
    icon_dir: Option<&std::path::Path>,
    data: &axion_import::Imported,
) -> axion_import::Result<Report> {
    let mut report = axion_import::apply(store, data, unix_now())?;
    if let Some(dir) = icon_dir {
        report.icons_added = axion_import::write_icons(dir, &data.icons).added;
    }
    Ok(report)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// `1234567` as "1,234,567" (German: "1.234.567").
pub fn group_digits(n: usize) -> String {
    let separator = match axion_i18n::language() {
        axion_i18n::Language::German => '.',
        axion_i18n::Language::English => ',',
    };
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(c);
    }
    out
}

/// "1 bookmark" / "3 bookmarks" with grouped digits; `one` and `other` are catalog keys
/// containing `{n}` (declare them with `tr_mark` where this is called).
pub fn counted(one: &'static str, other: &'static str, n: usize) -> String {
    let template = tr(if n == 1 { one } else { other });
    axion_i18n::format_named(template, &[("n", &group_digits(n))])
}

pub fn bookmarks_text(n: usize) -> String {
    counted(tr_mark("{n} bookmark"), tr_mark("{n} bookmarks"), n)
}

pub fn history_text(n: usize) -> String {
    counted(tr_mark("{n} history entry"), tr_mark("{n} history entries"), n)
}

pub fn icons_text(n: usize) -> String {
    counted(tr_mark("{n} site icon"), tr_mark("{n} site icons"), n)
}

/// "Imported 312 bookmarks, 8,450 history entries and 120 site icons; 40 already existed" for
/// a finished import (only the kinds that were asked for are named; icons when some arrived).
pub fn summary(report: &Report, wanted: Wanted) -> String {
    let mut parts: Vec<String> = Vec::new();
    // Zero counts are only named when nothing else is: with icons they would be noise.
    let named = |n: usize| n > 0 || report.icons_added == 0;
    if wanted.bookmarks && named(report.bookmarks_added) {
        parts.push(bookmarks_text(report.bookmarks_added));
    }
    if wanted.history && named(report.history_added) {
        parts.push(history_text(report.history_added));
    }
    if report.icons_added > 0 {
        parts.push(icons_text(report.icons_added));
    }
    let nothing_new = (!wanted.bookmarks || report.bookmarks_added == 0)
        && (!wanted.history || report.history_added == 0)
        && report.icons_added == 0;
    let existing = report.existing();
    if nothing_new {
        return if existing == 0 {
            tr("Nothing to import").to_owned()
        } else {
            tr_fmt!("Nothing new to import; {count} already existed", count = group_digits(existing))
        };
    }
    let items = match parts.as_slice() {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => tr_fmt!("{first} and {second}", first = a, second = b),
        [a, b, c, ..] => tr_fmt!("{first}, {second} and {third}", first = a, second = b, third = c),
    };
    let base = tr_fmt!("Imported {items}", items = items);
    if existing == 0 {
        base
    } else {
        tr_fmt!("{summary}; {count} already existed", summary = base, count = group_digits(existing))
    }
}

/// The short form for a toast (no "already existed" part).
pub fn toast_text(report: &Report, wanted: Wanted) -> String {
    let short = Report { bookmarks_existing: 0, history_existing: 0, ..report.clone() };
    summary(&short, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(8450), "8,450");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    fn report(bookmarks_added: usize, history_added: usize, existing: usize) -> Report {
        Report { bookmarks_added, history_added, bookmarks_existing: existing, ..Report::default() }
    }

    #[test]
    fn summaries_name_what_was_imported() {
        let both = Wanted::ALL;
        assert_eq!(
            summary(&report(312, 8450, 40), both),
            "Imported 312 bookmarks and 8,450 history entries; 40 already existed"
        );
        assert_eq!(summary(&report(1, 1, 0), both), "Imported 1 bookmark and 1 history entry");
        let bookmarks_only = Wanted { bookmarks: true, history: false };
        assert_eq!(summary(&report(5, 0, 0), bookmarks_only), "Imported 5 bookmarks");
        let history_only = Wanted { bookmarks: false, history: true };
        assert_eq!(summary(&report(0, 2, 0), history_only), "Imported 2 history entries");
        assert_eq!(summary(&report(0, 0, 0), both), "Nothing to import");
        assert_eq!(summary(&report(0, 0, 7), both), "Nothing new to import; 7 already existed");
        assert_eq!(toast_text(&report(3, 4, 9), both), "Imported 3 bookmarks and 4 history entries");
        let with_icons = |r: Report, icons| Report { icons_added: icons, ..r };
        assert_eq!(
            summary(&with_icons(report(312, 8450, 40), 120), both),
            "Imported 312 bookmarks, 8,450 history entries and 120 site icons; 40 already existed"
        );
        assert_eq!(summary(&with_icons(report(0, 0, 0), 1), both), "Imported 1 site icon");
        // Only icons were new (the entries were imported before).
        let bookmarks_only = Wanted { bookmarks: true, history: false };
        assert_eq!(
            summary(&with_icons(report(0, 0, 7), 2), bookmarks_only),
            "Imported 2 site icons; 7 already existed"
        );
    }

    #[test]
    fn limits_follow_the_configuration() {
        let mut config = Config::default();
        config.import.max_history_items = 5;
        config.import.max_database_mb = 2;
        let limits = limits(&config);
        assert_eq!(limits.max_history_items, 5);
        assert_eq!(limits.max_db_bytes, 2 * MB);
    }
}
