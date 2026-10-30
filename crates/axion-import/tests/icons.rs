//! Site icons: Chromium's `Favicons`, Firefox's `favicons.sqlite` (and the old `moz_favicons`),
//! exported HTML with `ICON` data URIs, and writing them into the icon cache. All data is
//! synthetic; nothing here reads a real browser.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use axion_import::fixtures::{self, chromium_file, json_url};
use axion_import::icon_fixtures::{
    ChromiumIcon, chromium_favicons, firefox_favicons, firefox_legacy_favicons, tiny_ico, tiny_png,
};
use axion_import::{Home, ImportIcon, Limits, Wanted, apply, detect, read, read_file, write_icons};
use axion_store::Store;

const NOW: i64 = 1_800_000_000;

fn rgba_of(icon: &ImportIcon) -> [u8; 4] {
    let img = image::load_from_memory(&icon.png).unwrap().into_rgba8();
    img.get_pixel(0, 0).0
}

fn icon_of<'a>(icons: &'a [ImportIcon], host: &str) -> &'a ImportIcon {
    icons.iter().find(|i| i.host == host).unwrap_or_else(|| panic!("no icon for {host}"))
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// A fake Chrome profile with two bookmarks and two history entries, returns the home.
fn chrome_home(favicons: &[ChromiumIcon<'_>]) -> (tempfile::TempDir, Home) {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join(".config/google-chrome/Default");
    fixtures::chromium_bookmarks(
        &profile,
        &chromium_file(
            &[
                json_url("Rust", "https://www.rust-lang.org/learn", NOW),
                json_url("Example", "https://example.com/a", NOW),
            ],
            &[],
        ),
    );
    fixtures::chromium_history(
        &profile,
        &[
            ("https://docs.rs/rusqlite/", "rusqlite", 3, NOW - 100),
            ("https://unrelated.test/", "Unrelated", 1, NOW - 200),
        ],
    );
    chromium_favicons(&profile, favicons);
    let home = Home::new(dir.path());
    (dir, home)
}

fn chrome_icons(favicons: &[ChromiumIcon<'_>], limits: &Limits) -> Vec<ImportIcon> {
    let (_dir, home) = chrome_home(favicons);
    let sources = detect(&home);
    assert_eq!(sources.len(), 1);
    read(&sources[0], Wanted::ALL, limits).unwrap().icons
}

#[test]
fn chromium_exact_page_host_fallback_and_best_size() {
    let icons = chrome_icons(
        &[
            // Exact page of the Rust bookmark; 16, 32 and 64 px: the 32 px one is chosen.
            ChromiumIcon {
                pages: &["https://www.rust-lang.org/learn"],
                bitmaps: &[
                    (16, 16, tiny_png(16, RED)),
                    (32, 32, tiny_png(32, GREEN)),
                    (64, 64, tiny_png(64, BLUE)),
                ],
            },
            // Another page of the same host: loses against the exact page.
            ChromiumIcon {
                pages: &["https://www.rust-lang.org/"],
                bitmaps: &[(32, 32, tiny_png(32, YELLOW))],
            },
            // Only the site's front page is mapped; the history entry is a deeper page: host fallback.
            ChromiumIcon { pages: &["https://docs.rs/"], bitmaps: &[(180, 180, tiny_png(180, BLUE))] },
            // A host nobody imported.
            ChromiumIcon { pages: &["https://not-imported.test/"], bitmaps: &[(32, 32, tiny_png(32, RED))] },
        ],
        &Limits::default(),
    );
    assert_eq!(icons.len(), 2, "{icons:?}");
    let rust = icon_of(&icons, "rust-lang.org");
    assert_eq!((rust.width, rust.height), (32, 32));
    assert_eq!(rgba_of(rust), GREEN);
    // 180 px is scaled down to the 64 px cap.
    let docs = icon_of(&icons, "docs.rs");
    assert_eq!((docs.width, docs.height), (64, 64));
    assert_eq!(rgba_of(docs), BLUE);
}

#[test]
fn chromium_skips_corrupt_and_oversized_blobs_and_tries_the_next() {
    let big = {
        // A valid PNG that is larger than the byte limit set below.
        let mut v = tiny_png(64, RED);
        v.extend(std::iter::repeat_n(0u8, 5000));
        v
    };
    let limits = Limits { max_favicon_bytes: 4000, ..Limits::default() };
    let icons = chrome_icons(
        &[
            ChromiumIcon {
                pages: &["https://www.rust-lang.org/learn"],
                bitmaps: &[
                    // Best size, but broken; then too big; then a small good one.
                    (32, 32, b"\x89PNG\r\n\x1a\ngarbage".to_vec()),
                    (48, 48, big.clone()),
                    (16, 16, tiny_png(16, GREEN)),
                ],
            },
            // Nothing usable at all: this host gets no icon, and the import still works.
            ChromiumIcon {
                pages: &["https://example.com/a"],
                bitmaps: &[(32, 32, vec![0, 1, 2, 3]), (16, 16, Vec::new()), (64, 64, big)],
            },
            // A zero-size image (0x0 in the database) is no candidate unless it decodes.
            ChromiumIcon { pages: &["https://docs.rs/rusqlite/"], bitmaps: &[(0, 0, tiny_png(8, YELLOW))] },
        ],
        &limits,
    );
    assert_eq!(icons.len(), 2, "{icons:?}");
    assert_eq!(rgba_of(icon_of(&icons, "rust-lang.org")), GREEN);
    assert_eq!(rgba_of(icon_of(&icons, "docs.rs")), YELLOW);
    assert!(icons.iter().all(|i| i.host != "example.com"));
}

#[test]
fn chromium_shared_and_duplicate_icons() {
    let icons = chrome_icons(
        &[
            // One icon on pages of two hosts, and on two pages of one host: one icon per host.
            ChromiumIcon {
                pages: &[
                    "https://www.rust-lang.org/learn",
                    "https://www.rust-lang.org/other",
                    "https://example.com/a",
                ],
                bitmaps: &[(32, 32, tiny_png(32, RED)), (32, 32, tiny_png(32, RED))],
            },
        ],
        &Limits::default(),
    );
    assert_eq!(icons.len(), 2);
    assert_eq!(icons.iter().filter(|i| i.host == "rust-lang.org").count(), 1);
    assert_eq!(rgba_of(icon_of(&icons, "example.com")), RED);
}

#[test]
fn the_icon_limit_and_the_switch() {
    let favicons = [
        ChromiumIcon { pages: &["https://www.rust-lang.org/learn"], bitmaps: &[(32, 32, tiny_png(32, RED))] },
        ChromiumIcon { pages: &["https://example.com/a"], bitmaps: &[(32, 32, tiny_png(32, GREEN))] },
        ChromiumIcon { pages: &["https://docs.rs/"], bitmaps: &[(32, 32, tiny_png(32, BLUE))] },
    ];
    // Bookmarks come first, so the history host is the one left out.
    let limited = chrome_icons(&favicons, &Limits { max_favicons: 2, ..Limits::default() });
    let mut hosts: Vec<&str> = limited.iter().map(|i| i.host.as_str()).collect();
    hosts.sort_unstable();
    assert_eq!(hosts, ["example.com", "rust-lang.org"]);
    assert!(chrome_icons(&favicons, &Limits { favicons: false, ..Limits::default() }).is_empty());
}

#[test]
fn a_damaged_icon_database_does_not_fail_the_import() {
    let (dir, home) = chrome_home(&[]);
    std::fs::write(dir.path().join(".config/google-chrome/Default/Favicons"), b"this is not sqlite").unwrap();
    let sources = detect(&home);
    let data = read(&sources[0], Wanted::ALL, &Limits::default()).unwrap();
    assert_eq!(data.bookmarks.len(), 2);
    assert!(data.icons.is_empty());
    assert!(data.warnings.iter().any(|w| w.starts_with("site icons")));
    // No Favicons file at all: no warning either.
    std::fs::remove_file(dir.path().join(".config/google-chrome/Default/Favicons")).unwrap();
    let data = read(&sources[0], Wanted::ALL, &Limits::default()).unwrap();
    assert!(data.icons.is_empty() && data.warnings.is_empty());
}

fn firefox_profile(dir: &Path) -> std::path::PathBuf {
    let root = dir.join(".mozilla/firefox");
    let profile = root.join("abcd.default");
    fixtures::profiles_ini(&root, &[("default", "abcd.default", true)]);
    fixtures::firefox_places(
        &profile,
        &[
            ("https://developer.mozilla.org/en-US/docs", "MDN", 5, NOW - 10),
            ("https://lwn.net/Articles/1/", "LWN", 2, NOW - 20),
        ],
        &[fixtures::Mark {
            id: 11,
            kind: 1,
            parent: 3,
            title: "Mozilla",
            guid: "m1__________",
            url: Some("https://www.mozilla.org/en-US/"),
            unix: NOW,
        }],
    );
    profile
}

#[test]
fn firefox_icons_by_page_by_site_and_with_ico() {
    let dir = tempfile::tempdir().unwrap();
    let profile = firefox_profile(dir.path());
    firefox_favicons(
        &profile,
        &[
            // The bookmarked page itself: a PNG in two sizes (two rows).
            (
                "https://www.mozilla.org/media/a16.png",
                16,
                tiny_png(16, RED),
                &["https://www.mozilla.org/en-US/"],
            ),
            (
                "https://www.mozilla.org/media/a32.png",
                32,
                tiny_png(32, GREEN),
                &["https://www.mozilla.org/en-US/"],
            ),
            // A page of the history host, an ICO.
            (
                "https://developer.mozilla.org/favicon.ico",
                16,
                tiny_ico(16, BLUE),
                &["https://developer.mozilla.org/"],
            ),
            // SVG (width 65535 in Firefox) is skipped.
            ("https://lwn.net/icon.svg", 65535, b"<svg/>".to_vec(), &["https://lwn.net/Articles/1/"]),
        ],
    );
    let home = Home::new(dir.path());
    let sources = detect(&home);
    let data = read(&sources[0], Wanted::ALL, &Limits::default()).unwrap();
    assert_eq!(data.icons.len(), 2, "{:?}", data.icons.iter().map(|i| &i.host).collect::<Vec<_>>());
    assert_eq!(rgba_of(icon_of(&data.icons, "mozilla.org")), GREEN);
    assert_eq!(rgba_of(icon_of(&data.icons, "developer.mozilla.org")), BLUE);
}

#[test]
fn firefox_site_icons_without_a_page_row() {
    let dir = tempfile::tempdir().unwrap();
    let profile = firefox_profile(dir.path());
    // The icon is only known by its address (a root icon): the host of that address counts.
    firefox_favicons(&profile, &[("https://lwn.net/favicon.ico", 16, tiny_png(16, YELLOW), &[])]);
    let sources = detect(&Home::new(dir.path()));
    let data = read(&sources[0], Wanted::ALL, &Limits::default()).unwrap();
    assert_eq!(data.icons.len(), 1);
    assert_eq!(rgba_of(icon_of(&data.icons, "lwn.net")), YELLOW);
}

#[test]
fn old_firefox_keeps_icons_in_places_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".mozilla/firefox");
    let profile = root.join("old.default");
    fixtures::profiles_ini(&root, &[("old", "old.default", true)]);
    // `places.sqlite` of the fixture has the icon tables, but no `favicon_id` column; the
    // legacy builder makes a complete old-style database instead.
    firefox_legacy_favicons(
        &profile,
        &[
            ("https://www.mozilla.org/en-US/", tiny_png(16, RED)),
            ("https://lwn.net/Articles/1/", b"junk".to_vec()),
        ],
    );
    let sources = detect(&Home::new(dir.path()));
    assert_eq!(sources.len(), 1);
    let history = Wanted { bookmarks: false, history: true };
    let icons = read(&sources[0], history, &Limits::default()).unwrap().icons;
    assert_eq!(icons.len(), 1);
    assert_eq!(rgba_of(icon_of(&icons, "mozilla.org")), RED);
}

