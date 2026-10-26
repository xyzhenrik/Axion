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
    // 4: the open tabs, for session restore
    "CREATE TABLE session_tabs (position INTEGER PRIMARY KEY, url TEXT NOT NULL,
        title TEXT NOT NULL DEFAULT '', active INTEGER NOT NULL DEFAULT 0);",
    // 5: when each open tab was last looked at (for closing unused tabs)
    "ALTER TABLE session_tabs ADD COLUMN last_viewed INTEGER NOT NULL DEFAULT 0;",
    // 6: finished downloads (the download list)
    "CREATE TABLE downloads (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL,
        path TEXT NOT NULL,
        file_name TEXT NOT NULL,
        mime_type TEXT NOT NULL DEFAULT '',
        size INTEGER NOT NULL DEFAULT 0,
        state TEXT NOT NULL,
        finished INTEGER NOT NULL
    );
    CREATE INDEX downloads_by_time ON downloads(finished);",
    // 7: the start page's top sites: pinned (or edited) and removed sites, by host
    "CREATE TABLE top_sites (
        host TEXT PRIMARY KEY,
        url TEXT NOT NULL DEFAULT '',
        title TEXT NOT NULL DEFAULT '',
        position INTEGER NOT NULL DEFAULT 0,
        hidden INTEGER NOT NULL DEFAULT 0
    );",
    // 8: the Playlist (videos saved for later, optionally downloaded)
    "CREATE TABLE playlist (
        id INTEGER PRIMARY KEY,
        list TEXT NOT NULL DEFAULT '',
        sort INTEGER NOT NULL DEFAULT 0,
        title TEXT NOT NULL DEFAULT '',
        page_url TEXT NOT NULL,
        media_url TEXT,
        thumbnail TEXT NOT NULL DEFAULT '',
        duration REAL NOT NULL DEFAULT 0,
        added_at INTEGER NOT NULL,
        position REAL NOT NULL DEFAULT 0,
        watched INTEGER NOT NULL DEFAULT 0,
        dl_state TEXT NOT NULL DEFAULT 'none',
        dl_path TEXT NOT NULL DEFAULT '',
        dl_size INTEGER NOT NULL DEFAULT 0,
        dl_note TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX playlist_by_list ON playlist(list, sort);",
    // 9: the media source captured for a Playlist item (JSON; empty until captured)
    "ALTER TABLE playlist ADD COLUMN source TEXT NOT NULL DEFAULT '';",
];

/// Download states of a playlist item (`PlaylistItem::dl_state`).
pub mod dl {
    pub const NONE: &str = "none";
    pub const QUEUED: &str = "queued";
    pub const DOWNLOADING: &str = "downloading";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
    /// A stream (HLS, blob) that can only be watched online.
    pub const STREAMING: &str = "streaming";
}

/// A video saved in the Playlist.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistItem {
    pub id: i64,
    pub list: String,
    pub sort: i64,
    pub title: String,
    pub page_url: String,
    /// A plain file's address when the page showed one.
    pub media_url: Option<String>,
    /// Path of the thumbnail file, empty for none.
    pub thumbnail: String,
    /// Seconds, 0 while unknown.
    pub duration: f64,
    /// Unix seconds.
    pub added_at: i64,
    /// Where playback stopped (seconds).
    pub position: f64,
    pub watched: bool,
    /// One of the [`dl`] states.
    pub dl_state: String,
    pub dl_path: String,
    pub dl_size: i64,
    /// Why a download failed or isn't possible, for people.
    pub dl_note: String,
    /// The captured media source as JSON (see `axion_media::MediaSource`); empty until captured.
    pub source: String,
}

/// A video about to be added to the Playlist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewPlaylistItem {
    pub list: String,
    pub title: String,
    pub page_url: String,
    pub media_url: Option<String>,
    pub thumbnail: String,
    pub duration: f64,
    pub position: f64,
    pub dl_state: String,
    pub dl_path: String,
    pub dl_size: i64,
    pub dl_note: String,
    pub source: String,
}

const PLAYLIST_COLUMNS: &str = "id, list, sort, title, page_url, media_url, thumbnail, duration, added_at, \
     position, watched, dl_state, dl_path, dl_size, dl_note, source";

