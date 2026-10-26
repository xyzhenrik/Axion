//! Release providers. The trait is small on purpose: GitHub-style releases and a plain JSON
//! manifest cover every forge (Codeberg, GitLab and self-hosted servers publish a manifest,
//! or a GitHub-compatible API).

use axion_config::UpdateChannel;
use semver::Version;
use serde::Deserialize;

use crate::UpdateError;
use crate::http::Http;
use crate::release::{Asset, Release};
use crate::source::{Repository, Source};
use crate::version::{is_newer, parse_version};

/// Something that knows the newest release.
pub trait Provider {
    /// The newest release on `channel`, or `None` when there is none.
    fn latest(&self, http: &Http, channel: UpdateChannel) -> Result<Option<Release>, UpdateError>;
}

impl Source {
    /// The provider for this source.
    pub fn provider(&self) -> Box<dyn Provider> {
        match self {
            Source::GitHub { api_url, repository } => {
                Box::new(GitHub { api_url: api_url.clone(), repository: repository.clone() })
            }
            Source::Manifest { url } => Box::new(Manifest { url: url.clone() }),
        }
    }
}

// ---- GitHub ----

/// GitHub's Releases API: `/releases/latest` (stable), `/releases` (pre-releases included),
/// and the tags list when a repository has tags but no releases.
pub struct GitHub {
    api_url: String,
    repository: Repository,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
struct GhTag {
    name: String,
}

impl GhRelease {
    fn into_release(self) -> Result<Release, UpdateError> {
        let version = parse_version(&self.tag_name)?;
        Ok(Release {
            prerelease: self.prerelease || !version.pre.is_empty(),
            version,
            name: self.name.unwrap_or_default(),
            notes: self.body.unwrap_or_default(),
            page_url: self.html_url.unwrap_or_default(),
            tag: self.tag_name,
            assets: self
                .assets
                .into_iter()
                .map(|a| Asset { name: a.name, url: a.browser_download_url, size: a.size, sha256: None })
                .collect(),
        })
    }
}

/// The newest of `releases` that `channel` allows (drafts never count; parse failures of
/// individual tags are skipped, as repositories carry tags like `nightly`).
fn newest(releases: Vec<GhRelease>, channel: UpdateChannel) -> Option<Release> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| r.into_release().ok())
        .filter(|r| channel == UpdateChannel::Prerelease || !r.prerelease)
        .max_by(|a, b| a.version.cmp_precedence(&b.version))
}

fn parse_json<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, UpdateError> {
    serde_json::from_str(text).map_err(|e| UpdateError::Parse(e.to_string()))
}

impl GitHub {
    fn base(&self) -> String {
        format!("{}/repos/{}/{}", self.api_url, self.repository.owner, self.repository.name)
    }

    /// The repository's web page, derived from the API address (`api.github.com` →
    /// `github.com`, `host/api/v3` → `host`).
    fn web_url(&self) -> String {
        let api = self.api_url.as_str();
        let web =
            api.replace("://api.", "://").trim_end_matches("/api/v3").trim_end_matches("/api/v1").to_owned();
        format!("{web}/{}/{}", self.repository.owner, self.repository.name)
    }

    fn from_tags(&self, http: &Http, channel: UpdateChannel) -> Result<Option<Release>, UpdateError> {
        let tags: Vec<GhTag> = parse_json(&http.get_text(&format!("{}/tags?per_page=50", self.base()))?)?;
        let newest = tags
            .into_iter()
            .filter_map(|t| parse_version(&t.name).ok().map(|v| (v, t.name)))
            .filter(|(v, _)| channel == UpdateChannel::Prerelease || v.pre.is_empty())
            .max_by(|a, b| a.0.cmp_precedence(&b.0));
        Ok(newest.map(|(version, tag)| Release {
            prerelease: !version.pre.is_empty(),
            page_url: format!("{}/releases/tag/{tag}", self.web_url()),
            version,
            name: String::new(),
            notes: String::new(),
            tag,
            assets: Vec::new(),
        }))
    }
}

fn is_not_found(e: &UpdateError) -> bool {
    matches!(e, UpdateError::Network(ureq::Error::StatusCode(404)))
}

