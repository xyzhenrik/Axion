//! Synthetic icon databases for tests and captures: Chromium's `Favicons`, Firefox's
//! `favicons.sqlite`, with tiny PNGs made in code. Never real browser data.
#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;

use rusqlite::{Connection, params};

fn create(dir: &Path, name: &str) -> Connection {
    let _ = fs::create_dir_all(dir);
    Connection::open(dir.join(name)).expect("fixture db")
}

/// A square PNG of one colour.
pub fn tiny_png(side: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(side, side, image::Rgba(rgba));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("fixture png");
    out
}

/// A square ICO holding one image of one colour.
pub fn tiny_ico(side: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(side, side, image::Rgba(rgba));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Ico)
        .expect("fixture ico");
    out
}

/// One icon of a Chromium `Favicons` fixture: the pages it is used on and its bitmaps as
/// `(width, height, bytes)`.
pub struct ChromiumIcon<'a> {
    pub pages: &'a [&'a str],
    pub bitmaps: &'a [(i64, i64, Vec<u8>)],
}

/// A Chromium `Favicons` database (`favicons`, `favicon_bitmaps`, `icon_mapping`).
pub fn chromium_favicons(dir: &Path, icons: &[ChromiumIcon<'_>]) {
    let conn = create(dir, "Favicons");
    conn.execute_batch(
        "CREATE TABLE favicons (id INTEGER PRIMARY KEY, url LONGVARCHAR NOT NULL, icon_type INTEGER DEFAULT 1);
         CREATE TABLE favicon_bitmaps (id INTEGER PRIMARY KEY, icon_id INTEGER NOT NULL,
            last_updated INTEGER DEFAULT 0, image_data BLOB, width INTEGER DEFAULT 0,
            height INTEGER DEFAULT 0, last_requested INTEGER DEFAULT 0);
         CREATE TABLE icon_mapping (id INTEGER PRIMARY KEY, page_url LONGVARCHAR NOT NULL,
            icon_id INTEGER, page_url_type INTEGER DEFAULT 0);",
    )
    .expect("fixture schema");
    for (n, icon) in icons.iter().enumerate() {
        let id = n as i64 + 1;
        conn.execute(
            "INSERT INTO favicons (id, url) VALUES (?1, ?2)",
            params![id, format!("https://icons.test/{id}")],
        )
        .expect("fixture icon");
        for (w, h, data) in icon.bitmaps {
            conn.execute(
                "INSERT INTO favicon_bitmaps (icon_id, image_data, width, height) VALUES (?1, ?2, ?3, ?4)",
                params![id, data, w, h],
            )
            .expect("fixture bitmap");
        }
        for page in icon.pages {
            conn.execute("INSERT INTO icon_mapping (page_url, icon_id) VALUES (?1, ?2)", params![page, id])
                .expect("fixture mapping");
        }
    }
}

/// One icon of a Firefox `favicons.sqlite` fixture: `(icon url, width, data, pages)`.
pub type FirefoxIcon<'a> = (&'a str, i64, Vec<u8>, &'a [&'a str]);

/// Firefox's `favicons.sqlite` (`moz_icons`, `moz_pages_w_icons`, `moz_icons_to_pages`).
pub fn firefox_favicons(dir: &Path, icons: &[FirefoxIcon<'_>]) {
    let conn = create(dir, "favicons.sqlite");
    conn.execute_batch(
        "CREATE TABLE moz_icons (id INTEGER PRIMARY KEY, icon_url TEXT NOT NULL,
            fixed_icon_url_hash INTEGER NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0,
            root INTEGER NOT NULL DEFAULT 0, color INTEGER, expire_ms INTEGER NOT NULL DEFAULT 0,
            data BLOB, flags INTEGER);
         CREATE TABLE moz_pages_w_icons (id INTEGER PRIMARY KEY, page_url TEXT NOT NULL,
            page_url_hash INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE moz_icons_to_pages (page_id INTEGER NOT NULL, icon_id INTEGER NOT NULL,
            expire_ms INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (page_id, icon_id));",
    )
    .expect("fixture schema");
    for (n, (url, width, data, pages)) in icons.iter().enumerate() {
        let icon_id = n as i64 + 1;
        conn.execute(
            "INSERT INTO moz_icons (id, icon_url, width, data) VALUES (?1, ?2, ?3, ?4)",
            params![icon_id, url, width, data],
        )
        .expect("fixture icon");
        for page in *pages {
            conn.execute("INSERT INTO moz_pages_w_icons (page_url) VALUES (?1)", params![page])
                .expect("fixture page");
            let page_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO moz_icons_to_pages (page_id, icon_id) VALUES (?1, ?2)",
                params![page_id, icon_id],
            )
            .expect("fixture link");
        }
    }
}

/// Old Firefox: `moz_favicons` and `moz_places.favicon_id` in `places.sqlite` (with the
/// bookmarks fixture's `moz_places` columns plus `favicon_id`). `icons`: `(page url, data)`.
pub fn firefox_legacy_favicons(dir: &Path, icons: &[(&str, Vec<u8>)]) {
    let conn = create(dir, "places.sqlite");
    conn.execute_batch(
        "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR,
            visit_count INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0 NOT NULL, last_visit_date INTEGER,
            favicon_id INTEGER);
         CREATE TABLE moz_favicons (id INTEGER PRIMARY KEY, url LONGVARCHAR, data BLOB, mime_type VARCHAR(32));",
    )
    .expect("fixture schema");
    for (n, (page, data)) in icons.iter().enumerate() {
        let id = n as i64 + 1;
        conn.execute(
            "INSERT INTO moz_favicons (id, url, data, mime_type) VALUES (?1, ?2, ?3, 'image/png')",
            params![id, format!("{page}favicon.ico"), data],
        )
        .expect("fixture icon");
        conn.execute(
            "INSERT INTO moz_places (url, title, visit_count, last_visit_date, favicon_id) VALUES (?1, 't', 1, 1700000000000000, ?2)",
            params![page, id],
        )
        .expect("fixture place");
    }
}
