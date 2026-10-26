//! Builders for fake browser profiles, used by the tests and by captures (see
//! `examples/fake_home.rs`). Not part of the API: nothing here is used at runtime.

// Fixture builders: a failing step should panic loudly, so they `expect`.
#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;

use rusqlite::{Connection, params};

const WEBKIT_OFFSET: i64 = 11_644_473_600;

/// Chromium's time for unix seconds.
pub fn webkit(unix: i64) -> i64 {
    (unix + WEBKIT_OFFSET) * 1_000_000
}

/// Creates `dir` if needed and returns `dir/name`.
fn file_in(dir: &Path, name: &str) -> std::path::PathBuf {
    let _ = fs::create_dir_all(dir);
    dir.join(name)
}

/// A Chromium `History` database with `(url, title, visit_count, unix time)` rows.
pub fn chromium_history(dir: &Path, rows: &[(&str, &str, i64, i64)]) {
    let conn = Connection::open(file_in(dir, "History")).expect("fixture db");
    conn.execute_batch(
        "CREATE TABLE urls (id INTEGER PRIMARY KEY AUTOINCREMENT, url LONGVARCHAR, title LONGVARCHAR,
            visit_count INTEGER DEFAULT 0 NOT NULL, typed_count INTEGER DEFAULT 0 NOT NULL,
            last_visit_time INTEGER NOT NULL, hidden INTEGER DEFAULT 0 NOT NULL);",
    )
    .expect("fixture schema");
    for (url, title, visits, at) in rows {
        conn.execute(
            "INSERT INTO urls (url, title, visit_count, last_visit_time) VALUES (?1, ?2, ?3, ?4)",
            params![url, title, visits, webkit(*at)],
        )
        .expect("fixture row");
    }
}

/// A Chromium `Bookmarks` file from its JSON text.
pub fn chromium_bookmarks(dir: &Path, json: &str) {
    fs::write(file_in(dir, "Bookmarks"), json).expect("fixture file");
}

/// A bookmark node of a Chromium `Bookmarks` file.
pub fn json_url(name: &str, url: &str, unix: i64) -> String {
    format!(r#"{{"type":"url","name":{name:?},"url":{url:?},"date_added":"{}","id":"1"}}"#, webkit(unix))
}

/// A folder node of a Chromium `Bookmarks` file (children already rendered).
pub fn json_folder(name: &str, children: &[String]) -> String {
    format!(r#"{{"type":"folder","name":{name:?},"children":[{}],"id":"2"}}"#, children.join(","))
}

/// A whole `Bookmarks` file from the children of the bar and of "other bookmarks".
pub fn chromium_file(bar: &[String], other: &[String]) -> String {
    format!(
        r#"{{"checksum":"x","roots":{{"bookmark_bar":{{"type":"folder","name":"Bookmarks bar","children":[{}]}},
        "other":{{"type":"folder","name":"Other bookmarks","children":[{}]}},
        "synced":{{"type":"folder","name":"Mobile bookmarks","children":[]}}}},"version":1}}"#,
        bar.join(","),
        other.join(",")
    )
}

/// A row of Firefox's `moz_bookmarks`.
pub struct Mark<'a> {
    pub id: i64,
    /// 1 bookmark, 2 folder.
    pub kind: i64,
    pub parent: i64,
    pub title: &'a str,
    pub guid: &'a str,
    /// Bookmarked page (kind 1).
    pub url: Option<&'a str>,
    pub unix: i64,
}

/// Firefox's `places.sqlite` with history rows `(url, title, visit_count, unix time)` and
/// bookmark rows (the standard roots 1..=6 are created).
pub fn firefox_places(dir: &Path, history: &[(&str, &str, i64, i64)], marks: &[Mark<'_>]) {
    let conn = Connection::open(file_in(dir, "places.sqlite")).expect("fixture db");
    conn.execute_batch(
        "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR,
            visit_count INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0 NOT NULL, last_visit_date INTEGER);
         CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER DEFAULT NULL,
            parent INTEGER, position INTEGER, title LONGVARCHAR, dateAdded INTEGER, guid TEXT);
         INSERT INTO moz_bookmarks (id, type, parent, position, title, guid) VALUES
            (1, 2, 0, 0, '', 'root________'), (2, 2, 1, 0, 'menu', 'menu________'),
            (3, 2, 1, 1, 'toolbar', 'toolbar_____'), (4, 2, 1, 2, 'tags', 'tags________'),
            (5, 2, 1, 3, 'unfiled', 'unfiled_____'), (6, 2, 1, 4, 'mobile', 'mobile______');",
    )
    .expect("fixture schema");
    for (url, title, visits, at) in history {
        conn.execute(
            "INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES (?1, ?2, ?3, ?4)",
            params![url, title, visits, at * 1_000_000],
        )
        .expect("fixture row");
    }
    for (i, m) in marks.iter().enumerate() {
        let fk = m.url.map(|url| {
            conn.execute(
                "INSERT INTO moz_places (url, title, visit_count) VALUES (?1, ?2, 0)",
                params![url, m.title],
            )
            .expect("fixture place");
            conn.last_insert_rowid()
        });
        conn.execute(
            "INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![m.id, m.kind, fk, m.parent, i as i64, m.title, m.unix * 1_000_000, m.guid],
        )
        .expect("fixture bookmark");
    }
}

/// `profiles.ini` for the given `(name, relative path, is default)` profiles.
pub fn profiles_ini(root: &Path, profiles: &[(&str, &str, bool)]) {
    let mut text = String::new();
    for (i, (name, path, default)) in profiles.iter().enumerate() {
        text.push_str(&format!("[Profile{i}]\nName={name}\nIsRelative=1\nPath={path}\n"));
        if *default {
            text.push_str("Default=1\n");
        }
        text.push('\n');
    }
    text.push_str("[General]\nStartWithLastProfile=1\nVersion=2\n");
    fs::write(file_in(root, "profiles.ini"), text).expect("fixture ini");
}