fn playlist_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<PlaylistItem> {
    Ok(PlaylistItem {
        id: r.get(0)?,
        list: r.get(1)?,
        sort: r.get(2)?,
        title: r.get(3)?,
        page_url: r.get(4)?,
        media_url: r.get(5)?,
        thumbnail: r.get(6)?,
        duration: r.get(7)?,
        added_at: r.get(8)?,
        position: r.get(9)?,
        watched: r.get::<_, i64>(10)? != 0,
        dl_state: r.get(11)?,
        dl_path: r.get(12)?,
        dl_size: r.get(13)?,
        dl_note: r.get(14)?,
        source: r.get(15)?,
    })
}

/// A site the user pinned to (or removed from) the start page's top sites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopSiteOverride {
    /// Lowercase, without `www.`.
    pub host: String,
    pub url: String,
    pub title: String,
    /// Order among the pinned sites.
    pub position: i64,
    /// Removed: never listed from history again (until restored).
    pub hidden: bool,
}

/// A site among the most visited, ranked by frecency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopSite {
    /// Lowercase, without `www.`.
    pub host: String,
    /// The site's most visited page.
    pub url: String,
    pub title: String,
}

/// A finished download in the download list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDownload {
    pub id: i64,
    pub url: String,
    pub path: String,
    pub file_name: String,
    pub mime_type: String,
    /// Bytes.
    pub size: i64,
    /// "complete", "cancelled" or "failed".
    pub state: String,
    /// Unix seconds.
    pub finished: i64,
}

/// A saved tab of the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    /// Unix seconds.
    pub last_viewed: i64,
}

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

