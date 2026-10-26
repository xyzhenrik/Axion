//! When to check, and when a found release may bother the user. Pure functions over an
//! injected clock, so the persistence rules are testable.

use axion_config::Updates;
use semver::Version;

use crate::Release;
use crate::version::parse_version;

/// Seconds since the Unix epoch.
pub trait Clock {
    fn now(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }
}

/// Whether Axion should check on its own now: updates on, a source set, automatic checks on,
/// and the interval since the last check has passed (a `last_checked` in the future, from a
/// wrong clock, counts as due).
pub fn auto_check_due(updates: &Updates, clock: &dyn Clock) -> bool {
    if !updates.enabled || !updates.auto_check_on_start || !updates.has_source() {
        return false;
    }
    let now = clock.now();
    let interval = u64::from(updates.check_interval_hours) * 3600;
    updates.last_checked == 0 || updates.last_checked > now || now - updates.last_checked >= interval
}

/// Remembers that a check just ran.
pub fn record_check(updates: &mut Updates, clock: &dyn Clock) {
    updates.last_checked = clock.now();
}

/// "Skip This Version".
pub fn skip(updates: &mut Updates, version: &Version) {
    updates.skipped_version = version.to_string();
}

/// "Remind Me Later": silences automatic notices for `remind_later_hours`.
pub fn remind_later(updates: &mut Updates, clock: &dyn Clock) {
    updates.remind_until = clock.now() + u64::from(updates.remind_later_hours) * 3600;
}

/// Whether the release the user skipped is this one.
pub fn is_skipped(updates: &Updates, release: &Release) -> bool {
    parse_version(&updates.skipped_version).is_ok_and(|skipped| skipped == release.version)
}

/// Whether an automatically found release may raise a notice (a toast and a badge): not
/// skipped, and not within a "remind me later" period. A manual check always shows its result.
pub fn may_notify(updates: &Updates, release: &Release, clock: &dyn Clock) -> bool {
    !is_skipped(updates, release) && clock.now() >= updates.remind_until
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(u64);
    impl Clock for Fixed {
        fn now(&self) -> u64 {
            self.0
        }
    }

    fn configured() -> Updates {
        Updates { repository: "o/n".into(), ..Updates::default() }
    }

    fn release(version: &str) -> Release {
        Release {
            version: parse_version(version).expect("valid"),
            tag: version.into(),
            name: String::new(),
            notes: String::new(),
            page_url: String::new(),
            prerelease: false,
            assets: Vec::new(),
        }
    }

    const DAY: u64 = 24 * 3600;

    #[test]
    fn nothing_happens_without_a_source() {
        let updates = Updates { repository: String::new(), ..Updates::default() };
        assert!(!updates.has_source());
        assert!(!auto_check_due(&updates, &Fixed(10 * DAY)));
    }

    #[test]
    fn interval_logic() {
        let mut updates = configured();
        // Never checked: due.
        assert!(auto_check_due(&updates, &Fixed(1_000_000)));
        updates.last_checked = 1_000_000;
        assert!(!auto_check_due(&updates, &Fixed(1_000_000 + DAY - 1)));
        assert!(auto_check_due(&updates, &Fixed(1_000_000 + DAY)));
        // A last check "in the future" (clock moved back) is due again.
        assert!(auto_check_due(&updates, &Fixed(500_000)));
        updates.check_interval_hours = 1;
        assert!(auto_check_due(&updates, &Fixed(1_000_000 + 3600)));
    }

    #[test]
    fn switches_turn_checks_off() {
        let mut updates = configured();
        updates.enabled = false;
        assert!(!auto_check_due(&updates, &Fixed(DAY)));
        updates.enabled = true;
        updates.auto_check_on_start = false;
        assert!(!auto_check_due(&updates, &Fixed(DAY)));
        // A manifest alone is a source.
        let manifest = Updates { manifest_url: "https://x.org/l.json".into(), ..Updates::default() };
        assert!(auto_check_due(&manifest, &Fixed(DAY)));
    }

    #[test]
    fn skip_and_remind_persist_through_the_config_file() {
        let mut updates = configured();
        let r = release("0.2.0");
        assert!(may_notify(&updates, &r, &Fixed(100)));

        skip(&mut updates, &r.version);
        assert!(!may_notify(&updates, &r, &Fixed(100)));
        // Only that version is skipped.
        assert!(may_notify(&updates, &release("0.2.1"), &Fixed(100)));

        remind_later(&mut updates, &Fixed(1000));
        assert_eq!(updates.remind_until, 1000 + DAY);
        assert!(!may_notify(&updates, &release("0.2.1"), &Fixed(1000 + DAY - 1)));
        assert!(may_notify(&updates, &release("0.2.1"), &Fixed(1000 + DAY)));
        record_check(&mut updates, &Fixed(777));

        // Written to config.toml and read back.
        let config = axion_config::Config { updates: updates.clone(), ..Default::default() };
        let text = toml::to_string(&config).expect("serializes");
        let back: axion_config::Config = toml::from_str(&text).expect("parses");
        assert_eq!(back.updates, updates);
        assert_eq!(back.updates.skipped_version, "0.2.0");
        assert_eq!(back.updates.last_checked, 777);
    }

    #[test]
    fn the_default_config_follows_the_default_repository_and_documented_keys_parse() {
        let defaults = Updates::default();
        assert!(defaults.enabled);
        assert_eq!(defaults.repository, axion_config::DEFAULT_REPOSITORY);
        assert_eq!(defaults.has_source(), !axion_config::DEFAULT_REPOSITORY.is_empty());
        let parsed: axion_config::Config = toml::from_str(
            "[updates]\nrepository = \"o/n\"\nchannel = \"prerelease\"\ncheck_interval_hours = 6\n",
        )
        .expect("parses");
        assert_eq!(parsed.updates.channel, axion_config::UpdateChannel::Prerelease);
        assert_eq!(parsed.updates.check_interval_hours, 6);
    }
}
