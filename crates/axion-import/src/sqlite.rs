//! Reading another browser's SQLite file without touching it: the file (and its write-ahead
//! log) is copied into a temporary directory, and only the copy is opened.

use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};

use crate::error::{ImportError, Result};
use crate::model::ImportHistory;
use crate::text::{clean_title, clean_url};

/// A private copy of a database; the copy goes away when this is dropped.
pub struct DbCopy {
    _dir: tempfile::TempDir,
    pub conn: Connection,
}

impl DbCopy {
    /// Copies `source` (and `source-wal`, which holds what the browser wrote last) and opens
    /// the copy. Files larger than `max_bytes` in total are refused.
    pub fn open(source: &Path, max_bytes: u64) -> Result<Self> {
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| ImportError::Io { path, source }
        };
        let wal = sidecar(source, "-wal");
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len());
        let total = size(source).map_err(io(source))? + size(&wal).unwrap_or(0);
        if total > max_bytes {
            return Err(ImportError::TooLarge { path: source.to_owned(), size: total, limit: max_bytes });
        }
        let dir =
            tempfile::Builder::new().prefix("axion-import-").tempdir().map_err(io(Path::new("/tmp")))?;
        let copy = dir.path().join("db.sqlite");
        std::fs::copy(source, &copy).map_err(io(source))?;
        if wal.exists() {
            // Best effort: without it only the newest few entries are missing.
            let _ = std::fs::copy(&wal, sidecar(&copy, "-wal"));
        }
        // The copy is ours, so it may be opened read-write (a WAL needs that to be replayed);
        // `query_only` keeps this code from changing it anyway.
        let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.pragma_update(None, "query_only", true)?;
        conn.busy_timeout(std::time::Duration::from_millis(500))?;
        Ok(Self { _dir: dir, conn })
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Column `i` as text, whatever the cell holds (invalid UTF-8 is repaired, NULL is empty).
pub fn text(row: &Row<'_>, i: usize) -> String {
    match row.get_ref(i) {
        Ok(ValueRef::Text(b) | ValueRef::Blob(b)) => String::from_utf8_lossy(b).into_owned(),
        Ok(ValueRef::Integer(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Column `i` as an integer (0 for NULL and for text that is no number).
pub fn int(row: &Row<'_>, i: usize) -> i64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(n)) => n,
        Ok(ValueRef::Real(f)) => f as i64,
        Ok(ValueRef::Text(b)) => std::str::from_utf8(b).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        _ => 0,
    }
}

/// Runs `sql` (url, title, visit count, last visit; newest first) and hands every importable
/// row to `sink` until it returns false. Rows with an unusable URL or time are left out.
pub fn history_rows(
    conn: &Connection,
    sql: &str,
    to_unix: fn(i64) -> i64,
    mut sink: impl FnMut(ImportHistory) -> bool,
) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let last_visit = to_unix(int(row, 3));
        let Some(url) = clean_url(&text(row, 0)) else { continue };
        if last_visit == 0 {
            continue;
        }
        let entry = ImportHistory {
            url,
            title: clean_title(&text(row, 1)),
            visit_count: int(row, 2).clamp(1, i64::from(u32::MAX)) as u32,
            last_visit,
        };
        if !sink(entry) {
            break;
        }
    }
    Ok(())
}

/// The first `max` rows of [`history_rows`].
pub fn history_top(
    conn: &Connection,
    sql: &str,
    to_unix: fn(i64) -> i64,
    max: usize,
) -> Result<Vec<ImportHistory>> {
    let mut out = Vec::new();
    if max > 0 {
        history_rows(conn, sql, to_unix, |e| {
            out.push(e);
            out.len() < max
        })?;
    }
    Ok(out)
}

/// How many rows [`history_rows`] would hand out.
pub fn history_count(conn: &Connection, sql: &str, to_unix: fn(i64) -> i64) -> Result<usize> {
    let mut n = 0;
    history_rows(conn, sql, to_unix, |_| {
        n += 1;
        true
    })?;
    Ok(n)
}
