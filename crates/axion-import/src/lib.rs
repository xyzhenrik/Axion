//! Importing bookmarks and history from other browsers.
//!
//! Reads, never writes, what other browsers keep on disk:
//! - the Chromium family (Chrome, Chromium, Brave, Edge, Vivaldi, Opera): `Bookmarks` (JSON)
//!   and `History` (SQLite) in each profile directory;
//! - the Firefox family (Firefox, LibreWolf, Zen, Waterfox, Floorp): `places.sqlite` of each
//!   profile listed in `profiles.ini`;
//! - exported bookmark files: the Netscape HTML every browser exports, a Chromium `Bookmarks`
//!   file, a Firefox JSON backup.
//!
//! Native, Flatpak and Snap installations are looked for under an injected [`Home`], so tests
//! (and captures) never touch the real one. A running browser keeps its databases locked, so
//! SQLite files are copied (with their `-wal`) into a temporary directory first and read from
//! the copy, which is deleted afterwards. Site icons come from the same profiles (Chromium's
//! `Favicons`, Firefox's `favicons.sqlite`, the `ICON` attributes of exported HTML) and go
//! into Axion's icon cache folder, see [`write_icons`]. Passwords, cookies and extensions are not imported:
//! Chromium encrypts passwords with the desktop keyring (use the CSV import for those).
//!
//! The flow: [`detect`] / [`scan`] list [`Source`]s; [`read`] (or [`read_file`]) turns one into
//! an [`Imported`] (pure data, capped by [`Limits`]); [`apply`] writes it into a store.

mod apply;
mod chromium;
mod detect;
mod error;
mod firefox;
mod html;
mod icons;
mod model;
mod sqlite;
mod text;

#[doc(hidden)]
pub mod fixtures;
#[doc(hidden)]
pub mod icon_fixtures;

pub use apply::{Report, apply};
pub use detect::{Family, Home, Source, detect, scan};
pub use error::{ImportError, Result};
pub use icons::{IconReport, ImportIcon, host_key, icon_file_name, write as write_icons};
pub use model::{ImportBookmark, ImportHistory, Imported, Limits, Wanted};
pub use text::{firefox_to_unix, webkit_to_unix};

use std::path::Path;

/// Reads what `wanted` asks for from `source`. A part that cannot be read (a locked or damaged
/// database) becomes a warning when the other part works; when everything asked for fails, the
/// first error is returned.
pub fn read(source: &Source, wanted: Wanted, limits: &Limits) -> Result<Imported> {
    let mut out = Imported::default();
    let mut first_error = None;
    let mut tried = 0;
    let mut fail = |what: &str, e: ImportError, out: &mut Imported| {
        tracing::warn!(browser = %source.browser, profile = %source.profile, "reading {what} failed: {e}");
        out.warnings.push(format!("{what}: {e}"));
        first_error.get_or_insert(e);
    };
    match source.family {
        Family::Chromium => {
            if wanted.bookmarks && source.has_bookmarks {
                tried += 1;
                match chromium::read_bookmarks(&source.dir, limits) {
                    Ok(b) => out.bookmarks = b,
                    Err(e) => fail("bookmarks", e, &mut out),
                }
            }
            if wanted.history && source.has_history {
                tried += 1;
                match chromium::read_history(&source.dir, limits) {
                    Ok(h) => out.history = h,
                    Err(e) => fail("history", e, &mut out),
                }
            }
        }
        Family::Firefox => {
            if (wanted.bookmarks || wanted.history) && (source.has_bookmarks || source.has_history) {
                tried += 1;
                match firefox::read(&source.dir, wanted, limits) {
                    Ok((b, h)) => {
                        out.bookmarks = b;
                        out.history = h;
                    }
                    Err(e) => fail("places.sqlite", e, &mut out),
                }
            }
        }
    }
    if let Some(e) = first_error
        && out.warnings.len() == tried
    {
        return Err(e);
    }
    // Icons are a bonus: a profile whose icon database cannot be read still imports.
    if limits.favicons && !(out.bookmarks.is_empty() && out.history.is_empty()) {
        let wanted = icons::WantedIcons::from_imported(&out.bookmarks, &out.history);
        let icons = match source.family {
            Family::Chromium => chromium::read_icons(&source.dir, &wanted, limits),
            Family::Firefox => firefox::read_icons(&source.dir, &wanted, limits),
        };
        match icons {
            Ok(icons) => out.icons = icons,
            Err(e) => {
                tracing::warn!(browser = %source.browser, profile = %source.profile, "reading site icons failed: {e}");
                out.warnings.push(format!("site icons: {e}"));
            }
        }
    }
    Ok(out)
}

/// Reads bookmarks from an exported file: Netscape HTML, a Chromium `Bookmarks` JSON file or a
/// Firefox JSON backup (told apart by content, not by name).
pub fn read_file(path: &Path, limits: &Limits) -> Result<Imported> {
    let text = text::read_text_file(path, limits.max_text_bytes)?;
    let (bookmarks, icons) = parse_bookmarks_with_icons(&text, limits)?;
    Ok(Imported { bookmarks, icons, ..Imported::default() })
}

/// Parses the text of a bookmarks file (see [`read_file`]).
pub fn parse_bookmarks(text: &str, limits: &Limits) -> Result<Vec<ImportBookmark>> {
    let no_icons = Limits { favicons: false, ..limits.clone() };
    parse_bookmarks_with_icons(text, &no_icons).map(|(b, _)| b)
}

/// Like [`parse_bookmarks`], plus the site icons the file carries (`ICON="data:image/…"` in
/// Netscape HTML) when `limits.favicons` is on.
pub fn parse_bookmarks_with_icons(
    text: &str,
    limits: &Limits,
) -> Result<(Vec<ImportBookmark>, Vec<ImportIcon>)> {
    let mut icons = Vec::new();
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    let mut bookmarks = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let value: serde_json::Value = serde_json::from_str(trimmed)?;
        if value.get("roots").is_some() {
            chromium::parse_bookmarks(&value)?
        } else if value.get("children").is_some() || value.get("uri").is_some() {
            firefox::parse_backup(&value)?
        } else {
            return Err(ImportError::Format("not a bookmarks file".into()));
        }
    } else if html::looks_like_bookmarks(trimmed) {
        if limits.favicons {
            let (bookmarks, found) = html::parse_with_icons(trimmed, limits);
            icons = found;
            bookmarks
        } else {
            html::parse_bookmarks(trimmed)
        }
    } else {
        return Err(ImportError::Format("not a bookmarks file".into()));
    };
    bookmarks.truncate(limits.max_bookmarks);
    Ok((bookmarks, icons))
}
