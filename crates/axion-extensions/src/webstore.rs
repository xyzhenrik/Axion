//! Talking to the Chrome Web Store's update service (`clients2.google.com/service/update2/crx`):
//! download a package by extension id, and ask which installed extensions have a newer version.
//!
//! Only Google's own hosts are contacted, over https, and every redirect is checked against
//! that list. A downloaded package is verified by [`crate::verify_crx3`] before anything of it
//! is unpacked; the update service's SHA-256 (when it gives one) is checked as well.

use std::io::Read;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::{ExtensionError, MAX_BYTES, Result};

/// The update service.
const ENDPOINT: &str = "https://clients2.google.com/service/update2/crx";
/// Redirects followed for one download.
const MAX_REDIRECTS: usize = 5;
/// An update-check answer is a few hundred bytes per extension.
const MAX_XML_BYTES: u64 = 1024 * 1024;

/// Whether `id` looks like an extension id: 32 letters from a to p.
pub fn is_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// The Chromium version to announce, from CEF's version string
/// (`156.0.14+gabc+chromium-156.0.7290.0`). Falls back to the major version alone.
pub fn product_version(cef_version: &str) -> String {
    let from_chromium = cef_version.split('+').find_map(|part| part.strip_prefix("chromium-"));
    match from_chromium {
        Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.') => v.to_owned(),
        _ => {
            let major = cef_version.split('.').next().filter(|m| m.parse::<u32>().is_ok());
            format!("{}.0.0.0", major.unwrap_or("140"))
        }
    }
}

/// The URL that redirects to the package of extension `id`.
pub fn download_url(id: &str, product_version: &str) -> String {
    format!(
        "{ENDPOINT}?response=redirect&prodversion={product_version}&acceptformat=crx2,crx3&x=id%3D{id}%26uc"
    )
}

/// The update check for `installed` (id, version) pairs: one request for all of them.
pub fn update_check_url(installed: &[(String, String)], product_version: &str) -> String {
    let mut url = format!("{ENDPOINT}?prodversion={product_version}&acceptformat=crx3");
    for (id, version) in installed {
        url.push_str(&format!("&x=id%3D{id}%26v%3D{version}%26uc"));
    }
    url
}

/// One extension's answer of an update check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub id: String,
    pub version: String,
    /// Where the package is (a Google host).
    pub url: String,
    /// The package's SHA-256 (hex), when the service says it.
    pub sha256: Option<String>,
}

/// Reads the update service's answer: the extensions with a newer package. Entries without
/// an update (`status="noupdate"`), with a malformed id, or with a download address outside
/// Google's hosts are left out.
pub fn parse_update_response(xml: &str) -> Vec<Update> {
    let mut updates = Vec::new();
    let mut app: Option<String> = None;
    for tag in tags(xml) {
        match tag.name {
            "app" => app = tag.attr("appid").filter(|id| is_extension_id(id)),
            "updatecheck" => {
                let (Some(id), Some("ok"), Some(url), Some(version)) =
                    (app.clone(), tag.attr("status").as_deref(), tag.attr("codebase"), tag.attr("version"))
                else {
                    continue;
                };
                if !is_google_url(&url) || !valid_version(&version) {
                    continue;
                }
                let sha256 = tag
                    .attr("hash_sha256")
                    .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
                updates.push(Update { id, version, url, sha256 });
            }
            _ => {}
        }
    }
    updates
}

/// Whether `candidate` is a newer version than `installed` (dotted numbers; a missing part
/// counts as 0, anything else as not newer).
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> { v.split('.').map(|p| p.parse().ok()).collect() };
    let (Some(mut a), Some(mut b)) = (parse(candidate), parse(installed)) else { return false };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

fn valid_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 40 && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// An https URL on one of Google's hosts (where the Web Store keeps packages).
pub fn is_google_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.contains('@') {
        return false;
    }
    let host = host.split(':').next().unwrap_or_default().to_ascii_lowercase();
    ["google.com", "googleusercontent.com", "gvt1.com"]
        .iter()
        .any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.')))
}

/// A start tag of the response with its attributes.
struct Tag<'a> {
    name: &'a str,
    body: &'a str,
}

impl Tag<'_> {
    /// The value of attribute `name` (single or double quoted), entities decoded.
    fn attr(&self, name: &str) -> Option<String> {
        let mut rest = self.body;
        while let Some(at) = rest.find(name) {
            let before_ok = at == 0 || rest.as_bytes()[at - 1].is_ascii_whitespace();
            let after = &rest[at + name.len()..];
            let after = after.trim_start();
            if before_ok && let Some(value) = after.strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next().filter(|c| *c == '\'' || *c == '"')?;
                let value = &value[1..];
                return value.find(quote).map(|end| decode_entities(&value[..end]));
            }
            rest = &rest[at + name.len()..];
        }
        None
    }
}

