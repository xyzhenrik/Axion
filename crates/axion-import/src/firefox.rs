//! The Firefox family: `profiles.ini` and each profile's `places.sqlite` (bookmarks and
//! history live in the same database), and Firefox's JSON bookmark backups.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::error::{ImportError, Result};
use crate::icons::{Candidates, ImportIcon, WantedIcons};
use crate::model::{ImportBookmark, ImportHistory, Limits, Wanted};
use crate::sqlite::{DbCopy, history_count, history_top, int, text};
use crate::text::{clean_title, clean_url, firefox_to_unix, read_text_file};

const MAX_DEPTH: usize = 64;

/// A profile listed in `profiles.ini`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IniProfile {
    pub name: String,
    pub dir: PathBuf,
    /// Marked as the default (`Default=1`, or named by an `[Install…]` section).
    pub default: bool,
}

/// Parses `profiles.ini` of `root` (the directory that holds it): the `[ProfileN]` sections
/// with `Name`, `Path` and `IsRelative`. Paths that leave `root` through `..` are dropped.
pub fn parse_profiles_ini(text: &str, root: &Path) -> Vec<IniProfile> {
    struct Raw {
        name: String,
        path: String,
        relative: bool,
        default: bool,
    }
    let mut profiles: Vec<Raw> = Vec::new();
    let mut install_defaults: Vec<String> = Vec::new();
    #[derive(PartialEq)]
    enum Section {
        None,
        Profile,
        Install,
        Other,
    }
    let mut section = Section::None;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = if name.starts_with("Profile") {
                profiles.push(Raw {
                    name: String::new(),
                    path: String::new(),
                    relative: true,
                    default: false,
                });
                Section::Profile
            } else if name.starts_with("Install") {
                Section::Install
            } else {
                Section::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match (&section, key) {
            (Section::Profile, "Name") => {
                if let Some(p) = profiles.last_mut() {
                    p.name = clean_title(value);
                }
            }
            (Section::Profile, "Path") => {
                if let Some(p) = profiles.last_mut() {
                    p.path = value.to_owned();
                }
            }
            (Section::Profile, "IsRelative") => {
                if let Some(p) = profiles.last_mut() {
                    p.relative = value != "0";
                }
            }
            (Section::Profile, "Default") => {
                if let Some(p) = profiles.last_mut() {
                    p.default = value == "1";
                }
            }
            (Section::Install, "Default") => install_defaults.push(value.to_owned()),
            _ => {}
        }
    }
    profiles
        .into_iter()
        .filter(|p| !p.path.is_empty())
        .filter_map(|p| {
            let dir = if p.relative {
                if Path::new(&p.path).components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    return None;
                }
                root.join(&p.path)
            } else {
                PathBuf::from(&p.path)
            };
            let default = p.default || install_defaults.contains(&p.path);
            let name = if p.name.is_empty() {
                p.path.rsplit('/').next().unwrap_or(&p.path).to_owned()
            } else {
                p.name
            };
            Some(IniProfile { name, dir, default })
        })
        .collect()
}

/// The profiles under `root`: those of `profiles.ini` that have a `places.sqlite`, else any
/// subdirectory that has one.
pub fn profiles(root: &Path) -> Vec<IniProfile> {
    let mut found: Vec<IniProfile> = read_text_file(&root.join("profiles.ini"), 1024 * 1024)
        .map(|t| parse_profiles_ini(&t, root))
        .unwrap_or_default();
    found.retain(|p| p.dir.join("places.sqlite").is_file());
    if found.is_empty()
        && let Ok(entries) = std::fs::read_dir(root)
    {
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.join("places.sqlite").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                found.push(IniProfile { name, dir, default: false });
            }
        }
        found.sort_by(|a, b| a.dir.cmp(&b.dir));
    }
    found
}

