//! Detection, reading and applying, against fake home directories and fixture files.
//! Nothing here reads the real browsers of the machine.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use axion_import::fixtures::{self, Mark};
use axion_import::{
    Family, Home, ImportError, Limits, Source, Wanted, apply, detect, parse_bookmarks, read, read_file, scan,
};
use axion_store::Store;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn fake_home() -> (tempfile::TempDir, Home) {
    let dir = tempfile::tempdir().unwrap();
    fixtures::fake_home(dir.path(), NOW);
    let home = Home::new(dir.path());
    (dir, home)
}

fn by_label<'a>(sources: &'a [Source], label: &str) -> &'a Source {
    sources.iter().find(|s| s.label() == label).unwrap_or_else(|| panic!("no source {label}"))
}

fn titles(b: &[axion_import::ImportBookmark]) -> Vec<(String, String)> {
    b.iter().map(|b| (b.folder_path(), b.title.clone())).collect()
}

// ---- detection ----

#[test]
fn an_empty_home_has_nothing_to_import() {
    let dir = tempfile::tempdir().unwrap();
    assert!(detect(&Home::new(dir.path())).is_empty());
    assert!(scan(&Home::new(dir.path()), &Limits::default()).is_empty());
    // A home that does not exist is no problem either.
    assert!(detect(&Home::new(dir.path().join("missing"))).is_empty());
}

#[test]
fn detects_native_flatpak_and_firefox_profiles_newest_first() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let labels: Vec<String> = sources.iter().map(Source::label).collect();
    assert_eq!(
        labels,
        [
            "Firefox · default-release",
            "Google Chrome · Person 1",
            "Brave · Default",
            "Google Chrome · Work",
            "Chromium · Default"
        ]
    );
    // Only the most recently used one is preselected.
    assert_eq!(sources.iter().filter(|s| s.primary).count(), 1);
    assert!(sources[0].primary);
    assert_eq!(sources[0].family, Family::Firefox);
    // Flatpak profiles are told apart from native ones by their id.
    assert!(by_label(&sources, "Chromium · Default").id.contains("flatpak"));
    assert!(by_label(&sources, "Brave · Default").id.contains("native"));
    // A profile without a history file offers bookmarks only.
    let work = by_label(&sources, "Google Chrome · Work");
    assert!(work.has_bookmarks && !work.has_history);
    // Detection alone does not read the databases.
    assert!(sources.iter().all(|s| s.bookmark_count.is_none() && s.history_count.is_none()));
}

#[test]
fn scan_counts_what_each_profile_holds() {
    let (_dir, home) = fake_home();
    let sources = scan(&home, &Limits::default());
    let counts = |label: &str| {
        let s = by_label(&sources, label);
        (s.bookmark_count, s.history_count)
    };
    assert_eq!(counts("Firefox · default-release"), (Some(3), Some(5)));
    assert_eq!(counts("Google Chrome · Person 1"), (Some(3), Some(4)));
    assert_eq!(counts("Google Chrome · Work"), (Some(1), None));
    assert_eq!(counts("Brave · Default"), (Some(2), Some(2)));
    assert_eq!(counts("Chromium · Default"), (Some(1), None));
}

#[test]
fn snap_and_xdg_config_locations_are_found() {
    let dir = tempfile::tempdir().unwrap();
    // Snap Chromium, and Firefox under the XDG config directory (a custom one).
    let snap = dir.path().join("snap/chromium/common/chromium/Default");
    fixtures::chromium_bookmarks(
        &snap,
        &fixtures::chromium_file(&[fixtures::json_url("A", "https://a.test/", NOW)], &[]),
    );
    let xdg = dir.path().join("cfg");
    let firefox = xdg.join("mozilla/firefox");
    fixtures::profiles_ini(&firefox, &[("main", "x.main", false)]);
    fixtures::firefox_places(&firefox.join("x.main"), &[("https://b.test/", "B", 1, NOW)], &[]);
    let home = Home::with_config(dir.path(), &xdg);
    let sources = detect(&home);
    assert_eq!(sources.len(), 2);
    assert!(by_label(&sources, "Chromium · Default").id.contains("snap"));
    assert_eq!(by_label(&sources, "Firefox · main").dir, firefox.join("x.main"));
}

