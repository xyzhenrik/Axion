//! Chromium's profile preferences, for the one setting Axion has to make for extensions.
//!
//! Since Chromium 138 the `chrome.userScripts` API (what Tampermonkey and Violentmonkey run
//! their scripts with) exists only for an extension whose "Allow User Scripts" switch is on.
//! The switch lives on `chrome://extensions`, which Axion does not have; it is the profile
//! preference `extensions.settings.<id>.user_scripts_enabled`. Axion sets it, before Chromium
//! starts, for the extensions that ask for the `userScripts` permission: the user has seen
//! that permission ("Run scripts in pages") when installing.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{ExtensionError, Result, io};

/// Switches "Allow User Scripts" on for the extensions `ids` in the preferences file
/// `preferences` (`<profile>/Default/Preferences`). Creates the file if there is none; a file
/// that is not valid JSON is left alone and reported. Only call this while Chromium is not
/// running (it would overwrite the change).
pub fn allow_user_scripts(preferences: &Path, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut root = match std::fs::read_to_string(preferences) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|e| ExtensionError::Manifest(format!("Preferences: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(io(preferences)(e)),
    };
    let invalid = || ExtensionError::Manifest("Preferences: not a JSON object".into());
    let top = root.as_object_mut().ok_or_else(invalid)?;
    let mut changed = false;
    for id in ids {
        let settings = child(top, "extensions").and_then(|e| child(e, "settings")).ok_or_else(invalid)?;
        let extension = child(settings, id).ok_or_else(invalid)?;
        if extension.get("user_scripts_enabled") != Some(&Value::Bool(true)) {
            extension.insert("user_scripts_enabled".into(), Value::Bool(true));
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    if let Some(parent) = preferences.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let out = serde_json::to_string(&root).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    let temporary = preferences.with_extension("axion-tmp");
    std::fs::write(&temporary, out).map_err(io(&temporary))?;
    std::fs::rename(&temporary, preferences).map_err(io(preferences))?;
    tracing::info!(
        count = ids.len(),
        "user scripts allowed for extensions that ask for the userScripts permission"
    );
    Ok(())
}

/// The object `key` of `map`, made if it is missing (or something else).
fn child<'a>(map: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Map<String, Value>> {
    let entry = map.entry(key).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn creates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Default/Preferences");
        allow_user_scripts(&file, &["abc".into()]).unwrap();
        assert_eq!(read(&file)["extensions"]["settings"]["abc"]["user_scripts_enabled"], true);
    }

    #[test]
    fn keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Preferences");
        std::fs::write(
            &file,
            r#"{"a":1,"extensions":{"ui":{"developer_mode":false},"settings":{"abc":{"location":8,"path":"/x"},"other":{"k":1}}}}"#,
        )
        .unwrap();
        allow_user_scripts(&file, &["abc".into(), "new".into()]).unwrap();
        let v = read(&file);
        assert_eq!(v["a"], 1);
        assert_eq!(v["extensions"]["ui"]["developer_mode"], false);
        assert_eq!(v["extensions"]["settings"]["abc"]["path"], "/x");
        assert_eq!(v["extensions"]["settings"]["abc"]["user_scripts_enabled"], true);
        assert_eq!(v["extensions"]["settings"]["new"]["user_scripts_enabled"], true);
        assert!(v["extensions"]["settings"]["other"].get("user_scripts_enabled").is_none());
    }

    #[test]
    fn a_damaged_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Preferences");
        std::fs::write(&file, "{not json").unwrap();
        assert!(allow_user_scripts(&file, &["abc".into()]).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{not json");
    }

    #[test]
    fn nothing_to_do_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Preferences");
        allow_user_scripts(&file, &[]).unwrap();
        assert!(!file.exists());
        std::fs::write(&file, r#"{"extensions":{"settings":{"abc":{"user_scripts_enabled":true}}}}"#)
            .unwrap();
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        allow_user_scripts(&file, &["abc".into()]).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);
    }
}
