//! Axion's local data in one SQLite file: browsing history (with "frecency" ranking for the
//! address bar) and bookmarks.
//!
//! All calls are synchronous and quick (indexed, WAL mode); the browser calls them on the main
//! thread.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

/// Schema migrations, applied in order; `PRAGMA user_version` records how many ran.
const MIGRATIONS: &[&str] = &[
    // 1: history and bookmarks
    "CREATE TABLE urls (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL DEFAULT '',
        visit_count INTEGER NOT NULL DEFAULT 0,
        last_visit INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE visits (
        id INTEGER PRIMARY KEY,
        url_id INTEGER NOT NULL REFERENCES urls(id) ON DELETE CASCADE,
        at INTEGER NOT NULL
    );
    CREATE INDEX visits_by_time ON visits(at);
    CREATE INDEX urls_by_last_visit ON urls(last_visit);
    CREATE TABLE bookmarks (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL,
        title TEXT NOT NULL DEFAULT '',
        folder INTEGER REFERENCES bookmarks(id) ON DELETE CASCADE,
        position INTEGER NOT NULL DEFAULT 0,
        created INTEGER NOT NULL
    );
    CREATE INDEX bookmarks_by_url ON bookmarks(url);",
    // 2: small key/value settings that don't belong in config.toml (e.g. secrets' hashes)
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // 3: per-site settings (Shields level, …), keyed by host
    "CREATE TABLE site_settings (host TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
        PRIMARY KEY (host, key));",
];

/// Seconds per day, for the recency buckets.
const DAY: i64 = 86_400;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("creating {path}: {source}")]
    Io { path: std::path::PathBuf, source: std::io::Error },
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: u32,
    /// Unix seconds.
    pub last_visit: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub folder: Option<i64>,
    pub position: i64,
}

pub struct Store {
    conn: Connection,
}

/// URLs not worth remembering.
fn is_recordable(url: &str) -> bool {
    !url.is_empty()
        && !["about:", "data:", "blob:", "chrome:", "devtools:", "view-source:"]
            .iter()
            .any(|p| url.starts_with(p))
}