#[test]
fn profile_names_come_from_local_state_and_stay_inside_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = dir.path().join(".config/google-chrome");
    fs::create_dir_all(chrome.join("Profile 2")).unwrap();
    // A name that tries to leave the user-data directory, pointing at a real folder.
    let evil = dir.path().join(".config/evil");
    fixtures::chromium_bookmarks(&evil, &fixtures::chromium_file(&[], &[]));
    fixtures::chromium_bookmarks(&chrome.join("Profile 2"), &fixtures::chromium_file(&[], &[]));
    fs::write(
        chrome.join("Local State"),
        r#"{"profile":{"info_cache":{"Profile 2":{"name":"Zoë 🦀"},"../evil":{"name":"Evil"},"Profile 9":{"name":"Gone"}}}}"#,
    )
    .unwrap();
    let sources = detect(&Home::new(dir.path()));
    let labels: Vec<String> = sources.iter().map(Source::label).collect();
    assert_eq!(labels, ["Google Chrome · Zoë 🦀"]);
}

#[test]
fn opera_keeps_its_profile_in_the_base_directory() {
    let dir = tempfile::tempdir().unwrap();
    let opera = dir.path().join(".config/opera");
    fixtures::chromium_bookmarks(
        &opera,
        &fixtures::chromium_file(&[fixtures::json_url("A", "https://a.test/", NOW)], &[]),
    );
    let sources = detect(&Home::new(dir.path()));
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].browser, "Opera");
    assert_eq!(sources[0].dir, opera);
}

#[test]
fn firefox_without_profiles_ini_is_found_by_its_databases() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".librewolf");
    fixtures::firefox_places(&root.join("zzz.default"), &[("https://b.test/", "B", 1, NOW)], &[]);
    let sources = detect(&Home::new(dir.path()));
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].browser, "LibreWolf");
}

#[test]
fn profiles_ini_parsing() {
    let dir = tempfile::tempdir().unwrap();
    // Absolute path, a parent-directory escape, and a BOM and comments.
    fs::write(
        dir.path().join("profiles.ini"),
        "\u{feff}; comment\n[Profile0]\nName=abs\nIsRelative=0\nPath=/somewhere/else\n\n[Profile1]\nName=escape\nIsRelative=1\nPath=../../etc\n\n[Profile2]\nName=ok\nPath=ok.dir\nDefault=1\n\n[Install4F96D1932A9F858E]\nDefault=ok.dir\n",
    )
    .unwrap();
    // Through detection: only profiles that exist and stay inside the root appear.
    let root = dir.path().join(".mozilla/firefox");
    fs::create_dir_all(&root).unwrap();
    fs::copy(dir.path().join("profiles.ini"), root.join("profiles.ini")).unwrap();
    fixtures::firefox_places(&root.join("ok.dir"), &[("https://b.test/", "B", 1, NOW)], &[]);
    let sources = detect(&Home::new(dir.path()));
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].profile, "ok");
}

// ---- reading ----

#[test]
fn reads_chromium_bookmarks_and_history_with_the_right_epochs() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let chrome = by_label(&sources, "Google Chrome · Person 1");
    let data = read(chrome, Wanted::ALL, &Limits::default()).unwrap();
    assert_eq!(
        titles(&data.bookmarks),
        [
            ("".to_owned(), "Rust Programming Language".to_owned()),
            ("Reading".to_owned(), "Hacker News".to_owned()),
            ("Other Bookmarks".to_owned(), "Wikipedia – Zürich".to_owned()),
        ]
    );
    assert_eq!(data.bookmarks[0].added, NOW - 400 * DAY);
    assert_eq!(data.history.len(), 4);
    // Newest first, WebKit microseconds converted exactly.
    assert_eq!(data.history[0].url, "https://www.rust-lang.org/");
    assert_eq!(data.history[0].last_visit, NOW - 2 * 3600);
    assert_eq!(data.history[0].visit_count, 12);
    assert!(data.history.windows(2).all(|w| w[0].last_visit >= w[1].last_visit));
    let unicode = data.history.iter().find(|h| h.url.contains("日本語")).unwrap();
    assert_eq!(unicode.title, "例え 🦀");
    assert_eq!(unicode.last_visit, NOW - 3 * DAY);
}

