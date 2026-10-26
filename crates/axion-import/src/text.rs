//! Small helpers shared by the readers: epochs, URL and title cleaning, reading text files.

use std::path::Path;

use crate::error::{ImportError, Result};

/// Seconds between 1601-01-01 (Chromium's epoch) and 1970-01-01.
const WEBKIT_TO_UNIX: i64 = 11_644_473_600;
/// 2100-01-01: later times are garbage.
const MAX_UNIX: i64 = 4_102_444_800;
const MAX_URL: usize = 4096;
const MAX_TITLE_CHARS: usize = 300;

/// Chromium time (microseconds since 1601-01-01) as unix seconds; 0 when unset or absurd.
pub fn webkit_to_unix(micros: i64) -> i64 {
    sane(micros.checked_div(1_000_000).unwrap_or(0).saturating_sub(WEBKIT_TO_UNIX), micros)
}

/// Firefox time (microseconds since 1970-01-01) as unix seconds; 0 when unset or absurd.
pub fn firefox_to_unix(micros: i64) -> i64 {
    sane(micros.checked_div(1_000_000).unwrap_or(0), micros)
}

/// Unix seconds from a Netscape `ADD_DATE`, which exporters write in seconds, milliseconds or
/// microseconds.
pub fn guess_to_unix(value: i64) -> i64 {
    let seconds = match value {
        v if v > 100_000_000_000_000 => v / 1_000_000,
        v if v > 100_000_000_000 => v / 1000,
        v => v,
    };
    sane(seconds, value)
}

fn sane(seconds: i64, raw: i64) -> i64 {
    if raw <= 0 || !(1..MAX_UNIX).contains(&seconds) { 0 } else { seconds }
}

/// The URL if it is one worth importing (web pages and files), trimmed; `None` for internal
/// pages (`about:`, `chrome://`, `place:`), scripts (`javascript:`), data and the like.
pub fn clean_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_URL || url.chars().any(|c| c.is_control()) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    let ok = match scheme.as_str() {
        "http" | "https" | "ftp" => rest.starts_with("//") && rest.len() > 2,
        "file" => rest.starts_with("//"),
        _ => false,
    };
    ok.then(|| url.to_owned())
}

/// A title without control characters (newlines and tabs become spaces), trimmed and cut to a
/// sensible length on a character boundary.
pub fn clean_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len().min(MAX_TITLE_CHARS * 2));
    let mut chars = 0;
    for c in title.chars() {
        let c = match c {
            '\n' | '\r' | '\t' => ' ',
            // Invisible controls, including NUL and the bidi/zero-width overrides that could
            // fake the look of a title.
            c if c.is_control()
                || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}') =>
            {
                continue;
            }
            c => c,
        };
        out.push(c);
        chars += 1;
        if chars >= MAX_TITLE_CHARS * 2 {
            break;
        }
    }
    let trimmed = out.trim();
    trimmed.chars().take(MAX_TITLE_CHARS).collect()
}

/// Reads a text file of at most `max` bytes (lossy UTF-8: exports come in odd encodings).
pub fn read_text_file(path: &Path, max: u64) -> Result<String> {
    let io = |source| ImportError::Io { path: path.to_owned(), source };
    let size = std::fs::metadata(path).map_err(io)?.len();
    if size > max {
        return Err(ImportError::TooLarge { path: path.to_owned(), size, limit: max });
    }
    let bytes = std::fs::read(path).map_err(io)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_epoch() {
        // 2024-01-01 00:00:00 UTC is 1704067200 s since 1970.
        assert_eq!(webkit_to_unix((1_704_067_200 + WEBKIT_TO_UNIX) * 1_000_000), 1_704_067_200);
        assert_eq!(webkit_to_unix(0), 0);
        assert_eq!(webkit_to_unix(-5), 0);
        // Before 1970 or far in the future is garbage.
        assert_eq!(webkit_to_unix(1_000_000), 0);
        assert_eq!(webkit_to_unix(i64::MAX), 0);
    }

    #[test]
    fn firefox_epoch() {
        assert_eq!(firefox_to_unix(1_704_067_200_123_456), 1_704_067_200);
        assert_eq!(firefox_to_unix(0), 0);
        assert_eq!(firefox_to_unix(i64::MAX), 0);
        assert_eq!(firefox_to_unix(i64::MIN), 0);
    }

    #[test]
    fn netscape_dates_in_any_unit() {
        assert_eq!(guess_to_unix(1_704_067_200), 1_704_067_200);
        assert_eq!(guess_to_unix(1_704_067_200_000), 1_704_067_200);
        assert_eq!(guess_to_unix(1_704_067_200_000_000), 1_704_067_200);
        assert_eq!(guess_to_unix(0), 0);
    }

    #[test]
    fn urls() {
        assert_eq!(clean_url(" https://example.com/a b ").as_deref(), Some("https://example.com/a b"));
        assert!(clean_url("HTTP://EXAMPLE.com").is_some());
        assert!(clean_url("file:///home/me/x.html").is_some());
        for bad in [
            "",
            "javascript:alert(1)",
            "data:text/html,x",
            "chrome://settings",
            "about:blank",
            "place:sort=8",
            "https://",
            "https://a\nb",
            "no scheme",
        ] {
            assert_eq!(clean_url(bad), None, "{bad}");
        }
        assert_eq!(clean_url(&format!("https://x.test/{}", "a".repeat(5000))), None);
    }

    #[test]
    fn titles() {
        assert_eq!(clean_title("  Zürich – Größe 日本語 🦀 "), "Zürich – Größe 日本語 🦀");
        assert_eq!(clean_title("a\nb\tc\0d"), "a b cd");
        assert_eq!(clean_title("evil\u{202e}gnp"), "evilgnp");
        assert_eq!(clean_title("אבג"), "אבג");
        let long = "ä".repeat(1000);
        assert_eq!(clean_title(&long).chars().count(), MAX_TITLE_CHARS);
        assert_eq!(clean_title(""), "");
    }
}
