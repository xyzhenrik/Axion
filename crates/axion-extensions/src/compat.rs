//! What Axion switches off in an installed extension because it does not work in the engine.
//!
//! An extension that holds the request APIs (`webRequest`, `declarativeNetRequest`) makes
//! Chromium route every page request through the extension system; in Axion's off-screen
//! browser that stalls navigations to websites (the page never starts loading). Until that is
//! fixed in the engine, Axion removes those permissions (and the static rule files of
//! `declarative_net_request`) from its copy of the manifest. Content scripts, service workers,
//! `chrome.userScripts` and the other APIs are not touched, so userscript managers such as
//! Tampermonkey run. Switched off by `extensions.block_request_apis = false`.

use std::path::Path;

use serde_json::Value;

use crate::{ExtensionError, Installed, Manifest, Result, io};

/// The API permissions that are removed.
pub const REQUEST_PERMISSIONS: &[&str] = &[
    "webRequest",
    "webRequestBlocking",
    "webRequestAuthProvider",
    "declarativeNetRequest",
    "declarativeNetRequestWithHostAccess",
    "declarativeNetRequestFeedback",
];

/// Switches the request APIs off in a freshly installed extension and re-reads its manifest;
/// `installed.stripped` says what was removed.
pub fn disable_request_apis(installed: &mut Installed) -> Result<()> {
    installed.stripped = strip_request_apis(&installed.dir)?;
    if !installed.stripped.is_empty() {
        installed.manifest = Manifest::read(&installed.dir)?;
    }
    Ok(())
}

/// Removes the request API permissions from `<dir>/manifest.json` (and its
/// `declarative_net_request` rule list). Returns the permissions that were there, in manifest
/// order; empty (and the file untouched) if there were none.
pub fn strip_request_apis(dir: &Path) -> Result<Vec<String>> {
    let path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    let mut value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    let Some(manifest) = value.as_object_mut() else { return Ok(Vec::new()) };
    let mut removed: Vec<String> = Vec::new();
    for key in ["permissions", "optional_permissions"] {
        if let Some(Value::Array(list)) = manifest.get_mut(key) {
            list.retain(|p| match p.as_str() {
                Some(name) if REQUEST_PERMISSIONS.contains(&name) => {
                    if !removed.iter().any(|r| r == name) {
                        removed.push(name.to_owned());
                    }
                    false
                }
                _ => true,
            });
        }
    }
    let rules = manifest.remove("declarative_net_request").is_some();
    if removed.is_empty() && !rules {
        return Ok(removed);
    }
    let out = serde_json::to_string_pretty(&value).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    std::fs::write(&path, out).map_err(io(&path))?;
    tracing::info!(dir = %dir.display(), ?removed, "request APIs switched off for an extension");
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, json: &str) {
        std::fs::write(dir.join("manifest.json"), json).unwrap();
    }

    #[test]
    fn the_request_apis_go_and_the_rest_stays() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"manifest_version":3,"name":"X","version":"1",
            "permissions":["tabs","webRequest","storage","webRequestBlocking","declarativeNetRequestWithHostAccess"],
            "optional_permissions":["declarativeNetRequestFeedback","downloads"],
            "declarative_net_request":{"rule_resources":[{"id":"r","enabled":true,"path":"r.json"}]},
            "host_permissions":["<all_urls>"]}"#,
        );
        let removed = strip_request_apis(dir.path()).unwrap();
        assert_eq!(
            removed,
            [
                "webRequest",
                "webRequestBlocking",
                "declarativeNetRequestWithHostAccess",
                "declarativeNetRequestFeedback"
            ]
        );
        let manifest = crate::Manifest::read(dir.path()).unwrap();
        assert_eq!(manifest.permissions.apis, ["tabs", "storage"]);
        assert_eq!(manifest.permissions.optional, ["downloads"]);
        assert!(manifest.permissions.broad());
        let text = std::fs::read_to_string(dir.path().join("manifest.json")).unwrap();
        assert!(!text.contains("declarative_net_request"));
        // Done once: nothing left to do, nothing rewritten.
        assert!(strip_request_apis(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn an_extension_without_them_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"manifest_version":3,"name":"X","version":"1","permissions":["tabs"]}"#;
        write(dir.path(), json);
        assert!(strip_request_apis(dir.path()).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("manifest.json")).unwrap(), json);
    }

    #[test]
    fn a_missing_or_broken_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(strip_request_apis(dir.path()).is_err());
        write(dir.path(), "{nope");
        assert!(strip_request_apis(dir.path()).is_err());
    }
}