/// Sets a file's modification time (`unix` seconds), so the order of detected browsers is
/// the one a test expects.
pub fn touch(path: &Path, unix: i64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix.max(0) as u64);
    if let Ok(file) = fs::OpenOptions::new().append(true).open(path) {
        let _ = file.set_modified(time);
    }
}

/// A fake home with four browsers, for tests and captures. `now` is the time the history is
/// relative to. Contents:
/// - Google Chrome, profiles "Person 1" (Default: 3 bookmarks, one in a folder, 4 history
///   entries) and "Work" (Profile 1: bookmarks only, 1 bookmark);
/// - Brave (Default: 2 bookmarks, 2 history entries);
/// - Firefox (profile "default-release": 3 bookmarks incl. one in a nested toolbar folder and
///   one in the menu, 5 history entries; the newest data, so it is the primary source);
/// - Chromium as a Flatpak (Default: 1 bookmark, no history).
pub fn fake_home(root: &Path, now: i64) {
    let day = 86_400;
    // Chrome.
    let chrome = root.join(".config/google-chrome");
    fs::create_dir_all(&chrome).expect("fixture dir");
    fs::write(
        chrome.join("Local State"),
        r#"{"profile":{"info_cache":{"Default":{"name":"Person 1"},"Profile 1":{"name":"Work"}}}}"#,
    )
    .expect("fixture file");
    let default = chrome.join("Default");
    chromium_bookmarks(
        &default,
        &chromium_file(
            &[
                json_url("Rust Programming Language", "https://www.rust-lang.org/", now - 400 * day),
                json_folder(
                    "Reading",
                    &[json_url("Hacker News", "https://news.ycombinator.com/", now - 300 * day)],
                ),
            ],
            &[json_url("Wikipedia – Zürich", "https://de.wikipedia.org/wiki/Z%C3%BCrich", now - 200 * day)],
        ),
    );
    chromium_history(
        &default,
        &[
            ("https://www.rust-lang.org/", "Rust Programming Language", 12, now - 2 * 3600),
            ("https://docs.rs/rusqlite/", "rusqlite - Rust", 4, now - day),
            ("https://example.com/日本語", "例え 🦀", 2, now - 3 * day),
            ("https://news.ycombinator.com/", "Hacker News", 30, now - 5 * day),
        ],
    );
    chromium_bookmarks(
        &chrome.join("Profile 1"),
        &chromium_file(&[json_url("Intranet", "https://intranet.example.org/", now - 50 * day)], &[]),
    );
    // Brave.
    let brave = root.join(".config/BraveSoftware/Brave-Browser/Default");
    chromium_bookmarks(
        &brave,
        &chromium_file(
            &[
                json_url("Brave", "https://brave.com/", now - 90 * day),
                json_url("Rust Programming Language", "https://www.rust-lang.org/", now - 80 * day),
            ],
            &[],
        ),
    );
    chromium_history(
        &brave,
        &[
            ("https://brave.com/", "Brave", 3, now - 6 * day),
            ("https://github.com/", "GitHub", 40, now - 4 * day),
        ],
    );
    // Firefox.
    let firefox = root.join(".mozilla/firefox");
    let profile = firefox.join("abcd1234.default-release");
    profiles_ini(&firefox, &[("default-release", "abcd1234.default-release", true)]);
    firefox_places(
        &profile,
        &[
            ("https://www.mozilla.org/", "Mozilla", 5, now - 3600),
            ("https://developer.mozilla.org/en-US/", "MDN Web Docs", 25, now - 2 * day),
            ("https://www.kernel.org/", "The Linux Kernel Archives", 2, now - 7 * day),
            ("https://lwn.net/", "LWN.net", 9, now - 9 * day),
            ("https://example.org/ä?q=1&b=2", "Straße & Café", 1, now - 11 * day),
        ],
        &[
            Mark { id: 10, kind: 2, parent: 3, title: "Dev", guid: "devfolder___", url: None, unix: now },
            Mark {
                id: 11,
                kind: 1,
                parent: 3,
                title: "Mozilla",
                guid: "m1__________",
                url: Some("https://www.mozilla.org/"),
                unix: now - 30 * day,
            },
            Mark {
                id: 12,
                kind: 1,
                parent: 10,
                title: "MDN Web Docs",
                guid: "m2__________",
                url: Some("https://developer.mozilla.org/en-US/"),
                unix: now - 20 * day,
            },
            Mark {
                id: 13,
                kind: 1,
                parent: 2,
                title: "LWN.net",
                guid: "m3__________",
                url: Some("https://lwn.net/"),
                unix: now - 10 * day,
            },
        ],
    );
    // Chromium (Flatpak).
    let flatpak = root.join(".var/app/org.chromium.Chromium/config/chromium/Default");
    chromium_bookmarks(
        &flatpak,
        &chromium_file(&[json_url("Debian", "https://www.debian.org/", now - day)], &[]),
    );
    // Newest first: Firefox, then Chrome, Brave, Chromium.
    touch(&profile.join("places.sqlite"), now - 100);
    touch(&default.join("History"), now - 200);
    touch(&brave.join("History"), now - 300);
    touch(&default.join("Bookmarks"), now - 200);
    touch(&chrome.join("Profile 1/Bookmarks"), now - 1000);
    touch(&brave.join("Bookmarks"), now - 300);
    touch(&flatpak.join("Bookmarks"), now - 5000);
}