/// Reads what `wanted` asks for from the profile directory's `places.sqlite`.
pub fn read(
    profile: &Path,
    wanted: Wanted,
    limits: &Limits,
) -> Result<(Vec<ImportBookmark>, Vec<ImportHistory>)> {
    let db = DbCopy::open(&profile.join("places.sqlite"), limits.max_db_bytes)?;
    let bookmarks = if wanted.bookmarks {
        let mut b = bookmarks(&db.conn)?;
        b.truncate(limits.max_bookmarks);
        b
    } else {
        Vec::new()
    };
    let history = if wanted.history {
        history_top(&db.conn, HISTORY_SQL, firefox_to_unix, limits.max_history_items)?
    } else {
        Vec::new()
    };
    Ok((bookmarks, history))
}

/// The site icons for the wanted hosts: `favicons.sqlite` (`moz_icons` with the pages they
/// belong to), or, in old profiles without it, `moz_favicons` of `places.sqlite`.
pub fn read_icons(profile: &Path, wanted: &WantedIcons, limits: &Limits) -> Result<Vec<ImportIcon>> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let modern = profile.join("favicons.sqlite");
    if modern.is_file() {
        let db = DbCopy::open(&modern, limits.max_db_bytes)?;
        let mut candidates = Candidates::default();
        // The pages an icon was seen on, then icons of the site itself (root icons) that no
        // page row ties to a page. Width 65535 marks SVG (cannot be rasterised here).
        for sql in [
            "SELECT p.page_url, i.id, i.width, length(i.data)
             FROM moz_icons_to_pages ip
             JOIN moz_pages_w_icons p ON p.id = ip.page_id
             JOIN moz_icons i ON i.id = ip.icon_id
             WHERE i.data IS NOT NULL",
            "SELECT icon_url, id, width, length(data) FROM moz_icons WHERE data IS NOT NULL",
        ] {
            let mut stmt = db.conn.prepare(sql)?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let side = int(row, 2);
                if side == 65535 {
                    continue;
                }
                let side = side.clamp(0, 4096) as u32;
                candidates.add(wanted, &text(row, 0), int(row, 1), side, int(row, 3), limits);
            }
        }
        let mut blob = db.conn.prepare("SELECT data FROM moz_icons WHERE id = ?1")?;
        return Ok(
            candidates.resolve(wanted, limits, |id| blob.query_row([id], |r| r.get::<_, Vec<u8>>(0)).ok())
        );
    }
    let places = profile.join("places.sqlite");
    if !places.is_file() {
        return Ok(Vec::new());
    }
    // Old profiles keep icons in `moz_favicons` of `places.sqlite`, tied to pages through
    // `moz_places.favicon_id`; newer ones have neither.
    let db = DbCopy::open(&places, limits.max_db_bytes)?;
    let Ok(mut stmt) = db.conn.prepare(
        "SELECT p.url, f.id, 0, length(f.data) FROM moz_places p
         JOIN moz_favicons f ON f.id = p.favicon_id WHERE f.data IS NOT NULL",
    ) else {
        return Ok(Vec::new());
    };
    let mut candidates = Candidates::default();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        candidates.add(wanted, &text(row, 0), int(row, 1), 0, int(row, 3), limits);
    }
    drop(rows);
    drop(stmt);
    let mut blob = db.conn.prepare("SELECT data FROM moz_favicons WHERE id = ?1")?;
    Ok(candidates.resolve(wanted, limits, |id| blob.query_row([id], |r| r.get::<_, Vec<u8>>(0)).ok()))
}

/// Counts of what is there: `(bookmarks, history entries)`.
pub fn count(profile: &Path, limits: &Limits) -> Result<(usize, usize)> {
    let db = DbCopy::open(&profile.join("places.sqlite"), limits.max_db_bytes)?;
    Ok((bookmarks(&db.conn)?.len(), history_count(&db.conn, HISTORY_SQL, firefox_to_unix)?))
}

const HISTORY_SQL: &str = "SELECT url, title, visit_count, last_visit_date FROM moz_places
    WHERE hidden = 0 AND visit_count > 0 AND last_visit_date IS NOT NULL
    ORDER BY last_visit_date DESC";

struct Node {
    parent: i64,
    kind: i64,
    title: String,
    guid: String,
}