/// URLs not worth remembering (Axion's own `axion:` pages included).
fn is_recordable(url: &str) -> bool {
    !url.is_empty()
        && !["about:", "data:", "blob:", "chrome:", "devtools:", "view-source:", "axion:"]
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

    /// The inline completion for what was typed in the address bar: the most visited history
    /// page whose address (without scheme and `www.`) starts with `typed`, case-insensitively.
    /// Without a `/` in `typed` it completes to the host only (as Safari does), otherwise to
    /// the whole address. The result starts with `typed` as typed.
    pub fn autocomplete(&self, typed: &str) -> Result<Option<String>> {
        let typed = typed.trim();
        if typed.is_empty() || typed.contains(char::is_whitespace) {
            return Ok(None);
        }
        let pattern = format!("{}%", like_escape(&typed.to_lowercase()));
        let mut stmt = self.conn.prepare_cached(
            "SELECT url FROM urls
             WHERE lower(url) LIKE 'http://' || ?1 ESCAPE '\\' OR lower(url) LIKE 'https://' || ?1 ESCAPE '\\'
                OR lower(url) LIKE 'http://www.' || ?1 ESCAPE '\\' OR lower(url) LIKE 'https://www.' || ?1 ESCAPE '\\'
             ORDER BY visit_count DESC, last_visit DESC LIMIT 20",
        )?;
        let urls: Vec<String> = stmt.query_map([&pattern], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let host_only = !typed.contains('/');
        for url in urls {
            let bare = strip_scheme(&url);
            let candidate = if host_only { bare.split('/').next().unwrap_or(bare) } else { bare };
            let candidate = candidate.trim_end_matches('/');
            if candidate.len() > typed.len()
                && candidate.is_char_boundary(typed.len())
                && candidate[..typed.len()].eq_ignore_ascii_case(typed)
            {
                return Ok(Some(format!("{typed}{}", &candidate[typed.len()..])));
            }
        }
        Ok(None)
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

    // ---- playlist ----

    /// Adds a video to the Playlist, at the top. A video already in the list (same file, or
    /// for streams the same page) isn't added twice: returns its id and `false`.
    pub fn add_playlist_item(&self, new: &NewPlaylistItem, now: i64) -> Result<(i64, bool)> {
        let existing: Option<i64> = match &new.media_url {
            Some(media) => self
                .conn
                .query_row(
                    "SELECT id FROM playlist WHERE list = ?1 AND media_url = ?2",
                    params![new.list, media],
                    |r| r.get(0),
                )
                .optional()?,
            None if !new.source.is_empty() => self
                .conn
                .query_row(
                    "SELECT id FROM playlist WHERE list = ?1 AND media_url IS NULL AND source = ?2",
                    params![new.list, new.source],
                    |r| r.get(0),
                )
                .optional()?,
            None => self
                .conn
                .query_row(
                    "SELECT id FROM playlist WHERE list = ?1 AND media_url IS NULL AND page_url = ?2",
                    params![new.list, new.page_url],
                    |r| r.get(0),
                )
                .optional()?,
        };
        if let Some(id) = existing {
            return Ok((id, false));
        }
        let top: i64 = self.conn.query_row(
            "SELECT COALESCE(MIN(sort), 0) - 1 FROM playlist WHERE list = ?1",
            [&new.list],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO playlist (list, sort, title, page_url, media_url, thumbnail, duration, added_at,
                position, dl_state, dl_path, dl_size, dl_note, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                new.list,
                top,
                new.title,
                new.page_url,
                new.media_url,
                new.thumbnail,
                new.duration,
                now,
                new.position,
                new.dl_state,
                new.dl_path,
                new.dl_size,
                new.dl_note,
                new.source
            ],
        )?;
        Ok((self.conn.last_insert_rowid(), true))
    }

    /// The items of a list in their order (newest added first, unless reordered).
    pub fn playlist_items(&self, list: &str) -> Result<Vec<PlaylistItem>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {PLAYLIST_COLUMNS} FROM playlist WHERE list = ?1 ORDER BY sort, id DESC"
        ))?;
        let rows = stmt.query_map([list], playlist_item)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Renames a list: its items move to `to` (merging with one of that name).
    pub fn rename_playlist(&self, from: &str, to: &str) -> Result<()> {
        self.conn.execute("UPDATE playlist SET list = ?2 WHERE list = ?1", rusqlite::params![from, to])?;
        Ok(())
    }

    /// The names of the lists that have items.
    pub fn playlist_lists(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached("SELECT DISTINCT list FROM playlist ORDER BY list")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn playlist_item(&self, id: i64) -> Result<Option<PlaylistItem>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {PLAYLIST_COLUMNS} FROM playlist WHERE id = ?1"), [id], playlist_item)
            .optional()?)
    }

    pub fn playlist_len(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM playlist", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Saves where playback stopped, and whether the video counts as watched now.
    pub fn set_playlist_progress(&self, id: i64, position: f64, watched: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET position = ?2, watched = ?3 WHERE id = ?1",
            params![id, position, watched],
        )?;
        Ok(())
    }

    /// Keeps the media source captured for an item (JSON).
    pub fn set_playlist_source(&self, id: i64, source: &str) -> Result<()> {
        self.conn.execute("UPDATE playlist SET source = ?2 WHERE id = ?1", params![id, source])?;
        Ok(())
    }

    pub fn set_playlist_duration(&self, id: i64, duration: f64) -> Result<()> {
        self.conn.execute("UPDATE playlist SET duration = ?2 WHERE id = ?1", params![id, duration])?;
        Ok(())
    }

    pub fn set_playlist_thumbnail(&self, id: i64, path: &str) -> Result<()> {
        self.conn.execute("UPDATE playlist SET thumbnail = ?2 WHERE id = ?1", params![id, path])?;
        Ok(())
    }

    /// Records a download's state (one of [`dl`]), file and size, and a note for people.
    pub fn set_playlist_download(
        &self,
        id: i64,
        state: &str,
        path: &str,
        size: i64,
        note: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET dl_state = ?2, dl_path = ?3, dl_size = ?4, dl_note = ?5 WHERE id = ?1",
            params![id, state, path, size, note],
        )?;
        Ok(())
    }

    pub fn remove_playlist_item(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM playlist WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Swaps an item's place with its neighbour in `list` (`up`: towards the top).
    pub fn move_playlist_item(&mut self, id: i64, up: bool) -> Result<()> {
        let items = self.playlist_items(&self.playlist_item(id)?.map(|i| i.list).unwrap_or_default())?;
        let Some(at) = items.iter().position(|i| i.id == id) else { return Ok(()) };
        let other = if up { at.checked_sub(1) } else { Some(at + 1).filter(|&n| n < items.len()) };
        let Some(other) = other else { return Ok(()) };
        // Renumber the whole list, so equal `sort` values never leave a swap without effect.
        let mut order: Vec<i64> = items.iter().map(|i| i.id).collect();
        order.swap(at, other);
        let tx = self.conn.transaction()?;
        for (n, id) in order.iter().enumerate() {
            tx.execute("UPDATE playlist SET sort = ?2 WHERE id = ?1", params![id, n as i64])?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- downloads ----

    /// Adds a finished download to the list; returns its id.
    pub fn add_download(&self, d: &StoredDownload) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO downloads (url, path, file_name, mime_type, size, state, finished)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![d.url, d.path, d.file_name, d.mime_type, d.size, d.state, d.finished],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The download list, newest first.
    pub fn downloads(&self, limit: usize) -> Result<Vec<StoredDownload>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, path, file_name, mime_type, size, state, finished FROM downloads
             ORDER BY finished DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(StoredDownload {
                id: r.get(0)?,
                url: r.get(1)?,
                path: r.get(2)?,
                file_name: r.get(3)?,
                mime_type: r.get(4)?,
                size: r.get(5)?,
                state: r.get(6)?,
                finished: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_download(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Removes list entries finished before `before` (unix seconds; `i64::MAX`: all).
    pub fn clear_downloads_before(&self, before: i64) -> Result<()> {
        self.conn.execute("DELETE FROM downloads WHERE finished < ?1", [before])?;
        Ok(())
    }

    /// Removes list entries finished at or after `since`.
    pub fn clear_downloads_since(&self, since: i64) -> Result<()> {
        self.conn.execute("DELETE FROM downloads WHERE finished >= ?1", [since])?;
        Ok(())
    }

    /// The hosts visited at or after `since` (for clearing their site data).
    pub fn hosts_visited_since(&self, since: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT u.url FROM urls u JOIN visits v ON v.url_id = u.id WHERE v.at >= ?1")?;
        let urls = stmt.query_map([since], |r| r.get::<_, String>(0))?;
        let mut hosts: Vec<String> = urls
            .filter_map(|u| u.ok())
            .filter_map(|u| {
                let rest = u.split_once("://")?.1;
                let host = rest.split(['/', '?', '#']).next()?.split(':').next()?;
                (!host.is_empty()).then(|| host.trim_start_matches("www.").to_ascii_lowercase())
            })
            .collect();
        hosts.sort();
        hosts.dedup();
        Ok(hosts)
    }

    // ---- session ----

    /// Replaces the saved session with these tabs; `active` is an index into them.
    pub fn save_session(&mut self, tabs: &[SessionTab], active: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM session_tabs", [])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO session_tabs (position, url, title, active, last_viewed) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (i, tab) in tabs.iter().enumerate() {
                insert.execute(params![i as i64, tab.url, tab.title, i == active, tab.last_viewed])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The saved session's tabs and the active one's index.
    pub fn session(&self) -> Result<(Vec<SessionTab>, usize)> {
        let mut stmt = self
            .conn
            .prepare("SELECT url, title, active, last_viewed FROM session_tabs ORDER BY position")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                SessionTab { url: r.get(0)?, title: r.get(1)?, last_viewed: r.get(3)? },
                r.get::<_, bool>(2)?,
            ))
        })?;
        let mut tabs = Vec::new();
        let mut active = 0;
        for row in rows {
            let (tab, is_active) = row?;
            if is_active {
                active = tabs.len();
            }
            tabs.push(tab);
        }
        Ok((tabs, active))
    }

    // ---- top sites ----

    /// The most visited sites (one entry per host, its most visited page), best first by
    /// frecency summed over the host's pages. Only http(s) pages count.
    pub fn top_sites(&self, limit: usize, now: i64) -> Result<Vec<TopSite>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT url, title, visit_count * CASE
                    WHEN ?1 - last_visit < 4 * ?2 THEN 100
                    WHEN ?1 - last_visit < 14 * ?2 THEN 70
                    WHEN ?1 - last_visit < 31 * ?2 THEN 50
                    WHEN ?1 - last_visit < 90 * ?2 THEN 30
                    ELSE 10 END AS score, visit_count
             FROM urls WHERE url LIKE 'http%' ORDER BY score DESC LIMIT 2000",
        )?;
        let rows = stmt.query_map(params![now, DAY], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })?;
        // host → (summed score, best page's visits, url, title, first seen)
        let mut sites: Vec<(String, i64, i64, String, String)> = Vec::new();
        for row in rows {
            let (url, title, score, visits) = row?;
            let Some(host) = site_host(&url) else { continue };
            match sites.iter_mut().find(|s| s.0 == host) {
                Some(site) => {
                    site.1 += score;
                    if visits > site.2 {
                        (site.2, site.3, site.4) = (visits, url, title);
                    }
                }
                None => sites.push((host, score, visits, url, title)),
            }
        }
        sites.sort_by_key(|s| std::cmp::Reverse(s.1));
        Ok(sites
            .into_iter()
            .take(limit)
            .map(|(host, _, _, url, title)| TopSite { host, url, title })
            .collect())
    }

    /// The pinned and removed top sites, pinned ones in their order.
    pub fn top_site_overrides(&self) -> Result<Vec<TopSiteOverride>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host, url, title, position, hidden FROM top_sites ORDER BY position, host",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TopSiteOverride {
                host: r.get(0)?,
                url: r.get(1)?,
                title: r.get(2)?,
                position: r.get(3)?,
                hidden: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Pins a site (or updates a pinned one) with this address and title; new pins go last.
    pub fn pin_top_site(&self, host: &str, url: &str, title: &str) -> Result<()> {
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE((SELECT position FROM top_sites WHERE host = ?1 AND hidden = 0),
                             (SELECT COALESCE(MAX(position) + 1, 0) FROM top_sites))",
            [host],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO top_sites (host, url, title, position, hidden) VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(host) DO UPDATE SET url = excluded.url, title = excluded.title,
                position = excluded.position, hidden = 0",
            params![host, url, title, position],
        )?;
        Ok(())
    }

    /// Removes a site from the top sites (also unpins it).
    pub fn hide_top_site(&self, host: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO top_sites (host, hidden) VALUES (?1, 1)
             ON CONFLICT(host) DO UPDATE SET hidden = 1, url = '', title = ''",
            [host],
        )?;
        Ok(())
    }

    /// Unpins a site (it may still show from history).
    pub fn unpin_top_site(&self, host: &str) -> Result<()> {
        self.conn.execute("DELETE FROM top_sites WHERE host = ?1 AND hidden = 0", [host])?;
        Ok(())
    }

    /// Brings back every removed site.
    pub fn restore_hidden_top_sites(&self) -> Result<()> {
        self.conn.execute("DELETE FROM top_sites WHERE hidden = 1", [])?;
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

/// `https://www.Example.com:8080/x` → `example.com:8080` (http and https only).
pub fn site_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    (!host.is_empty()).then_some(host)
}

