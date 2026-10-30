use axion_store::{NewBookmark, NewHistory, Store};

use crate::error::Result;
use crate::model::Imported;

/// What an import wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub bookmarks_added: usize,
    pub bookmarks_existing: usize,
    pub history_added: usize,
    pub history_existing: usize,
    /// Site icons newly written to the icon cache (see [`crate::write_icons`]; the app fills
    /// this in, [`apply`] only writes to the store).
    pub icons_added: usize,
    /// Parts that could not be read.
    pub warnings: Vec<String>,
}

impl Report {
    /// Adds another report's numbers to this one (several profiles imported in a row).
    pub fn merge(&mut self, other: Report) {
        self.bookmarks_added += other.bookmarks_added;
        self.bookmarks_existing += other.bookmarks_existing;
        self.history_added += other.history_added;
        self.history_existing += other.history_existing;
        self.icons_added += other.icons_added;
        self.warnings.extend(other.warnings);
    }

    /// Entries that were already there.
    pub fn existing(&self) -> usize {
        self.bookmarks_existing + self.history_existing
    }
}

/// Writes `data` into `store` (each kind in one transaction). What the store already has, by
/// URL, is kept as it is. `now` stands in for unknown bookmark dates.
pub fn apply(store: &Store, data: &Imported, now: i64) -> Result<Report> {
    let bookmarks: Vec<NewBookmark> = data
        .bookmarks
        .iter()
        .map(|b| NewBookmark {
            url: b.url.clone(),
            title: b.title.clone(),
            folder_path: b.folder_path(),
            created: b.added,
        })
        .collect();
    let history: Vec<NewHistory> = data
        .history
        .iter()
        .map(|h| NewHistory {
            url: h.url.clone(),
            title: h.title.clone(),
            visit_count: h.visit_count,
            last_visit: h.last_visit,
        })
        .collect();
    let b = store.import_bookmarks(&bookmarks, now)?;
    let h = store.import_history(&history)?;
    Ok(Report {
        bookmarks_added: b.added,
        bookmarks_existing: b.existing,
        history_added: h.added,
        history_existing: h.existing,
        icons_added: 0,
        warnings: data.warnings.clone(),
    })
}