fn bookmarks(conn: &Connection) -> Result<Vec<ImportBookmark>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.parent, b.type, b.title, b.guid, b.dateAdded, p.url
         FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk
         ORDER BY b.parent, b.position",
    )?;
    let mut rows = stmt.query([])?;
    let mut nodes: HashMap<i64, Node> = HashMap::new();
    // (id, url, added) of the bookmarks, in database order.
    let mut marks: Vec<(i64, String, i64)> = Vec::new();
    while let Some(row) = rows.next()? {
        let id = int(row, 0);
        let kind = int(row, 2);
        nodes.insert(id, Node { parent: int(row, 1), kind, title: text(row, 3), guid: text(row, 4) });
        if kind == 1 {
            marks.push((id, text(row, 6), int(row, 5)));
        }
    }
    let mut out = Vec::new();
    'marks: for (id, url, added) in marks {
        let Some(url) = clean_url(&url) else { continue };
        // Walk up to the root's child (menu, toolbar, unfiled, mobile; tags are no folders).
        let mut path: Vec<String> = Vec::new();
        let mut at = nodes.get(&id).map_or(0, |n| n.parent);
        let mut top = None;
        for _ in 0..MAX_DEPTH {
            let Some(node) = nodes.get(&at) else { break };
            if node.guid == "root________" {
                break;
            }
            if node.guid == "tags________" {
                continue 'marks;
            }
            if node.kind == 2 {
                top = Some((node.guid.as_str(), node.title.as_str()));
                path.push(clean_title(&node.title));
            }
            at = node.parent;
        }
        // `path` runs from the innermost folder up to (and including) the root's child.
        let mut folders = Vec::new();
        if let Some((guid, _)) = top {
            path.pop();
            match guid {
                "toolbar_____" => {}
                "menu________" => folders.push("Bookmarks Menu".to_owned()),
                "unfiled_____" => folders.push("Other Bookmarks".to_owned()),
                "mobile______" => folders.push("Mobile Bookmarks".to_owned()),
                _ => folders.push(clean_title(top.map_or("", |t| t.1))),
            }
        }
        folders.extend(path.into_iter().rev());
        let title = nodes.get(&id).map(|n| clean_title(&n.title)).unwrap_or_default();
        out.push(ImportBookmark { url, title, folders, added: firefox_to_unix(added) });
    }
    Ok(out)
}

/// A Firefox JSON bookmark backup (`{"guid": "root________", "children": […]}`).
pub fn parse_backup(root: &Value) -> Result<Vec<ImportBookmark>> {
    if !root.is_object() {
        return Err(ImportError::Format("not a Firefox bookmark backup".into()));
    }
    let mut out = Vec::new();
    backup_walk(root, &[], 0, true, &mut out);
    Ok(out)
}

fn backup_walk(node: &Value, folders: &[String], depth: usize, is_root: bool, out: &mut Vec<ImportBookmark>) {
    if let Some(uri) = node.get("uri").and_then(Value::as_str) {
        if let Some(url) = clean_url(uri) {
            out.push(ImportBookmark {
                url,
                title: node.get("title").and_then(Value::as_str).map(clean_title).unwrap_or_default(),
                folders: folders.to_vec(),
                added: node.get("dateAdded").and_then(Value::as_i64).map_or(0, firefox_to_unix),
            });
        }
        return;
    }
    let Some(children) = node.get("children").and_then(Value::as_array) else { return };
    if depth >= MAX_DEPTH {
        return;
    }
    for child in children {
        let inner = if is_root {
            match child.get("guid").and_then(Value::as_str) {
                Some("toolbar_____") => Vec::new(),
                Some("menu________") => vec!["Bookmarks Menu".to_owned()],
                Some("unfiled_____") => vec!["Other Bookmarks".to_owned()],
                Some("mobile______") => vec!["Mobile Bookmarks".to_owned()],
                // Tags and anything unknown.
                Some("tags________") => continue,
                _ => vec![child.get("title").and_then(Value::as_str).map(clean_title).unwrap_or_default()],
            }
        } else if child.get("children").is_some() {
            let mut f = folders.to_vec();
            f.push(child.get("title").and_then(Value::as_str).map(clean_title).unwrap_or_default());
            f
        } else {
            folders.to_vec()
        };
        backup_walk(child, &inner, depth + 1, false, out);
    }
}
