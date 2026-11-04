//! Icons for the start page's top-site tiles that no cache has: which hosts to fetch, and which
//! ones to leave alone for now.
//!
//! The tiles look an icon up in the site icon cache (`Favicons::get_site`). A host without one
//! is fetched in the background (`engine_icons::fetch`), once per session, and, when nothing
//! was found, remembered as a miss in the cache folder (`<host>.miss`, its modification time is
//! the time of the try) so it isn't asked again before `home.site_icon_retry_hours` passed,
//! across restarts too.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The file noting a failed fetch for `host` in the icon cache folder.
pub fn miss_file(dir: &Path, host: &str) -> PathBuf {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    dir.join(format!("{safe}.miss"))
}

/// Notes that fetching `host`'s icon failed just now.
pub fn note_miss(dir: &Path, host: &str) {
    if let Err(e) = std::fs::write(miss_file(dir, host), b"") {
        tracing::debug!("noting the missing icon of {host} failed: {e}");
    }
}

/// When `host`'s miss was noted (`None`: no miss on file).
pub fn noted_at(dir: &Path, host: &str) -> Option<SystemTime> {
    std::fs::metadata(miss_file(dir, host)).ok()?.modified().ok()
}

/// Whether a miss from `noted` still holds at `now`.
pub fn miss_is_fresh(noted: SystemTime, now: SystemTime, retry: Duration) -> bool {
    // A clock that went back keeps the miss: it is not retried in a loop.
    now.duration_since(noted).map_or(true, |age| age < retry)
}

/// The hosts to fetch icons for now: the tiles' hosts without an icon, not asked this session,
/// not a fresh miss, at most `limit`, in tile order. `noted` tells when a host's miss was noted.
pub fn hosts_to_fetch(
    missing: &[String],
    asked: &HashSet<String>,
    noted: impl Fn(&str) -> Option<SystemTime>,
    now: SystemTime,
    retry: Duration,
    limit: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for host in missing {
        if out.len() >= limit {
            break;
        }
        if host.is_empty() || asked.contains(host) || out.contains(host) {
            continue;
        }
        if noted(host).is_some_and(|at| miss_is_fresh(at, now, retry)) {
            continue;
        }
        out.push(host.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| (*h).to_owned()).collect()
    }

    #[test]
    fn misses_expire_after_the_retry_interval() {
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(miss_is_fresh(then, then + HOUR, 2 * HOUR));
        assert!(!miss_is_fresh(then, then + 3 * HOUR, 2 * HOUR));
        assert!(miss_is_fresh(then, then - HOUR, 2 * HOUR), "clock went back");
    }

    #[test]
    fn fetch_plan_skips_asked_fresh_and_over_limit() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let noted = |h: &str| match h {
            "fresh.example" => Some(now - HOUR),
            "stale.example" => Some(now - 100 * HOUR),
            _ => None,
        };
        let asked: HashSet<String> = hosts(&["asked.example"]).into_iter().collect();
        let missing = hosts(&[
            "fresh.example",
            "asked.example",
            "stale.example",
            "new.example",
            "new.example",
            "",
            "later.example",
        ]);
        let plan = |limit| hosts_to_fetch(&missing, &asked, noted, now, 72 * HOUR, limit);
        assert_eq!(plan(10), hosts(&["stale.example", "new.example", "later.example"]));
        assert_eq!(plan(2), hosts(&["stale.example", "new.example"]));
        assert!(plan(0).is_empty());
    }

    #[test]
    fn misses_are_kept_in_files_and_expire() {
        let dir = std::env::temp_dir().join(format!("axion-tile-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(noted_at(&dir, "a.example"), None);
        note_miss(&dir, "a.example");
        let at = noted_at(&dir, "a.example").expect("miss noted");
        assert!(miss_is_fresh(at, SystemTime::now(), 72 * HOUR));
        assert!(!miss_is_fresh(at, SystemTime::now() + 100 * HOUR, 72 * HOUR));
        std::fs::remove_dir_all(&dir).ok();
    }
}