impl Provider for GitHub {
    fn latest(&self, http: &Http, channel: UpdateChannel) -> Result<Option<Release>, UpdateError> {
        let found = match channel {
            UpdateChannel::Stable => match http.get_text(&format!("{}/releases/latest", self.base())) {
                Ok(text) => Some(parse_json::<GhRelease>(&text)?.into_release()?),
                // No releases yet (the endpoint 404s): fall back to plain tags.
                Err(e) if is_not_found(&e) => None,
                Err(e) => return Err(e),
            },
            UpdateChannel::Prerelease => {
                match http.get_text(&format!("{}/releases?per_page=30", self.base())) {
                    Ok(text) => newest(parse_json::<Vec<GhRelease>>(&text)?, channel),
                    Err(e) if is_not_found(&e) => None,
                    Err(e) => return Err(e),
                }
            }
        };
        match found {
            Some(release) => Ok(Some(release)),
            None => self.from_tags(http, channel),
        }
    }
}

// ---- Manifest ----

/// A `latest.json` on any web server:
/// `{"version": "0.2.0", "notes": "…", "url": "https://…/releases/0.2.0", "prerelease": false,
///   "assets": [{"name": "Axion-0.2.0-x86_64.AppImage", "url": "…", "size": 123, "sha256": "…"}]}`.
/// Relative asset addresses are resolved against the manifest's own.
pub struct Manifest {
    url: String,
}

#[derive(Deserialize)]
struct ManifestDoc {
    version: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, alias = "body")]
    notes: Option<String>,
    #[serde(default, alias = "page_url", alias = "html_url")]
    url: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
}

#[derive(Deserialize)]
struct ManifestAsset {
    name: String,
    url: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    sha256: Option<String>,
}

impl Manifest {
    /// Reads a manifest document fetched from `base`.
    pub fn parse(text: &str, base: &str) -> Result<Release, UpdateError> {
        let doc: ManifestDoc = parse_json(text)?;
        let version = parse_version(&doc.version)?;
        let base_url = url::Url::parse(base).map_err(|_| UpdateError::InvalidUrl(base.to_owned()))?;
        let resolve = |reference: &str| {
            base_url.join(reference).map(String::from).unwrap_or_else(|_| reference.to_owned())
        };
        Ok(Release {
            prerelease: doc.prerelease || !version.pre.is_empty(),
            tag: doc.version.clone(),
            name: doc.name.unwrap_or_default(),
            notes: doc.notes.unwrap_or_default(),
            page_url: doc.url.as_deref().map(resolve).unwrap_or_default(),
            version,
            assets: doc
                .assets
                .into_iter()
                .map(|a| Asset {
                    url: resolve(&a.url),
                    name: a.name,
                    size: a.size,
                    sha256: a.sha256.map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()),
                })
                .collect(),
        })
    }
}

impl Provider for Manifest {
    fn latest(&self, http: &Http, channel: UpdateChannel) -> Result<Option<Release>, UpdateError> {
        let release = Self::parse(&http.get_text(&self.url)?, &self.url)?;
        Ok((channel == UpdateChannel::Prerelease || !release.prerelease).then_some(release))
    }
}