#[test]
fn reads_firefox_places_with_folders_and_epochs() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let firefox = by_label(&sources, "Firefox · default-release");
    let data = read(firefox, Wanted::ALL, &Limits::default()).unwrap();
    let mut b = titles(&data.bookmarks);
    b.sort();
    assert_eq!(
        b,
        [
            ("".to_owned(), "Mozilla".to_owned()),
            ("Bookmarks Menu".to_owned(), "LWN.net".to_owned()),
            ("Dev".to_owned(), "MDN Web Docs".to_owned()),
        ]
    );
    assert!(data.bookmarks.iter().any(|b| b.added == NOW - 30 * DAY));
    assert_eq!(data.history.len(), 5);
    assert_eq!(data.history[0].last_visit, NOW - 3600);
    assert_eq!(data.history[4].title, "Straße & Café");
    assert_eq!(data.history[4].url, "https://example.org/ä?q=1&b=2");
}

#[test]
fn wanted_selects_the_kinds() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    for label in ["Firefox · default-release", "Google Chrome · Person 1"] {
        let only_bookmarks =
            read(by_label(&sources, label), Wanted { bookmarks: true, history: false }, &Limits::default())
                .unwrap();
        assert!(!only_bookmarks.bookmarks.is_empty() && only_bookmarks.history.is_empty(), "{label}");
        let only_history =
            read(by_label(&sources, label), Wanted { bookmarks: false, history: true }, &Limits::default())
                .unwrap();
        assert!(only_history.bookmarks.is_empty() && !only_history.history.is_empty(), "{label}");
    }
}

#[test]
fn the_history_limit_keeps_the_most_recent_entries() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let limits = Limits { max_history_items: 2, ..Limits::default() };
    for label in ["Firefox · default-release", "Google Chrome · Person 1"] {
        let data = read(by_label(&sources, label), Wanted::ALL, &limits).unwrap();
        assert_eq!(data.history.len(), 2, "{label}");
        let all = read(by_label(&sources, label), Wanted::ALL, &Limits::default()).unwrap();
        assert_eq!(data.history, all.history[..2]);
    }
    let none = Limits { max_history_items: 0, ..Limits::default() };
    assert!(read(by_label(&sources, "Brave · Default"), Wanted::ALL, &none).unwrap().history.is_empty());
}

/// Every file under `dir` with its bytes.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn reading_never_changes_the_original_files() {
    let (dir, home) = fake_home();
    let before = snapshot(dir.path());
    for source in scan(&home, &Limits::default()) {
        read(&source, Wanted::ALL, &Limits::default()).unwrap();
    }
    assert_eq!(snapshot(dir.path()), before, "files were changed or added");
}

#[test]
fn a_database_in_wal_mode_and_locked_by_its_browser_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".mozilla/firefox");
    let profile = root.join("x.default");
    fixtures::profiles_ini(&root, &[("default", "x.default", true)]);
    fixtures::firefox_places(&profile, &[("https://old.test/", "Old", 1, NOW - 10 * DAY)], &[]);
    // The browser is running: WAL mode, a fresh row only in the log, an exclusive lock held.
    let browser = rusqlite::Connection::open(profile.join("places.sqlite")).unwrap();
    browser.pragma_update(None, "journal_mode", "WAL").unwrap();
    browser.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    browser
        .execute(
            "INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES ('https://new.test/', 'New', 1, ?1)",
            [NOW * 1_000_000],
        )
        .unwrap();
    assert!(profile.join("places.sqlite-wal").exists());
    browser.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let sources = detect(&Home::new(dir.path()));
    let data = read(&sources[0], Wanted::ALL, &Limits::default()).unwrap();
    let urls: Vec<&str> = data.history.iter().map(|h| h.url.as_str()).collect();
    assert_eq!(urls, ["https://new.test/", "https://old.test/"]);
    drop(browser);
}

