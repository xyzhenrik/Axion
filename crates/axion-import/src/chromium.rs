//! The Chromium family: the `Bookmarks` JSON file and the `History` SQLite database.

use std::path::Path;

use serde_json::Value;

use crate::error::{ImportError, Result};
use crate::icons::{Candidates, ImportIcon, WantedIcons};
use crate::model::{ImportBookmark, ImportHistory, Limits};
use crate::sqlite::{DbCopy, history_count, history_top, int, text};
use crate::text::{clean_title, clean_url, read_text_file, webkit_to_unix};

/// Folders nested deeper than this are not followed.
const MAX_DEPTH: usize = 64;

pub fn read_bookmarks(profile: &Path, limits: &Limits) -> Result<Vec<ImportBookmark>> {
    let text = read_text_file(&profile.join("Bookmarks"), limits.max_text_bytes)?;
    let value: Value = serde_json::from_str(&text)?;
    let mut bookmarks = parse_bookmarks(&value)?;
    bookmarks.truncate(limits.max_bookmarks);
    Ok(bookmarks)
}

/// The bookmarks of a parsed `Bookmarks` file (`{"roots": {"bookmark_bar": {…}, …}}`).
pub fn parse_bookmarks(file: &Value) -> Result<Vec<ImportBookmark>> {
    let roots = file
        .get("roots")
        .and_then(Value::as_object)
        .ok_or_else(|| ImportError::Format("no bookmark roots in the file".into()))?;
    let mut out = Vec::new();
    // The bar first (it becomes the top level), then the others in a stable order.
    let order = ["bookmark_bar", "other", "synced"];
    let mut keys: Vec<&String> = roots.keys().collect();
    keys.sort_by_key(|k| (order.iter().position(|o| o == k).unwrap_or(order.len()), k.as_str().to_owned()));
    for key in keys {
        let root = &roots[key];
        if !root.is_object() || matches!(key.as_str(), "trash" | "meta_info" | "sync_transaction_version") {
            continue;
        }
        let folders = match key.as_str() {
            "bookmark_bar" => Vec::new(),
            "other" => vec!["Other Bookmarks".to_owned()],
            "synced" => vec!["Mobile Bookmarks".to_owned()],
            _ => vec![clean_title(root.get("name").and_then(Value::as_str).unwrap_or(key))],
        };
        walk(root, &folders, 0, &mut out);
    }
    Ok(out)
}

fn walk(node: &Value, folders: &[String], depth: usize, out: &mut Vec<ImportBookmark>) {
    let Some(children) = node.get("children").and_then(Value::as_array) else { return };
    for child in children {
        match child.get("type").and_then(Value::as_str) {
            Some("url") => {
                let Some(url) = child.get("url").and_then(Value::as_str).and_then(clean_url) else {
                    continue;
                };
                let title = child.get("name").and_then(Value::as_str).map(clean_title).unwrap_or_default();
                out.push(ImportBookmark {
                    url,
                    title,
                    folders: folders.to_vec(),
                    added: child.get("date_added").map_or(0, |d| webkit_to_unix(number(d))),
                });
            }
            Some("folder") if depth < MAX_DEPTH => {
                let name = child.get("name").and_then(Value::as_str).map(clean_title).unwrap_or_default();
                let mut inner = folders.to_vec();
                inner.push(if name.is_empty() { "Folder".to_owned() } else { name });
                walk(child, &inner, depth + 1, out);
            }
            _ => {}
        }
    }
}

/// Chromium writes big numbers as strings in its JSON.
fn number(v: &Value) -> i64 {
    match v {
        Value::String(s) => s.trim().parse().unwrap_or(0),
        Value::Number(n) => n.as_i64().unwrap_or(0),
        _ => 0,
    }
}

const HISTORY_SQL: &str = "SELECT url, title, visit_count, last_visit_time FROM urls
    WHERE hidden = 0 AND visit_count > 0 AND last_visit_time > 0 ORDER BY last_visit_time DESC";

pub fn read_history(profile: &Path, limits: &Limits) -> Result<Vec<ImportHistory>> {
    let db = DbCopy::open(&profile.join("History"), limits.max_db_bytes)?;
    history_top(&db.conn, HISTORY_SQL, webkit_to_unix, limits.max_history_items)
}

/// The site icons of `Favicons` (`icon_mapping` ties pages to icons, `favicon_bitmaps` holds
/// the PNGs in one or more sizes) for the wanted hosts. No `Favicons` file: no icons.
pub fn read_icons(profile: &Path, wanted: &WantedIcons, limits: &Limits) -> Result<Vec<ImportIcon>> {
    let path = profile.join("Favicons");
    if wanted.is_empty() || !path.is_file() {
        return Ok(Vec::new());
    }
    let db = DbCopy::open(&path, limits.max_db_bytes)?;
    let mut candidates = Candidates::default();
    {
        let mut stmt = db.conn.prepare(
            "SELECT m.page_url, b.id, b.width, b.height, length(b.image_data)
             FROM icon_mapping m JOIN favicon_bitmaps b ON b.icon_id = m.icon_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let (w, h) = (int(row, 2), int(row, 3));
            let side = w.min(h).clamp(0, 4096) as u32;
            candidates.add(wanted, &text(row, 0), int(row, 1), side, int(row, 4), limits);
        }
    }
    let mut blob = db.conn.prepare("SELECT image_data FROM favicon_bitmaps WHERE id = ?1")?;
    Ok(candidates.resolve(wanted, limits, |id| blob.query_row([id], |r| r.get::<_, Vec<u8>>(0)).ok()))
}

/// How many history entries there are to import (not capped).
pub fn count_history(profile: &Path, limits: &Limits) -> Result<usize> {
    let db = DbCopy::open(&profile.join("History"), limits.max_db_bytes)?;
    history_count(&db.conn, HISTORY_SQL, webkit_to_unix)
}

/// The profiles of a Chromium user-data directory: `(directory name, display name)`, those
/// with a `Bookmarks` or `History` file only. Names come from `Local State`'s
/// `profile.info_cache`; without it the `Default` and `Profile N` directories are used.
pub fn profiles(base: &Path) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = Vec::new();
    if let Ok(text) = read_text_file(&base.join("Local State"), 16 * 1024 * 1024)
        && let Ok(state) = serde_json::from_str::<Value>(&text)
        && let Some(cache) = state.pointer("/profile/info_cache").and_then(Value::as_object)
    {
        for (dir, info) in cache {
            let name = info.get("name").and_then(Value::as_str).map(clean_title).unwrap_or_default();
            names.push((dir.clone(), if name.is_empty() { dir.clone() } else { name }));
        }
    }
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let dir = entry.file_name().to_string_lossy().into_owned();
            let candidate = dir == "Default"
                || (dir.starts_with("Profile ") && dir[8..].chars().all(|c| c.is_ascii_digit()));
            if candidate && !names.iter().any(|(d, _)| *d == dir) {
                names.push((dir.clone(), dir));
            }
        }
    }
    // Opera keeps its single profile in the base directory itself.
    if base.join("Bookmarks").is_file() || base.join("History").is_file() {
        names.push((String::new(), "Default".to_owned()));
    }
    names.retain(|(dir, _)| {
        // A name from `Local State` must stay inside the base directory.
        !dir.contains(['/', '\\']) && dir != ".." && {
            let p = base.join(dir);
            p.join("Bookmarks").is_file() || p.join("History").is_file()
        }
    });
    names.sort();
    names.dedup();
    names
}
