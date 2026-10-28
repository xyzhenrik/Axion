//! `manifest.json`: the parts Axion shows, and the extension id.

use std::path::Path;

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ExtensionError, Result, io};

/// What an extension may access, as its manifest declares it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    /// API permissions (`tabs`, `storage`, `cookies` …), asked for at install.
    pub apis: Vec<String>,
    /// API permissions it may ask for later.
    pub optional: Vec<String>,
    /// Sites it can read and change: `host_permissions` and the pages its content scripts run on.
    pub hosts: Vec<String>,
}

impl Permissions {
    /// Whether the extension can reach (nearly) every site.
    pub fn broad(&self) -> bool {
        self.hosts.iter().any(|h| is_broad_host(h))
    }

    pub fn is_empty(&self) -> bool {
        self.apis.is_empty() && self.hosts.is_empty()
    }
}

/// The parts of a manifest Axion uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub manifest_version: u32,
    pub permissions: Permissions,
    /// The icon file (relative to the extension's folder), if it has one.
    pub icon: Option<String>,
    /// The toolbar button's popup page.
    pub popup: Option<String>,
    pub options_page: Option<String>,
    /// The `key` field (base64 public key): it fixes the extension id.
    pub key: Option<String>,
}

impl Manifest {
    /// Reads `<dir>/manifest.json`.
    pub fn read(dir: &Path) -> Result<Manifest> {
        let path = dir.join("manifest.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ExtensionError::NoManifest),
            Err(e) => return Err(io(&path)(e)),
        };
        Self::parse(&text, dir)
    }

    /// Parses manifest text; `dir` resolves `__MSG_x__` names from `_locales`.
    pub fn parse(text: &str, dir: &Path) -> Result<Manifest> {
        // Chromium accepts a leading byte order mark.
        let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        let Some(object) = value.as_object() else {
            return Err(ExtensionError::Manifest("the top level is not an object".into()));
        };
        let manifest_version = object.get("manifest_version").and_then(Value::as_u64).unwrap_or(0) as u32;
        if manifest_version != 3 {
            return Err(ExtensionError::UnsupportedManifest(manifest_version));
        }
        let text_of = |key: &str| object.get(key).and_then(Value::as_str).map(|s| localized(s, object, dir));
        let name = text_of("name").filter(|n| !n.trim().is_empty());
        let Some(name) = name else {
            return Err(ExtensionError::Manifest("the extension has no name".into()));
        };
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let mut hosts = strings(object.get("host_permissions"));
        if let Some(scripts) = object.get("content_scripts").and_then(Value::as_array) {
            for script in scripts {
                hosts.extend(strings(script.get("matches")));
            }
        }
        dedup(&mut hosts);
        let mut apis = strings(object.get("permissions"));
        dedup(&mut apis);
        let mut optional = strings(object.get("optional_permissions"));
        optional.retain(|p| !apis.contains(p));
        dedup(&mut optional);
        let action = object.get("action").or_else(|| object.get("browser_action"));
        let popup = action.and_then(|a| a.get("default_popup")).and_then(Value::as_str).and_then(safe_path);
        let options_page = object
            .get("options_page")
            .or_else(|| object.get("options_ui").and_then(|o| o.get("page")))
            .and_then(Value::as_str)
            .and_then(safe_path);
        Ok(Manifest {
            name,
            version: text_of("version").unwrap_or_default(),
            description: text_of("description").unwrap_or_default(),
            manifest_version,
            permissions: Permissions { apis, optional, hosts },
            icon: pick_icon(object.get("icons"), action.and_then(|a| a.get("default_icon"))),
            popup,
            options_page,
            key: object.get("key").and_then(Value::as_str).map(|k| k.split_whitespace().collect()),
        })
    }
}

fn dedup(list: &mut Vec<String>) {
    let mut seen = Vec::new();
    list.retain(|item| {
        let new = !seen.contains(item);
        if new {
            seen.push(item.clone());
        }
        new
    });
}

/// A path inside the extension: relative, without `..` (so it can't point out of the folder).
fn safe_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    let ok = !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|part| !part.is_empty() && part != ".." && part != ".");
    ok.then(|| path.to_owned())
}

/// The icon to show: the smallest one of at least 48 px, else the biggest. `icons` maps sizes
/// to paths; the action's `default_icon` is a map too, or one path.
fn pick_icon(icons: Option<&Value>, default_icon: Option<&Value>) -> Option<String> {
    let pick = |value: &Value| -> Option<String> {
        if let Some(path) = value.as_str() {
            return safe_path(path);
        }
        let mut sized: Vec<(u32, &str)> = value
            .as_object()?
            .iter()
            .filter_map(|(size, path)| Some((size.parse().ok()?, path.as_str()?)))
            .collect();
        sized.sort_unstable();
        let chosen = sized.iter().find(|(size, _)| *size >= 48).or(sized.last())?;
        safe_path(chosen.1)
    };
    icons.and_then(pick).or_else(|| default_icon.and_then(pick))
}

