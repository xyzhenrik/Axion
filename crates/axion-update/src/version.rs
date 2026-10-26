//! Version numbers: SemVer, tolerant of a leading `v` and of missing minor / patch parts.

use semver::Version;

use crate::UpdateError;

/// Parses `v1.2.3`, `1.2.3-beta.1`, `1.2` (= 1.2.0) or `1` (= 1.0.0`).
pub fn parse_version(text: &str) -> Result<Version, UpdateError> {
    let trimmed = text.trim().trim_start_matches(['v', 'V']);
    if let Ok(version) = Version::parse(trimmed) {
        return Ok(version);
    }
    // "1.2" / "1": pad up to three numeric parts before any pre-release or build suffix.
    let split = trimmed.find(['-', '+']).unwrap_or(trimmed.len());
    let (numbers, suffix) = trimmed.split_at(split);
    let parts = numbers.split('.').count();
    if (1..3).contains(&parts)
        && numbers.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        let padded = format!("{numbers}{}{suffix}", ".0".repeat(3 - parts));
        if let Ok(version) = Version::parse(&padded) {
            return Ok(version);
        }
    }
    Err(UpdateError::Version(text.to_owned()))
}

/// Whether `candidate` is a newer release than `current` (a pre-release is older than its
/// final release; build metadata is ignored).
pub fn is_newer(candidate: &Version, current: &Version) -> bool {
    candidate.cmp_precedence(current).is_gt()
}

/// The version of the running Axion, from the `CARGO_PKG_VERSION` the caller passes.
pub fn running_version(cargo_pkg_version: &str) -> Version {
    parse_version(cargo_pkg_version).unwrap_or_else(|_| Version::new(0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        parse_version(s).expect("valid version in test")
    }

    #[test]
    fn parses_common_spellings() {
        assert_eq!(v("1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("v1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v(" V0.10.0 "), Version::new(0, 10, 0));
        assert_eq!(v("1.2"), Version::new(1, 2, 0));
        assert_eq!(v("7"), Version::new(7, 0, 0));
        assert_eq!(v("1.2-rc.1").to_string(), "1.2.0-rc.1");
        assert!(parse_version("nightly").is_err());
        assert!(parse_version("").is_err());
        assert!(parse_version("1..2").is_err());
    }

    #[test]
    fn newer_means_strictly_greater() {
        assert!(is_newer(&v("0.2.0"), &v("0.1.9")));
        assert!(is_newer(&v("1.0.0"), &v("0.99.99")));
        assert!(!is_newer(&v("0.1.0"), &v("0.1.0")));
        assert!(!is_newer(&v("0.1.0"), &v("0.2.0")));
        // Numeric, not textual, comparison.
        assert!(is_newer(&v("0.10.0"), &v("0.9.0")));
    }

    #[test]
    fn prereleases_sort_before_their_release() {
        assert!(is_newer(&v("1.0.0"), &v("1.0.0-rc.1")));
        assert!(!is_newer(&v("1.0.0-rc.1"), &v("1.0.0")));
        assert!(is_newer(&v("1.0.0-rc.2"), &v("1.0.0-rc.1")));
        assert!(is_newer(&v("1.0.0-beta"), &v("0.9.0")));
        // Build metadata never decides.
        assert!(!is_newer(&v("1.0.0+abc"), &v("1.0.0+def")));
    }

    #[test]
    fn running_version_falls_back() {
        assert_eq!(running_version("0.0.1"), Version::new(0, 0, 1));
        assert_eq!(running_version("garbage"), Version::new(0, 0, 0));
    }
}
