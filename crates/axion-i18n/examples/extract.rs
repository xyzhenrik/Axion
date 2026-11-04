//! Adds every translation key used in the sources but missing from a catalog (with an empty
//! value, which falls back to English) and lists keys nothing uses any more. Creates a catalog
//! file that does not exist yet (with the standard header).
//!
//! `cargo run -p axion-i18n --example extract` handles every language in the registry;
//! `cargo run -p axion-i18n --example extract -- de fr` only those codes. After adding a
//! language to `LANGUAGES` in `src/lib.rs`, run it once to create its catalog file (the
//! `include_str!` needs the file to exist, so create an empty one first).

use std::collections::BTreeMap;
use std::path::PathBuf;

use axion_i18n::scan::{toml_quote, workspace_keys};
use axion_i18n::{LANGUAGES, LanguageInfo};

fn header(info: &LanguageInfo) -> String {
    format!(
        "# {english} catalog ({native}): \"English source text\" = \"translation\". An empty value falls back to English.\n\
         # Keep {{placeholders}} (and %s) exactly as in the key. See GLOSSARY.md. Regenerate missing keys with\n\
         # `cargo run -p axion-i18n --example extract`.\n",
        english = info.english_name,
        native = info.native_name,
    )
}

fn main() {
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let used = workspace_keys(&root);
    println!("{} keys in use", used.len());
    for info in LANGUAGES.iter().filter(|i| i.source.is_some()) {
        if !wanted.is_empty() && !wanted.iter().any(|w| w == info.code) {
            continue;
        }
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("locales/{}.toml", info.code));
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.trim().is_empty() {
            text = header(info);
        }
        let table: toml::Table = text.parse().unwrap_or_else(|e| {
            eprintln!("{} is invalid: {e}", path.display());
            std::process::exit(1);
        });
        let mut by_file: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (key, at) in &used {
            if !table.contains_key(key) {
                let file = at.rsplit_once(':').map_or(at.as_str(), |(f, _)| f);
                by_file.entry(file).or_default().push(key);
            }
        }
        let mut out = text.clone();
        if !out.ends_with('\n') {
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
        if (added > 0 || out != text)
            && let Err(e) = std::fs::write(&path, out)
        {
            eprintln!("writing {}: {e}", path.display());
            std::process::exit(1);
        }
        println!("{}: {added} keys added", info.code);
        let unused: Vec<&String> = table.keys().filter(|k| !used.contains_key(*k)).collect();
        if !unused.is_empty() {
            println!("{}: {} unused keys (delete them):", info.code, unused.len());
            for key in unused {
                println!("  {key:?}");
            }
        }
    }
}