fn tags(xml: &str) -> impl Iterator<Item = Tag<'_>> {
    xml.split('<').skip(1).filter_map(|piece| {
        let piece = piece.split('>').next()?;
        let name_end = piece.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(piece.len());
        let name = &piece[..name_end];
        (!name.is_empty() && !name.starts_with(['?', '!', '/']))
            .then_some(Tag { name, body: &piece[name_end..] })
    })
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The HTTP client for the update service. Redirects are followed by hand, so each hop can be
/// checked against Google's hosts.
pub struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub fn new(timeout: Duration, user_agent: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(user_agent)
            .build()
            .into();
        Self { agent }
    }

    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let mut url = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            if !is_google_url(&url) {
                return Err(net("the download address is not a Google address"));
            }
            let mut response = self.agent.get(&url).call().map_err(|e| net(&e.to_string()))?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                url = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
                    .ok_or_else(|| net("redirect without an address"))?;
                continue;
            }
            if status != 200 {
                return Err(ExtensionError::Package(format!(
                    "the Chrome Web Store answered with status {status}"
                )));
            }
            let mut data = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(limit + 1)
                .read_to_end(&mut data)
                .map_err(|e| net(&e.to_string()))?;
            if data.len() as u64 > limit {
                return Err(ExtensionError::TooLarge(limit / (1024 * 1024)));
            }
            return Ok(data);
        }
        Err(net("too many redirects"))
    }

    /// Downloads the package of extension `id`.
    pub fn download(&self, id: &str, product_version: &str) -> Result<Vec<u8>> {
        if !is_extension_id(id) {
            return Err(ExtensionError::Package("not an extension id".into()));
        }
        self.get(&download_url(id, product_version), MAX_BYTES)
    }

    /// Asks which of `installed` (id, version) have a newer package.
    pub fn check_updates(
        &self,
        installed: &[(String, String)],
        product_version: &str,
    ) -> Result<Vec<Update>> {
        if installed.is_empty() {
            return Ok(Vec::new());
        }
        let body = self.get(&update_check_url(installed, product_version), MAX_XML_BYTES)?;
        let xml = String::from_utf8_lossy(&body);
        let current: std::collections::HashMap<_, _> =
            installed.iter().map(|(i, v)| (i.as_str(), v.as_str())).collect();
        Ok(parse_update_response(&xml)
            .into_iter()
            .filter(|u| current.get(u.id.as_str()).is_some_and(|v| is_newer(&u.version, v)))
            .collect())
    }

    /// Downloads the package an [`Update`] points at and checks its SHA-256.
    pub fn download_update(&self, update: &Update) -> Result<Vec<u8>> {
        let data = self.get(&update.url, MAX_BYTES)?;
        if let Some(expected) = &update.sha256 {
            let actual: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(ExtensionError::Package("the download does not match its checksum".into()));
            }
        }
        Ok(data)
    }
}

fn net(message: &str) -> ExtensionError {
    ExtensionError::Network(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &str = include_str!("../tests/fixtures/update_response.xml");

    #[test]
    fn ids() {
        assert!(is_extension_id("dhdgffkkebhmkfjojejmpbldmpobfkfo"));
        assert!(!is_extension_id("dhdgffkkebhmkfjojejmpbldmpobfkf"));
        assert!(!is_extension_id("dhdgffkkebhmkfjojejmpbldmpobfkfq"));
        assert!(!is_extension_id("DHDGFFKKEBHMKFJOJEJMPBLDMPOBFKFO"));
    }

    #[test]
    fn the_product_version_comes_from_the_cef_string() {
        assert_eq!(product_version("156.0.14+gabc123+chromium-156.0.7290.0"), "156.0.7290.0");
        assert_eq!(product_version("156.0.14"), "156.0.0.0");
        assert_eq!(product_version(""), "140.0.0.0");
        assert_eq!(product_version("1.0.0+g+chromium-1&x=2"), "1.0.0.0");
    }

    #[test]
    fn urls() {
        let id = "dhdgffkkebhmkfjojejmpbldmpobfkfo";
        assert_eq!(
            download_url(id, "156.0.1.0"),
            "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=156.0.1.0&acceptformat=crx2,crx3&x=id%3Ddhdgffkkebhmkfjojejmpbldmpobfkfo%26uc"
        );
        let url = update_check_url(&[(id.into(), "5.0".into()), ("a".repeat(32), "1".into())], "156.0.1.0");
        assert!(url.contains("&x=id%3Ddhdgffkkebhmkfjojejmpbldmpobfkfo%26v%3D5.0%26uc&x=id%3Daaaa"));
        assert!(!url.contains("response=redirect"));
    }

    #[test]
    fn google_hosts_only() {
        assert!(is_google_url("https://clients2.google.com/service/update2/crx?x=1"));
        assert!(is_google_url("https://clients2.googleusercontent.com/crx/blobs/x/y.crx"));
        assert!(is_google_url("https://google.com/"));
        assert!(!is_google_url("http://clients2.google.com/"));
        assert!(!is_google_url("https://evilgoogle.com/"));
        assert!(!is_google_url("https://google.com.evil.org/"));
        assert!(!is_google_url("https://google.com@evil.org/"));
        assert!(!is_google_url("https://evil.org/?google.com"));
        assert!(!is_google_url("ftp://google.com/"));
    }

    #[test]
    fn versions() {
        assert!(is_newer("5.5.1", "5.5.0"));
        assert!(is_newer("5.10", "5.9.9"));
        assert!(is_newer("1.0.0.1", "1"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("1.0", "2"));
        assert!(!is_newer("x", "1"));
    }

    #[test]
    fn the_answer_is_read() {
        let updates = parse_update_response(RESPONSE);
        assert_eq!(updates.len(), 1, "{updates:?}");
        let u = &updates[0];
        assert_eq!(u.id, "dhdgffkkebhmkfjojejmpbldmpobfkfo");
        assert_eq!(u.version, "5.5.1");
        assert_eq!(u.url, "https://clients2.googleusercontent.com/crx/blobs/AAAA/DHDG_5_5_1_0.crx?x=1&y=2");
        assert_eq!(u.sha256.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn nonsense_gives_nothing() {
        assert!(parse_update_response("").is_empty());
        assert!(parse_update_response("<html><body>no</body></html>").is_empty());
        assert!(
            parse_update_response(
                "<app appid='x'><updatecheck status='ok' codebase='https://google.com/a' version='1'/>"
            )
            .is_empty()
        );
    }
}