#[test]
fn databases_over_the_size_limit_are_not_copied() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let limits = Limits { max_db_bytes: 10, ..Limits::default() };
    let brave = by_label(&sources, "Brave · Default");
    // History is refused, bookmarks (a JSON file) still work: a warning, not a failure.
    let data = read(brave, Wanted::ALL, &limits).unwrap();
    assert!(data.history.is_empty());
    assert_eq!(data.bookmarks.len(), 2);
    assert_eq!(data.warnings.len(), 1);
    // When nothing asked for can be read, it is an error.
    let err = read(brave, Wanted { bookmarks: false, history: true }, &limits).unwrap_err();
    assert!(matches!(err, ImportError::TooLarge { .. }), "{err}");
}

// ---- applying ----

#[test]
fn applying_adds_once_and_dedupes_by_url() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let store = Store::in_memory().unwrap();
    let chrome =
        read(by_label(&sources, "Google Chrome · Person 1"), Wanted::ALL, &Limits::default()).unwrap();
    let first = apply(&store, &chrome, NOW).unwrap();
    assert_eq!((first.bookmarks_added, first.bookmarks_existing), (3, 0));
    assert_eq!((first.history_added, first.history_existing), (4, 0));

    let bookmarks = store.bookmarks().unwrap();
    assert_eq!(bookmarks.len(), 3);
    let hn = bookmarks.iter().find(|b| b.title == "Hacker News").unwrap();
    assert_eq!(hn.folder_path, "Reading");
    assert!(bookmarks.iter().any(|b| b.folder_path.is_empty()), "the bar bookmark stays top level");
    assert!(store.bookmark_for("https://www.rust-lang.org/").unwrap().is_some());
    let history = store.recent_history(10).unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].last_visit, NOW - 2 * 3600);
    assert_eq!(history[0].visit_count, 12);

    // Brave shares one bookmark and none of the history.
    let brave = read(by_label(&sources, "Brave · Default"), Wanted::ALL, &Limits::default()).unwrap();
    let second = apply(&store, &brave, NOW).unwrap();
    assert_eq!((second.bookmarks_added, second.bookmarks_existing), (1, 1));
    assert_eq!((second.history_added, second.history_existing), (2, 0));

    // Importing the same thing again changes nothing.
    let again = apply(&store, &chrome, NOW).unwrap();
    assert_eq!((again.bookmarks_added, again.bookmarks_existing), (0, 3));
    assert_eq!((again.history_added, again.history_existing), (0, 4));
    assert_eq!(store.bookmarks().unwrap().len(), 4);
    assert_eq!(store.recent_history(100).unwrap().len(), 6);
}

#[test]
fn existing_history_keeps_its_own_entry() {
    let store = Store::in_memory().unwrap();
    store.record_visit("https://www.rust-lang.org/", "Mine", NOW - 5).unwrap();
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let chrome =
        read(by_label(&sources, "Google Chrome · Person 1"), Wanted::ALL, &Limits::default()).unwrap();
    let report = apply(&store, &chrome, NOW).unwrap();
    assert_eq!((report.history_added, report.history_existing), (3, 1));
    let entry = store
        .recent_history(10)
        .unwrap()
        .into_iter()
        .find(|e| e.url == "https://www.rust-lang.org/")
        .unwrap();
    assert_eq!((entry.title.as_str(), entry.visit_count, entry.last_visit), ("Mine", 1, NOW - 5));
}

#[test]
fn imported_history_feeds_the_top_sites_and_autocomplete() {
    let store = Store::in_memory().unwrap();
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let ff = read(by_label(&sources, "Firefox · default-release"), Wanted::ALL, &Limits::default()).unwrap();
    apply(&store, &ff, NOW).unwrap();
    let top = store.top_sites(10, NOW, 90).unwrap();
    assert!(top.iter().any(|t| t.host == "mozilla.org"), "{top:?}");
    assert!(store.autocomplete("kern").unwrap().is_some());
}

// ---- files ----