/// `__MSG_key__` replaced from `_locales/<default_locale>/messages.json` (or `en`); anything
/// else, and a message that can't be found, stays as it is.
fn localized(text: &str, manifest: &serde_json::Map<String, Value>, dir: &Path) -> String {
    let Some(key) = text.strip_prefix("__MSG_").and_then(|t| t.strip_suffix("__")) else {
        return text.to_owned();
    };
    let default = manifest.get("default_locale").and_then(Value::as_str).unwrap_or("en");
    for locale in [default, "en", "en_US"] {
        if locale.contains(['/', '\\']) || locale.contains("..") {
            continue;
        }
        let path = dir.join("_locales").join(locale).join("messages.json");
        let Ok(content) = std::fs::read_to_string(path) else { continue };
        let Ok(Value::Object(messages)) = serde_json::from_str::<Value>(content.trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        // Message names are case-insensitive.
        let found = messages
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .and_then(|(_, entry)| entry.get("message"))
            .and_then(Value::as_str);
        if let Some(message) = found {
            return message.to_owned();
        }
    }
    text.to_owned()
}

/// Whether a host permission or match pattern covers (nearly) every site.
pub fn is_broad_host(pattern: &str) -> bool {
    matches!(
        pattern,
        "<all_urls>" | "*://*/*" | "http://*/*" | "https://*/*" | "*://*/" | "https://*/" | "http://*/"
    )
}

/// Chromium's extension id for a base64 public key (the manifest's `key`, or a CRX's): the first
/// 16 bytes of its SHA-256 written with the letters `a`–`p`.
pub fn extension_id_for_key(key_base64: &str) -> Option<String> {
    let compact: String = key_base64.split_whitespace().collect();
    let der = base64::engine::general_purpose::STANDARD.decode(compact).ok()?;
    Some(id_from_hash(Sha256::digest(&der).as_slice()))
}

/// The id Chromium gives an unpacked extension without a `key`: derived from its folder's
/// (absolute, symlink-free) path.
pub fn extension_id_for_path(dir: &Path) -> String {
    let absolute = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    id_from_hash(Sha256::digest(absolute.to_string_lossy().as_bytes()).as_slice())
}

pub(crate) fn id_from_hash(hash: &[u8]) -> String {
    hash.iter()
        .take(16)
        .flat_map(|b| [b >> 4, b & 0xF])
        .map(|nibble| char::from(b'a' + nibble))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Manifest> {
        Manifest::parse(json, Path::new("/nonexistent"))
    }

    #[test]
    fn reads_the_parts_axion_shows() {
        let m = parse(
            r#"{"manifest_version":3,"name":"Dark Mode","version":"2.1","description":"Makes it dark",
            "permissions":["storage","tabs","storage"],"optional_permissions":["history","tabs"],
            "host_permissions":["https://*.example.com/*"],
            "content_scripts":[{"matches":["<all_urls>","https://*.example.com/*"],"js":["a.js"]}],
            "icons":{"16":"i16.png","128":"i128.png","48":"i48.png"},
            "action":{"default_popup":"popup.html"},"options_ui":{"page":"opts.html"}}"#,
        )
        .unwrap();
        assert_eq!(m.name, "Dark Mode");
        assert_eq!(m.version, "2.1");
        assert_eq!(m.permissions.apis, ["storage", "tabs"]);
        assert_eq!(m.permissions.optional, ["history"]);
        assert_eq!(m.permissions.hosts, ["https://*.example.com/*", "<all_urls>"]);
        assert!(m.permissions.broad());
        assert_eq!(m.icon.as_deref(), Some("i48.png"));
        assert_eq!(m.popup.as_deref(), Some("popup.html"));
        assert_eq!(m.options_page.as_deref(), Some("opts.html"));
    }

    #[test]
    fn manifest_v2_and_nameless_are_refused() {
        assert!(matches!(
            parse(r#"{"manifest_version":2,"name":"x","version":"1"}"#),
            Err(ExtensionError::UnsupportedManifest(2))
        ));
        assert!(matches!(parse(r#"{"manifest_version":3,"version":"1"}"#), Err(ExtensionError::Manifest(_))));
        assert!(matches!(parse("[]"), Err(ExtensionError::Manifest(_))));
    }

    #[test]
    fn paths_cannot_leave_the_folder() {
        let m = parse(
            r#"{"manifest_version":3,"name":"x","version":"1","icons":{"48":"../../etc/passwd"},
            "action":{"default_popup":"/ok/popup.html"},"options_page":"a/../b.html"}"#,
        )
        .unwrap();
        assert_eq!(m.icon, None);
        assert_eq!(m.popup.as_deref(), Some("ok/popup.html"));
        assert_eq!(m.options_page, None);
    }

    #[test]
    fn messages_come_from_locales() {
        let dir = tempfile::tempdir().unwrap();
        let locale = dir.path().join("_locales/de");
        std::fs::create_dir_all(&locale).unwrap();
        std::fs::write(locale.join("messages.json"), r#"{"AppName":{"message":"Dunkelmodus"}}"#).unwrap();
        let m = Manifest::parse(
            r#"{"manifest_version":3,"name":"__MSG_appname__","version":"1","default_locale":"de","description":"__MSG_missing__"}"#,
            dir.path(),
        )
        .unwrap();
        assert_eq!(m.name, "Dunkelmodus");
        assert_eq!(m.description, "__MSG_missing__");
    }

    #[test]
    fn ids_use_the_letters_a_to_p() {
        // sha256("") starts e3b0c442 98fc1c14 9afbf4c8 996fb924.
        let id = id_from_hash(&Sha256::digest(b""));
        assert_eq!(id, "odlameecjipmbmbejkplpemijjgpljce");
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| (b'a'..=b'p').contains(&b)));
        // base64("") is the empty key.
        assert_eq!(extension_id_for_key("").as_deref(), Some(id.as_str()));
        assert_eq!(extension_id_for_key("!!"), None);
    }
}