#[test]
fn html_export_with_icon_data_uris() {
    use base64::Engine as _;
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let red = b64(tiny_png(32, RED));
    let ico = b64(tiny_ico(16, GREEN));
    let html = format!(
        r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
<DT><A HREF="https://www.rust-lang.org/" ICON="data:image/png;base64,{red}">Rust</A>
<DT><A HREF="https://www.rust-lang.org/learn" ICON="data:image/png;base64,{red}">Rust Learn</A>
<DT><A HREF="https://example.com/" ICON="data:image/x-icon;base64,{ico}">Example</A>
<DT><A HREF="https://broken.test/" ICON="data:image/png;base64,AAAA">Broken</A>
<DT><A HREF="https://svg.test/" ICON="data:image/svg+xml;base64,PHN2Zy8+">Svg</A>
<DT><A HREF="https://remote.test/" ICON="https://remote.test/favicon.ico">Remote</A>
<DT><A HREF="https://plain.test/">Plain</A>
</DL><p>"#
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bookmarks.html");
    std::fs::write(&path, &html).unwrap();
    let data = read_file(&path, &Limits::default()).unwrap();
    assert_eq!(data.bookmarks.len(), 7);
    assert_eq!(data.icons.len(), 2, "{:?}", data.icons.iter().map(|i| &i.host).collect::<Vec<_>>());
    assert_eq!(rgba_of(icon_of(&data.icons, "rust-lang.org")), RED);
    assert_eq!(rgba_of(icon_of(&data.icons, "example.com")), GREEN);
    // Off: bookmarks only. Over the byte limit: no icon.
    assert!(read_file(&path, &Limits { favicons: false, ..Limits::default() }).unwrap().icons.is_empty());
    assert!(
        read_file(&path, &Limits { max_favicon_bytes: 20, ..Limits::default() }).unwrap().icons.is_empty()
    );
}

#[test]
fn applying_an_import_makes_icons_readable_from_the_cache_folder() {
    let (_dir, home) = chrome_home(&[
        ChromiumIcon {
            pages: &["https://www.rust-lang.org/learn"],
            bitmaps: &[(32, 32, tiny_png(32, GREEN))],
        },
        ChromiumIcon { pages: &["https://example.com/a"], bitmaps: &[(32, 32, tiny_png(32, BLUE))] },
    ]);
    let sources = detect(&home);
    let data = read(&sources[0], Wanted::ALL, &Limits::default()).unwrap();
    let store = Store::in_memory().unwrap();
    let report = apply(&store, &data, NOW).unwrap();
    assert_eq!(report.bookmarks_added, 2);

    let cache = tempfile::tempdir().unwrap();
    // A site the user already browsed keeps the icon from browsing.
    std::fs::write(cache.path().join("example.com.png"), b"browsed").unwrap();
    let written = write_icons(cache.path(), &data.icons);
    assert_eq!((written.added, written.existing), (1, 1));
    let png = std::fs::read(cache.path().join("rust-lang.org.png")).unwrap();
    let img = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(img.get_pixel(3, 3).0, GREEN);
    assert_eq!(std::fs::read(cache.path().join("example.com.png")).unwrap(), b"browsed");
    // Importing twice is harmless.
    let again = write_icons(cache.path(), &data.icons);
    assert_eq!((again.added, again.existing), (0, 2));
}