/// `https://www.example.com/x` → `example.com/x`.
fn strip_scheme(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.strip_prefix("www.").unwrap_or(rest)
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
    fn autocompletes_hosts_then_paths() {
        let s = store();
        s.record_visit("https://www.example.com/rust-lang/rust", "Rust", NOW).unwrap();
        s.record_visit("https://www.example.com/rust-lang/rust", "Rust", NOW).unwrap();
        s.record_visit("https://exhibit.org/", "Exhibit", NOW).unwrap();
        assert_eq!(s.autocomplete("ex").unwrap().as_deref(), Some("example.com"));
        assert_eq!(s.autocomplete("ExH").unwrap().as_deref(), Some("ExHibit.org"));
        assert_eq!(s.autocomplete("example.com/r").unwrap().as_deref(), Some("example.com/rust-lang/rust"));
        assert_eq!(s.autocomplete("example.com").unwrap(), None, "nothing left to complete");
        assert_eq!(s.autocomplete("nothing").unwrap(), None);
        assert_eq!(s.autocomplete("two words").unwrap(), None);
    }

    #[test]
    fn deleting_one_page_keeps_the_rest() {
        let s = store();
        s.record_visit("https://a.example/", "A", NOW).unwrap();
        s.record_visit("https://b.example/", "B", NOW).unwrap();
        s.delete_history_url("https://a.example/").unwrap();
        let left: Vec<String> = s.recent_history(10).unwrap().into_iter().map(|h| h.url).collect();
        assert_eq!(left, ["https://b.example/"]);
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
    fn session_round_trip() {
        let mut s = store();
        assert_eq!(s.session().unwrap(), (vec![], 0));
        let tab =
            |url: &str, title: &str| SessionTab { url: url.into(), title: title.into(), last_viewed: 7 };
        let tabs = vec![tab("https://a/", "A"), tab("https://b/", "B")];
        s.save_session(&tabs, 1).unwrap();
        assert_eq!(s.session().unwrap(), (tabs.clone(), 1));
        s.save_session(&tabs[..1], 0).unwrap();
        assert_eq!(s.session().unwrap().0.len(), 1);
    }

    #[test]
    fn downloads_list() {
        let s = store();
        let d = |name: &str, finished: i64| StoredDownload {
            id: 0,
            url: format!("https://a.example/{name}"),
            path: format!("/tmp/{name}"),
            file_name: name.into(),
            mime_type: "application/zip".into(),
            size: 10,
            state: "complete".into(),
            finished,
        };
        let a = s.add_download(&d("a.zip", 100)).unwrap();
        s.add_download(&d("b.zip", 200)).unwrap();
        let list = s.downloads(10).unwrap();
        assert_eq!(list.iter().map(|x| x.file_name.as_str()).collect::<Vec<_>>(), ["b.zip", "a.zip"]);
        s.remove_download(a).unwrap();
        assert_eq!(s.downloads(10).unwrap().len(), 1);
        s.clear_downloads_before(i64::MAX).unwrap();
        assert!(s.downloads(10).unwrap().is_empty());
    }

    #[test]
    fn hosts_since() {
        let s = store();
        s.record_visit("https://www.News.example/a?b", "", 100).unwrap();
        s.record_visit("http://other.org:8080/", "", 50).unwrap();
        assert_eq!(s.hosts_visited_since(60).unwrap(), ["news.example"]);
        assert_eq!(s.hosts_visited_since(0).unwrap().len(), 2);
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
    fn top_sites_by_host() {
        let s = store();
        for _ in 0..3 {
            s.record_visit("https://www.rust-lang.org/learn", "Learn", NOW).unwrap();
        }
        s.record_visit("https://rust-lang.org/", "Rust", NOW).unwrap();
        s.record_visit("https://docs.example/", "Docs", NOW - 100 * DAY).unwrap();
        s.record_visit("file:///tmp/x.html", "", NOW).unwrap();
        let top = s.top_sites(10, NOW).unwrap();
        let hosts: Vec<&str> = top.iter().map(|t| t.host.as_str()).collect();
        assert_eq!(hosts, ["rust-lang.org", "docs.example"]);
        assert_eq!(top[0].url, "https://www.rust-lang.org/learn", "the most visited page");
        assert_eq!(site_host("HTTP://user@WWW.A.example:80/x").as_deref(), Some("a.example:80"));
        assert_eq!(site_host("axion://home/"), None);
    }

    #[test]
    fn pinned_and_hidden_top_sites() {
        let s = store();
        s.pin_top_site("b.example", "https://b.example/", "B").unwrap();
        s.pin_top_site("a.example", "https://a.example/", "A").unwrap();
        s.pin_top_site("b.example", "https://b.example/x", "B2").unwrap();
        s.hide_top_site("c.example").unwrap();
        let all = s.top_site_overrides().unwrap();
        let pinned: Vec<(&str, &str)> =
            all.iter().filter(|o| !o.hidden).map(|o| (o.host.as_str(), o.title.as_str())).collect();
        assert_eq!(pinned, [("b.example", "B2"), ("a.example", "A")], "editing keeps the place");
        s.hide_top_site("a.example").unwrap();
        assert_eq!(s.top_site_overrides().unwrap().iter().filter(|o| o.hidden).count(), 2);
        s.unpin_top_site("b.example").unwrap();
        s.restore_hidden_top_sites().unwrap();
        assert!(s.top_site_overrides().unwrap().is_empty());
        s.record_visit("axion://home/", "Start", NOW).unwrap();
        assert!(s.recent_history(5).unwrap().is_empty(), "Axion's own pages stay out of history");
    }

    fn new_item(url: &str, media: Option<&str>) -> NewPlaylistItem {
        NewPlaylistItem {
            list: "Play Later".into(),
            title: "A video".into(),
            page_url: url.into(),
            media_url: media.map(str::to_owned),
            dl_state: dl::NONE.into(),
            ..Default::default()
        }
    }

    #[test]
    fn playlist_adds_once_and_newest_first() {
        let s = store();
        let (a, created) = s.add_playlist_item(&new_item("https://a/", Some("https://a/v.mp4")), 10).unwrap();
        assert!(created);
        // The same file again, from another page: the same entry.
        let (again, created) =
            s.add_playlist_item(&new_item("https://b/", Some("https://a/v.mp4")), 11).unwrap();
        assert_eq!((again, created), (a, false));
        // A stream is told apart by its page.
        let (b, _) = s.add_playlist_item(&new_item("https://s/1", None), 12).unwrap();
        let (c, _) = s.add_playlist_item(&new_item("https://s/2", None), 13).unwrap();
        assert_eq!(s.add_playlist_item(&new_item("https://s/1", None), 14).unwrap(), (b, false));
        let ids: Vec<i64> = s.playlist_items("Play Later").unwrap().iter().map(|i| i.id).collect();
        assert_eq!(ids, [c, b, a]);
        assert_eq!(s.playlist_len().unwrap(), 3);
        assert_eq!(s.playlist_lists().unwrap(), ["Play Later"]);
        s.remove_playlist_item(b).unwrap();
        assert!(s.playlist_item(b).unwrap().is_none());
    }

    #[test]
    fn playlist_progress_download_and_order() {
        let mut s = store();
        let (a, _) = s.add_playlist_item(&new_item("https://a/", Some("https://a/1.mp4")), 1).unwrap();
        let (b, _) = s.add_playlist_item(&new_item("https://b/", Some("https://b/2.mp4")), 2).unwrap();
        s.set_playlist_progress(a, 42.5, true).unwrap();
        s.set_playlist_download(a, dl::DONE, "/tmp/x.mp4", 1234, "").unwrap();
        s.set_playlist_thumbnail(a, "/tmp/x.png").unwrap();
        s.set_playlist_duration(a, 99.0).unwrap();
        let item = s.playlist_item(a).unwrap().unwrap();
        assert_eq!((item.position, item.watched, item.dl_size, item.duration), (42.5, true, 1234, 99.0));
        assert_eq!((item.dl_state.as_str(), item.dl_path.as_str()), ("done", "/tmp/x.mp4"));
        assert_eq!(item.thumbnail, "/tmp/x.png");
        assert_eq!(item.source, "");
        s.set_playlist_source(a, r#"{"url":"https://a/1.m3u8","kind":"hls"}"#).unwrap();
        assert!(s.playlist_item(a).unwrap().unwrap().source.contains("m3u8"));
        // A stream without a file is the same item when its captured source is the same.
        let mut stream = new_item("https://s/p1", None);
        stream.source = "S1".into();
        let (c, created) = s.add_playlist_item(&stream, 5).unwrap();
        assert!(created);
        stream.page_url = "https://s/p2".into();
        assert_eq!(s.add_playlist_item(&stream, 6).unwrap(), (c, false));
        s.remove_playlist_item(c).unwrap();
        // b is on top; moving it down puts a first.
        s.move_playlist_item(b, false).unwrap();
        let ids: Vec<i64> = s.playlist_items("Play Later").unwrap().iter().map(|i| i.id).collect();
        assert_eq!(ids, [a, b]);
        s.move_playlist_item(b, false).unwrap();
        s.move_playlist_item(a, true).unwrap();
        let ids: Vec<i64> = s.playlist_items("Play Later").unwrap().iter().map(|i| i.id).collect();
        assert_eq!(ids, [a, b]);
        s.rename_playlist("Play Later", "Weekend").unwrap();
        assert_eq!(s.playlist_items("Weekend").unwrap().len(), 2);
        assert!(s.playlist_items("Play Later").unwrap().is_empty());
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