#[test]
fn chrome_html_export() {
    let b = read_file(&fixture("chrome-export.html"), &Limits::default()).unwrap().bookmarks;
    assert_eq!(
        titles(&b),
        [
            ("".to_owned(), "Rust & Cargo".to_owned()),
            ("Dev".to_owned(), "docs.rs <crates>".to_owned()),
            ("Dev / Deep".to_owned(), "Deep ä中日".to_owned()),
            ("Other bookmarks".to_owned(), "Zürich – Wikipedia".to_owned()),
        ]
    );
    assert_eq!(b[0].added, 1_700_000_000);
    assert_eq!(b[1].url, "https://docs.rs/?a=1&b=2");
    // javascript: and chrome: links are dropped.
    assert!(b.iter().all(|b| b.url.starts_with("https://")));
}

#[test]
fn firefox_html_export_with_dates_in_any_unit() {
    let b = read_file(&fixture("firefox-export.html"), &Limits::default()).unwrap().bookmarks;
    assert_eq!(
        titles(&b),
        [
            ("Mozilla Firefox".to_owned(), "Get Help".to_owned()),
            ("".to_owned(), "LWN.net".to_owned()),
            ("Other Bookmarks".to_owned(), "kernel.org".to_owned()),
        ]
    );
    assert_eq!(b[0].added, 1_650_000_000);
    assert_eq!(b[1].added, 1_650_000_010);
    assert_eq!(b[2].added, 1_650_000_020);
}

#[test]
fn sloppy_html_still_yields_its_links() {
    let b = read_file(&fixture("loose.html"), &Limits::default()).unwrap().bookmarks;
    let urls: Vec<&str> = b.iter().map(|b| b.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://single-quoted.example/",
            "https://unquoted.example/path",
            "https://unclosed.example/",
            "https://after-folder.example/"
        ]
    );
    // Links outside any folder are filed under "Imported", not on the bar.
    assert!(b.iter().all(|b| b.folders == ["Imported"]));
    assert_eq!(b[2].title, "No closing tag");
}

#[test]
fn chromium_bookmarks_file() {
    let b = read_file(&fixture("chromium-Bookmarks.json"), &Limits::default()).unwrap().bookmarks;
    assert_eq!(
        titles(&b),
        [
            ("".to_owned(), "Tab  title with newline 🦀".to_owned()),
            ("Folder / with slash".to_owned(), "Nested".to_owned()),
            ("Other Bookmarks".to_owned(), String::new()),
        ]
    );
    assert_eq!(b[0].added, 1_700_000_000);
    assert_eq!(b[1].added, 1_700_000_000);
}

#[test]
fn firefox_json_backup() {
    let b = read_file(&fixture("firefox-backup.json"), &Limits::default()).unwrap().bookmarks;
    assert_eq!(
        titles(&b),
        [
            ("Bookmarks Menu".to_owned(), "Menu entry".to_owned()),
            ("".to_owned(), "Bar entry".to_owned()),
            ("Sub".to_owned(), "In sub".to_owned()),
        ]
    );
    assert_eq!(b[0].added, 1_650_000_000);
}

#[test]
fn the_bookmark_limit_applies_to_files() {
    let limits = Limits { max_bookmarks: 1, ..Limits::default() };
    assert_eq!(read_file(&fixture("chrome-export.html"), &limits).unwrap().bookmarks.len(), 1);
}

// ---- malformed input: errors, never panics ----