/// `Some(release)` when it is newer than `current`.
pub fn newer_than(release: Option<Release>, current: &Version) -> Option<Release> {
    release.filter(|r| is_newer(&r.version, current))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GH_LATEST: &str = r###"{
        "tag_name": "v0.3.0", "name": "Axion 0.3", "body": "## Notes\n- one",
        "html_url": "https://github.com/o/n/releases/tag/v0.3.0", "prerelease": false, "draft": false,
        "assets": [
            {"name": "Axion-0.3.0-x86_64.AppImage", "browser_download_url": "https://github.com/o/n/releases/download/v0.3.0/a.AppImage", "size": 1234},
            {"name": "SHA256SUMS", "browser_download_url": "https://github.com/o/n/releases/download/v0.3.0/SHA256SUMS", "size": 99}
        ],
        "unknown_field": 1
    }"###;

    #[test]
    fn parses_a_github_release() {
        let r = parse_json::<GhRelease>(GH_LATEST).and_then(GhRelease::into_release).expect("parses");
        assert_eq!(r.version, Version::new(0, 3, 0));
        assert_eq!(r.tag, "v0.3.0");
        assert_eq!(r.title(), "Axion 0.3");
        assert_eq!(r.assets.len(), 2);
        assert_eq!(r.assets[0].size, Some(1234));
        assert!(r.checksums_asset().is_some());
        assert!(!r.prerelease);
    }

    #[test]
    fn github_errors_are_reported() {
        assert!(matches!(parse_json::<GhRelease>("{}"), Err(UpdateError::Parse(_))));
        assert!(matches!(parse_json::<GhRelease>("<html>"), Err(UpdateError::Parse(_))));
        let bad_tag = r#"{"tag_name": "nightly"}"#;
        let result = parse_json::<GhRelease>(bad_tag).and_then(GhRelease::into_release);
        assert!(matches!(result, Err(UpdateError::Version(_))));
    }

    fn gh(tag: &str, prerelease: bool, draft: bool) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            name: None,
            body: None,
            html_url: None,
            prerelease,
            draft,
            assets: Vec::new(),
        }
    }

    #[test]
    fn newest_respects_channel_and_drafts() {
        let list = || {
            vec![
                gh("v1.0.0", false, false),
                gh("v1.1.0-rc.1", true, false),
                gh("v2.0.0", false, true),
                gh("nightly", true, false),
            ]
        };
        assert_eq!(newest(list(), UpdateChannel::Stable).map(|r| r.tag), Some("v1.0.0".into()));
        assert_eq!(newest(list(), UpdateChannel::Prerelease).map(|r| r.tag), Some("v1.1.0-rc.1".into()));
        assert!(newest(Vec::new(), UpdateChannel::Stable).is_none());
    }

    #[test]
    fn web_url_follows_the_api_address() {
        let make = |api: &str| GitHub {
            api_url: api.into(),
            repository: Repository { owner: "o".into(), name: "n".into() },
        };
        assert_eq!(make("https://api.github.com").web_url(), "https://github.com/o/n");
        assert_eq!(make("https://git.example.org/api/v3").web_url(), "https://git.example.org/o/n");
        assert_eq!(make("https://codeberg.org/api/v1").web_url(), "https://codeberg.org/o/n");
    }

    #[test]
    fn parses_a_manifest() {
        let text = r#"{
            "version": "0.4.0-beta.2", "notes": "Fixes", "url": "/releases/0.4.0",
            "assets": [
                {"name": "a.AppImage", "url": "files/a.AppImage", "size": 5, "sha256": "ABCDEF"},
                {"name": "SHA256SUMS", "url": "https://cdn.example.org/SHA256SUMS"}
            ]
        }"#;
        let r = Manifest::parse(text, "https://example.org/axion/latest.json").expect("parses");
        assert_eq!(r.version.to_string(), "0.4.0-beta.2");
        assert!(r.prerelease);
        assert_eq!(r.page_url, "https://example.org/releases/0.4.0");
        assert_eq!(r.assets[0].url, "https://example.org/axion/files/a.AppImage");
        assert_eq!(r.assets[0].sha256.as_deref(), Some("abcdef"));
        assert_eq!(r.assets[1].url, "https://cdn.example.org/SHA256SUMS");
        assert!(matches!(Manifest::parse("{}", "https://x.org/l.json"), Err(UpdateError::Parse(_))));
        assert!(matches!(
            Manifest::parse(r#"{"version": "soon"}"#, "https://x.org/l.json"),
            Err(UpdateError::Version(_))
        ));
    }

    #[test]
    fn newer_than_filters() {
        let r = Manifest::parse(r#"{"version": "0.2.0"}"#, "https://x.org/l.json").expect("parses");
        assert!(newer_than(Some(r.clone()), &Version::new(0, 1, 0)).is_some());
        assert!(newer_than(Some(r), &Version::new(0, 2, 0)).is_none());
        assert!(newer_than(None, &Version::new(0, 2, 0)).is_none());
    }
}
