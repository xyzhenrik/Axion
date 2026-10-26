//! Adds every translation key used in the sources but missing from `locales/de.toml` (with an
//! empty value, which falls back to English) and lists keys nothing uses any more.
//!
//! `cargo run -p axion-i18n --example extract`

use std::collections::BTreeMap;
use std::path::PathBuf;

use axion_i18n::scan::{toml_quote, workspace_keys};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales/de.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let table: toml::Table = text.parse().unwrap_or_else(|e| {
        eprintln!("locales/de.toml is invalid: {e}");
        std::process::exit(1);
    });
    let used = workspace_keys(&root);
    let mut by_file: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (key, at) in &used {
        if !table.contains_key(key) {
            let file = at.rsplit_once(':').map_or(at.as_str(), |(f, _)| f);
            by_file.entry(file).or_default().push(key);
        }
    }
    let mut out = text.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let mut added = 0;
    for (file, keys) in &by_file {
        out.push_str(&format!("\n# {file}\n"));
        for key in keys {
            out.push_str(&format!("{} = \"\"\n", toml_quote(key)));
            added += 1;
        }
    }
    if added > 0 {
        std::fs::write(&path, out).expect("writing locales/de.toml");
    }
    println!("{added} keys added, {} keys in use", used.len());
    let unused: Vec<&String> = table.keys().filter(|k| !used.contains_key(*k)).collect();
    if !unused.is_empty() {
        println!("{} unused keys (delete them from de.toml):", unused.len());
        for key in unused {
            println!("  {key:?}");
        }
    }
}