#[test]
fn garbage_files_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits::default();
    let write = |name: &str, bytes: &[u8]| {
        let p = dir.path().join(name);
        fs::write(&p, bytes).unwrap();
        p
    };
    for (name, bytes) in [
        ("empty", &b""[..]),
        ("text", b"hello, this is not a bookmarks file"),
        ("binary", &[0xff, 0xfe, 0x00, 0x01, 0x80, 0x81][..]),
        ("json-scalar", b"42"),
        ("json-other", br#"{"hello":"world"}"#),
        ("json-truncated", br#"{"roots":{"bookmark_bar":{"children":[{"type":"ur"#),
        ("json-roots-wrong", br#"{"roots":5}"#),
    ] {
        let err = read_file(&write(name, bytes), &limits);
        assert!(err.is_err(), "{name} should be refused");
    }
    // Missing file: an I/O error.
    assert!(matches!(read_file(&dir.path().join("nope"), &limits), Err(ImportError::Io { .. })));
    // Oversized file.
    let big = write("big.html", &[b'<'; 100]);
    let tiny = Limits { max_text_bytes: 10, ..Limits::default() };
    assert!(matches!(read_file(&big, &tiny), Err(ImportError::TooLarge { .. })));
}

#[test]
fn odd_but_valid_json_yields_no_bookmarks_without_panicking() {
    let limits = Limits::default();
    for text in [
        r#"{"roots":{}}"#,
        r#"{"roots":{"bookmark_bar":5}}"#,
        r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","url":123},{"type":"url"},{"type":7},null,[]]}}}"#,
        r#"{"roots":{"bookmark_bar":{"children":[{"type":"folder","children":"x"}]}}}"#,
        r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","url":"https://x.test/","date_added":"not a number"}]}}}"#,
    ] {
        let b = parse_bookmarks(text, &limits).unwrap();
        assert!(b.len() <= 1, "{text}");
    }
}

#[test]
fn deep_nesting_is_cut_off() {
    let limits = Limits::default();
    // JSON nested far beyond serde_json's recursion limit: an error.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(parse_bookmarks(&deep, &limits).is_err());
    // Folders nested beyond the importer's depth: parsed, the excess dropped.
    let mut json = String::new();
    for _ in 0..100 {
        json.push_str(r#"{"type":"folder","name":"f","children":["#);
    }
    json.push_str(r#"{"type":"url","name":"deep","url":"https://deep.test/"}"#);
    json.push_str(&"]}".repeat(100));
    let text = format!(r#"{{"roots":{{"bookmark_bar":{{"children":[{json}]}}}}}}"#);
    // 100 levels plus the roots are within serde_json's limit of 128 only partly; either an
    // error or a result is fine, a panic or hang is not.
    let _ = parse_bookmarks(&text, &limits);
    // HTML lists nested very deeply.
    let html = format!(
        "<DL>{}<DT><A HREF=\"https://x.test/\">x</A>{}",
        "<DT><H3>f</H3><DL>".repeat(5000),
        "</DL>".repeat(5000)
    );
    let b = parse_bookmarks(&html, &limits).unwrap();
    assert_eq!(b.len(), 1);
    assert!(b[0].folders.len() <= 64);
}

#[test]
fn html_with_unterminated_pieces_terminates() {
    let limits = Limits::default();
    for html in [
        "<DL><DT><A HREF=\"https://x.test/\" ADD_DATE=\"",
        "<DL><DT><A HREF=\"https://x.test/",
        "<DL><DT><H3>never closed",
        "<<<<<<<<<<<<<<<<<<<<<<<",
        "<DL><A HREF=>x</A>",
        "<DL><A HREF='https://x.test/'ADD_DATE=1>x</A></DL></DL></DL>",
        "<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><DT><A HREF=\"https://é.test/\">\u{0}\u{202e}t</A>",
    ] {
        let _ = parse_bookmarks(html, &limits);
    }
    // Many unclosed headings: no quadratic blowup.
    let many = format!("<DL>{}", "<DT><H3>x".repeat(200_000));
    let started = std::time::Instant::now();
    let _ = parse_bookmarks(&many, &limits);
    assert!(started.elapsed().as_secs() < 10, "took {:?}", started.elapsed());
}

fn profile_with(name: &str, bytes: &[u8]) -> (tempfile::TempDir, Source) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".mozilla/firefox/x.default");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(name), bytes).unwrap();
    let sources = detect(&Home::new(dir.path()));
    let source = sources.into_iter().next().expect("detected");
    (dir, source)
}

#[test]
fn broken_databases_are_errors() {
    let (_d, valid) = {
        let (dir, home) = fake_home();
        let s = by_label(&detect(&home), "Firefox · default-release").clone();
        (dir, s)
    };
    let good = fs::read(valid.dir.join("places.sqlite")).unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("garbage", b"this is definitely not an sqlite database, just text padding".repeat(100)),
        ("truncated", good[..good.len() / 3].to_vec()),
        ("header-only", good[..100].to_vec()),
        ("zeros", vec![0; 8192]),
    ];
    for (name, bytes) in cases {
        let (_dir, source) = profile_with("places.sqlite", &bytes);
        let result = read(&source, Wanted::ALL, &Limits::default());
        // An empty file is a valid empty database without tables: also an error.
        assert!(result.is_err(), "{name} should fail, got {result:?}");
    }
}

/// Deterministic xorshift, for fuzz-like mutations.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn mutate(rng: &mut Rng, data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    if out.is_empty() {
        return out;
    }
    for _ in 0..1 + rng.next() % 8 {
        let i = (rng.next() as usize) % out.len();
        match rng.next() % 3 {
            0 => out[i] = rng.next() as u8,
            1 => out[i] ^= 1 << (rng.next() % 8),
            _ => out.truncate(i.max(1)),
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

#[test]
fn mutated_bookmark_files_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let limits = Limits::default();
    for name in [
        "chrome-export.html",
        "firefox-export.html",
        "loose.html",
        "chromium-Bookmarks.json",
        "firefox-backup.json",
    ] {
        let original = fs::read(fixture(name)).unwrap();
        for _ in 0..300 {
            let mutated = mutate(&mut rng, &original);
            let _ = parse_bookmarks(&String::from_utf8_lossy(&mutated), &limits);
        }
    }
}

#[test]
fn mutated_databases_never_panic() {
    let (_dir, home) = fake_home();
    let sources = detect(&home);
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for label in ["Firefox · default-release", "Google Chrome · Person 1"] {
        let source = by_label(&sources, label);
        let file = if source.family == Family::Firefox { "places.sqlite" } else { "History" };
        let original = fs::read(source.dir.join(file)).unwrap();
        for _ in 0..60 {
            let target = tempfile::tempdir().unwrap();
            let copy = Source { dir: target.path().to_owned(), ..source.clone() };
            fs::write(target.path().join(file), mutate(&mut rng, &original)).unwrap();
            if source.family == Family::Chromium {
                fs::write(target.path().join("Bookmarks"), "{}").unwrap();
            }
            let _ = read(&copy, Wanted::ALL, &Limits::default());
        }
    }
}

#[test]
fn firefox_rows_with_wrong_types_do_not_break_the_read() {
    // Real databases hold surprises: numbers as text, NULL titles, invalid UTF-8.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".mozilla/firefox/x.default");
    fixtures::firefox_places(
        &root,
        &[],
        &[Mark {
            id: 10,
            kind: 1,
            parent: 3,
            title: "t",
            guid: "g___________",
            url: Some("https://x.test/"),
            unix: NOW,
        }],
    );
    let conn = rusqlite::Connection::open(root.join("places.sqlite")).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES ('https://text-number.test/', NULL, '7', '1700000000000000');
         INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES ('https://float.test/', 'F', 2, 1700000000000000.0);
         INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES (NULL, 'no url', 1, 1700000000000000);
         INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES ('https://future.test/', 'F', 1, 9223372036854775807);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO moz_places (url, title, visit_count, last_visit_date) VALUES ('https://blob.test/', ?1, 1, 1700000000000000)",
        [vec![0x66u8, 0xff, 0x6f]],
    )
    .unwrap();
    drop(conn);
    let source = detect(&Home::new(dir.path())).remove(0);
    let data = read(&source, Wanted::ALL, &Limits::default()).unwrap();
    let urls: Vec<&str> = data.history.iter().map(|h| h.url.as_str()).collect();
    assert!(urls.contains(&"https://text-number.test/"));
    assert!(urls.contains(&"https://float.test/"));
    assert!(urls.contains(&"https://blob.test/"));
    assert!(!urls.contains(&"https://future.test/"), "an absurd time is dropped");
    let text_number = data.history.iter().find(|h| h.url.contains("text-number")).unwrap();
    assert_eq!((text_number.visit_count, text_number.last_visit), (7, 1_700_000_000));
    assert_eq!(text_number.title, "");
    let blob = data.history.iter().find(|h| h.url.contains("blob")).unwrap();
    assert!(blob.title.starts_with('f') && blob.title.ends_with('o'));
}
