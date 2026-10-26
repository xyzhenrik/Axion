//! Keeps `locales/de.toml` and the sources in step.

use std::collections::BTreeSet;
use std::path::PathBuf;

use axion_i18n::scan::{placeholders, workspace_keys};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn catalog() -> toml::Table {
    match axion_i18n::DE_SOURCE.parse::<toml::Table>() {
        Ok(table) => table,
        // The TOML parser rejects duplicate keys, which is check (c).
        Err(e) => panic!("locales/de.toml is not valid TOML (duplicate keys count): {e}"),
    }
}

/// (a) Every key used in the sources is in the catalog; run
/// `cargo run -p axion-i18n --example extract` to add the missing ones.
#[test]
fn every_used_key_is_in_the_catalog() {
    let catalog = catalog();
    let missing: Vec<String> = workspace_keys(&root())
        .into_iter()
        .filter(|(key, _)| !catalog.contains_key(key))
        .map(|(key, at)| format!("  {at}: {key:?}"))
        .collect();
    assert!(
        missing.is_empty(),
        "{} translation keys missing from locales/de.toml (run `cargo run -p axion-i18n --example extract`):\n{}",
        missing.len(),
        missing.join("\n")
    );
}

/// Keys nobody uses any more would rot in the catalog.
#[test]
fn no_unused_keys() {
    let used: BTreeSet<String> = workspace_keys(&root()).into_keys().collect();
    let table = catalog();
    let unused: Vec<&String> = table.keys().filter(|k| !used.contains(*k)).collect();
    assert!(unused.is_empty(), "keys in locales/de.toml that no source uses (delete them): {unused:#?}");
}

/// (b) A translation uses exactly the placeholders of its key (any order).
#[test]
fn placeholders_match() {
    let mut bad = Vec::new();
    for (key, value) in catalog() {
        let Some(text) = value.as_str() else {
            bad.push(format!("  {key:?}: value is not a string"));
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let mut want = placeholders(&key);
        let mut have = placeholders(text);
        want.sort();
        have.sort();
        if want != have {
            bad.push(format!("  {key:?}: expected {want:?}, translation has {have:?}"));
        }
    }
    assert!(bad.is_empty(), "placeholder mismatches in locales/de.toml:\n{}", bad.join("\n"));
}

/// (c) No duplicate keys (the TOML parser refuses them) and no entry is a table.
#[test]
fn keys_are_unique_strings() {
    for (key, value) in catalog() {
        assert!(value.is_str(), "{key:?} must map to a string");
    }
}