/// Escapes `%`, `_` and `\` for a `LIKE … ESCAPE '\'` pattern.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| StoreError::Io { path: dir.to_owned(), source })?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, migration) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
            conn.execute_batch(migration)?;
            conn.pragma_update(None, "user_version", i as i64 + 1)?;
            tracing::info!(version = i + 1, "store migrated");
        }
        Ok(Self { conn })
    }

    // ---- history ----

    /// Records a visit to `url` at `at` (unix seconds).
    pub fn record_visit(&self, url: &str, title: &str, at: i64) -> Result<()> {
        if !is_recordable(url) {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO urls (url, title, visit_count, last_visit) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(url) DO UPDATE SET
                visit_count = visit_count + 1,
                last_visit = excluded.last_visit,
                title = CASE WHEN excluded.title = '' THEN title ELSE excluded.title END",
            params![url, title, at],
        )?;
        self.conn.execute(
            "INSERT INTO visits (url_id, at) SELECT id, ?2 FROM urls WHERE url = ?1",
            params![url, at],
        )?;
        Ok(())
    }

    pub fn set_title(&self, url: &str, title: &str) -> Result<()> {
        if !title.is_empty() {
            self.conn.execute("UPDATE urls SET title = ?2 WHERE url = ?1", params![url, title])?;
        }
        Ok(())
    }

    /// History matching every word of `query` (in URL or title), best first: pages whose host
    /// starts with the query, then by frecency (visits weighted by how recent they are).
    pub fn search_history(&self, query: &str, limit: usize, now: i64) -> Result<Vec<HistoryEntry>> {
        let words: Vec<String> = query.split_whitespace().map(|w| format!("%{}%", like_escape(w))).collect();
        if words.is_empty() {
            return self.recent_history(limit);
        }
        let mut sql = String::from("SELECT url, title, visit_count, last_visit FROM urls WHERE 1");
        for i in 0..words.len() {
            let n = i + 4;
            sql.push_str(&format!(" AND (url LIKE ?{n} ESCAPE '\\' OR title LIKE ?{n} ESCAPE '\\')"));
        }
        sql.push_str(
            " ORDER BY
                (url LIKE 'http://' || ?1 || '%' ESCAPE '\\' OR url LIKE 'https://' || ?1 || '%' ESCAPE '\\'
                 OR url LIKE 'https://www.' || ?1 || '%' ESCAPE '\\' OR url LIKE 'http://www.' || ?1 || '%' ESCAPE '\\') DESC,
                visit_count * CASE
                    WHEN ?2 - last_visit < 4 * ?3 THEN 100
                    WHEN ?2 - last_visit < 14 * ?3 THEN 70
                    WHEN ?2 - last_visit < 31 * ?3 THEN 50
                    WHEN ?2 - last_visit < 90 * ?3 THEN 30
                    ELSE 10 END DESC,
                last_visit DESC",
        );
        sql.push_str(&format!(" LIMIT {limit}"));
        let prefix = like_escape(query.trim());
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&prefix, &now, &DAY];
        for w in &words {
            values.push(w);
        }
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(values.as_slice(), entry)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn recent_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT url, title, visit_count, last_visit FROM urls ORDER BY last_visit DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], entry)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_history_url(&self, url: &str) -> Result<()> {
        self.conn.execute("DELETE FROM urls WHERE url = ?1", [url])?;
        Ok(())
    }

    /// Deletes history visited at or after `since` (unix seconds; 0 clears everything).
    pub fn clear_history_since(&self, since: i64) -> Result<()> {
        self.conn.execute("DELETE FROM visits WHERE at >= ?1", [since])?;
        self.conn.execute("DELETE FROM urls WHERE id NOT IN (SELECT DISTINCT url_id FROM visits)", [])?;
        Ok(())
    }

    // ---- meta ----

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    /// Sets (or with `None`, removes) a meta value.
    pub fn set_meta(&self, key: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) => self.conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, v],
            )?,
            None => self.conn.execute("DELETE FROM meta WHERE key = ?1", [key])?,
        };
        Ok(())
    }

    // ---- per-site settings ----

    /// Every site's value for `key`, as (host, value).
    pub fn site_settings(&self, key: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT host, value FROM site_settings WHERE key = ?1")?;
        let rows = stmt.query_map([key], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sets (or with `None`, removes) a site's value for `key`.
    pub fn set_site_setting(&self, host: &str, key: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) => self.conn.execute(
                "INSERT INTO site_settings (host, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(host, key) DO UPDATE SET value = excluded.value",
                params![host, key, v],
            )?,
            None => {
                self.conn.execute("DELETE FROM site_settings WHERE host = ?1 AND key = ?2", [host, key])?
            }
        };
        Ok(())
    }

    // ---- bookmarks ----

    pub fn add_bookmark(&self, url: &str, title: &str, now: i64) -> Result<i64> {
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM bookmarks WHERE folder IS NULL",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO bookmarks (url, title, position, created) VALUES (?1, ?2, ?3, ?4)",
            params![url, title, position, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn remove_bookmark(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn bookmark_for(&self, url: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM bookmarks WHERE url = ?1 LIMIT 1", [url], |r| r.get(0))
            .optional()?)
    }

    pub fn bookmarks(&self) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, url, title, folder, position FROM bookmarks ORDER BY folder IS NOT NULL, folder, position",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                url: r.get(1)?,
                title: r.get(2)?,
                folder: r.get(3)?,
                position: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn search_bookmarks(&self, query: &str, limit: usize) -> Result<Vec<Bookmark>> {
        let pattern = format!("%{}%", like_escape(query.trim()));
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, url, title, folder, position FROM bookmarks
             WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\' ORDER BY position LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                url: r.get(1)?,
                title: r.get(2)?,
                folder: r.get(3)?,
                position: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry { url: r.get(0)?, title: r.get(1)?, visit_count: r.get(2)?, last_visit: r.get(3)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    #[test]
    fn records_and_counts_visits() {
        let s = store();
        s.record_visit("https://example.com/", "Example", NOW - 10).unwrap();
        s.record_visit("https://example.com/", "", NOW).unwrap();
        let recent = s.recent_history(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].visit_count, 2);
        assert_eq!(recent[0].title, "Example", "an empty title must not overwrite a known one");
        assert_eq!(recent[0].last_visit, NOW);
    }

    #[test]
    fn skips_internal_urls() {
        let s = store();
        s.record_visit("about:blank", "", NOW).unwrap();
        s.record_visit("data:text/html,hi", "", NOW).unwrap();
        assert!(s.recent_history(10).unwrap().is_empty());
    }

    #[test]
    fn search_prefers_host_prefix_then_frecency() {
        let s = store();
        // Often visited long ago, matches only in the title.
        for _ in 0..5 {
            s.record_visit("https://docs.example.org/rust", "Rust notes", NOW - 200 * DAY).unwrap();
        }
        s.record_visit("https://www.rust-lang.org/", "Rust Programming Language", NOW - DAY).unwrap();
        s.record_visit("https://blog.example.com/", "Unrelated", NOW).unwrap();
        let found = s.search_history("rust", 10, NOW).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].url, "https://www.rust-lang.org/");
        // Every word must match.
        assert_eq!(s.search_history("rust notes", 10, NOW).unwrap().len(), 1);
        // LIKE wildcards in the query are literal.
        assert!(s.search_history("%", 10, NOW).unwrap().is_empty());
    }

    #[test]
    fn clearing_history() {
        let s = store();
        s.record_visit("https://old.example/", "", NOW - 2 * DAY).unwrap();
        s.record_visit("https://new.example/", "", NOW).unwrap();
        s.clear_history_since(NOW - DAY).unwrap();
        let left: Vec<String> = s.recent_history(10).unwrap().into_iter().map(|e| e.url).collect();
        assert_eq!(left, vec!["https://old.example/"]);
    }

    #[test]
    fn bookmarks_round_trip() {
        let s = store();
        let a = s.add_bookmark("https://a.example/", "A", NOW).unwrap();
        s.add_bookmark("https://b.example/", "B", NOW).unwrap();
        assert_eq!(s.bookmark_for("https://a.example/").unwrap(), Some(a));
        let all = s.bookmarks().unwrap();
        assert_eq!(all.iter().map(|b| b.position).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(s.search_bookmarks("b.ex", 5).unwrap().len(), 1);
        s.remove_bookmark(a).unwrap();
        assert_eq!(s.bookmark_for("https://a.example/").unwrap(), None);
    }

    #[test]
    fn meta_values() {
        let s = store();
        assert_eq!(s.meta("k").unwrap(), None);
        s.set_meta("k", Some("v1")).unwrap();
        s.set_meta("k", Some("v2")).unwrap();
        assert_eq!(s.meta("k").unwrap().as_deref(), Some("v2"));
        s.set_meta("k", None).unwrap();
        assert_eq!(s.meta("k").unwrap(), None);
    }

    #[test]
    fn site_settings() {
        let s = store();
        s.set_site_setting("a.com", "shields", Some("off")).unwrap();
        s.set_site_setting("b.com", "shields", Some("aggressive")).unwrap();
        s.set_site_setting("a.com", "shields", Some("standard")).unwrap();
        s.set_site_setting("a.com", "zoom", Some("1")).unwrap();
        let mut all = s.site_settings("shields").unwrap();
        all.sort();
        assert_eq!(all, [("a.com".into(), "standard".into()), ("b.com".into(), "aggressive".into())]);
        s.set_site_setting("b.com", "shields", None).unwrap();
        assert_eq!(s.site_settings("shields").unwrap().len(), 1);
    }

    #[test]
    fn reopening_keeps_data() {
        let dir = std::env::temp_dir().join(format!("axion-store-test-{}", std::process::id()));
        let path = dir.join("axion.sqlite");
        {
            let s = Store::open(&path).unwrap();
            s.record_visit("https://keep.example/", "Keep", NOW).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.recent_history(1).unwrap()[0].title, "Keep");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
