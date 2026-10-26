//! Axion's updater.
//!
//! Looks for newer releases of Axion in a repository's releases (GitHub, or a compatible API)
//! or in a JSON manifest, verifies downloads against `SHA256SUMS`, and applies updates by
//! installation kind (see [`install`]). The crate has no UI and no global state; the app runs
//! it on a background thread.
//!
//! **Inert until a source is set**: with `updates.repository` and `updates.manifest_url` empty
//! ([`axion_config::DEFAULT_REPOSITORY`]) nothing here touches the network. No telemetry: a
//! check is one GET with `Axion/<version>` as the user agent.

mod checksum;
mod error;
mod http;
pub mod install;
mod provider;
mod release;
mod schedule;
mod source;
#[cfg(test)]
mod testing;
mod version;

pub use axion_config::{UpdateChannel, Updates};
pub use checksum::{expected_hash, sha256_bytes, sha256_file, verify_file};
pub use error::UpdateError;
pub use http::Http;
pub use install::{
    AppImageInstalled, Checkout, InstallKind, Progress, Step, Tools, detect, detect_current,
    inspect_checkout, install_appimage, is_clean, published_checksum, update_checkout,
};
pub use provider::{GitHub, Manifest, Provider, newer_than};
pub use release::{Asset, Release, plain_notes};
pub use schedule::{
    Clock, SystemClock, auto_check_due, is_skipped, may_notify, record_check, remind_later, skip,
};
pub use semver::Version;
pub use source::{Repository, Source, parse_repository};
pub use version::{is_newer, parse_version, running_version};

/// `Axion/<version>`, the user agent of every request.
pub fn user_agent(version: &str) -> String {
    format!("Axion/{version}")
}

/// The result of asking the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    /// The newest release the channel offers (even when it is not newer than the running one).
    pub latest: Option<Release>,
    /// `latest`, when it is newer than the running version.
    pub update: Option<Release>,
}

/// Asks the configured source for the newest release and compares it with `current`.
///
/// `Err(UpdateError::NoSource)` while no repository or manifest is set — nothing is sent then.
pub fn check(updates: &Updates, current: &Version) -> Result<CheckReport, UpdateError> {
    let source = Source::from_config(updates)?;
    let http = Http::new(updates, &user_agent(&current.to_string()));
    let latest = source.provider().latest(&http, updates.channel)?;
    let update = newer_than(latest.clone(), current);
    Ok(CheckReport { latest, update })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Route, TestServer};

    fn release_json(version: &str, base: &str) -> String {
        format!(
            r###"{{"tag_name":"v{version}","name":"Axion {version}","body":"## Hi\n- a","html_url":"{base}/rel",
               "prerelease":false,"draft":false,"assets":[]}}"###
        )
    }

    fn updates_for(server: &TestServer) -> Updates {
        Updates {
            repository: "o/n".into(),
            api_url: server.url(""),
            timeout_seconds: 5,
            ..Updates::default()
        }
    }

    #[test]
    fn no_source_means_no_request() {
        let server = TestServer::start(Vec::new());
        let updates = Updates { api_url: server.url(""), repository: String::new(), ..Updates::default() };
        assert!(matches!(check(&updates, &Version::new(0, 0, 1)), Err(UpdateError::NoSource)));
        assert_eq!(server.requests(), 0);
    }

    #[test]
    fn github_update_is_found_and_up_to_date_is_reported() {
        let server = TestServer::start(Vec::new());
        server.set(vec![Route::json("/repos/o/n/releases/latest", &release_json("0.2.0", &server.url("")))]);
        let updates = updates_for(&server);
        let report = check(&updates, &Version::new(0, 1, 0)).expect("check works");
        assert_eq!(report.update.as_ref().map(|r| r.version.to_string()), Some("0.2.0".into()));
        let again = check(&updates, &Version::new(0, 2, 0)).expect("check works");
        assert!(again.update.is_none());
        assert!(again.latest.is_some());
        // The user agent names the running version and nothing else identifies the user.
        let seen = server.last_headers();
        assert!(seen.to_ascii_lowercase().contains("user-agent: axion/0.2.0"), "{seen}");
        assert!(!seen.to_ascii_lowercase().contains("cookie"));
    }

    #[test]
    fn github_falls_back_to_tags_without_releases() {
        let server = TestServer::start(Vec::new());
        server.set(vec![
            Route::status("/repos/o/n/releases/latest", 404, "{}"),
            Route::json(
                "/repos/o/n/tags",
                r#"[{"name":"nightly"},{"name":"v0.3.0-rc.1"},{"name":"v0.2.5"},{"name":"v0.2.0"}]"#,
            ),
        ]);
        let updates = updates_for(&server);
        let report = check(&updates, &Version::new(0, 1, 0)).expect("tags work");
        assert_eq!(report.update.map(|r| r.tag), Some("v0.2.5".into()));
        let beta = Updates { channel: UpdateChannel::Prerelease, ..updates };
        server.add(Route::status("/repos/o/n/releases", 404, "{}"));
        let report = check(&beta, &Version::new(0, 1, 0)).expect("tags work");
        assert_eq!(report.update.map(|r| r.tag), Some("v0.3.0-rc.1".into()));
    }

    #[test]
    fn manifest_provider_and_errors() {
        let server = TestServer::start(Vec::new());
        server.set(vec![
            Route::json(
                "/latest.json",
                r#"{"version":"1.0.0","notes":"n","assets":[{"name":"a","url":"a.bin"}]}"#,
            ),
            Route::json("/broken.json", "not json"),
            Route::status("/gone.json", 500, "oops"),
        ]);
        let manifest =
            |path: &str| Updates { manifest_url: server.url(path), timeout_seconds: 5, ..Updates::default() };
        let report = check(&manifest("/latest.json"), &Version::new(0, 9, 0)).expect("manifest works");
        let release = report.update.expect("newer");
        assert_eq!(release.assets[0].url, server.url("/a.bin"));
        assert!(matches!(
            check(&manifest("/broken.json"), &Version::new(0, 9, 0)),
            Err(UpdateError::Parse(_))
        ));
        assert!(matches!(
            check(&manifest("/gone.json"), &Version::new(0, 9, 0)),
            Err(UpdateError::Network(_))
        ));
        assert!(matches!(
            check(&manifest("/missing.json"), &Version::new(0, 9, 0)),
            Err(UpdateError::Network(_))
        ));
    }

    #[test]
    fn a_dead_server_fails_fast() {
        let updates = Updates {
            manifest_url: "http://127.0.0.1:9/latest.json".into(),
            timeout_seconds: 2,
            ..Updates::default()
        };
        assert!(matches!(check(&updates, &Version::new(0, 1, 0)), Err(UpdateError::Network(_))));
    }
}
