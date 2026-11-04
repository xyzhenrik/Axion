//! Keeps the catalogs and the sources in step. German is the reference catalog and is checked
//! strictly (complete, nothing unused). The other catalogs are filled in over time: they only have
//! to be valid, to use only keys the sources use, and to keep the placeholders of their keys;
//! a key they lack falls back to English. `coverage_report` prints how far each one is.

use std::collections::BTreeSet;
use std::path::PathBuf;

use axion_i18n::scan::{placeholders, workspace_keys};
use axion_i18n::{LANGUAGES, Language, LanguageInfo};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parse(info: &LanguageInfo) -> toml::Table {
    let source = info.source.unwrap_or("");
    match source.parse::<toml::Table>() {
        Ok(table) => table,
        // The TOML parser rejects duplicate keys, which is check (c).
        Err(e) => panic!("locales/{}.toml is not valid TOML (duplicate keys count): {e}", info.code),
    }
}

fn catalog() -> toml::Table {
    match axion_i18n::DE_SOURCE.parse::<toml::Table>() {
        Ok(table) => table,
        Err(e) => panic!("locales/de.toml is not valid TOML (duplicate keys count): {e}"),
    }
}

/// The catalogs that are filled in over time (every language except English and German).
fn secondary() -> impl Iterator<Item = &'static LanguageInfo> {
    LANGUAGES.iter().filter(|i| i.source.is_some() && i.language != Language::German)
}

/// (a) Every key used in the sources is in the German catalog; run
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

fn placeholder_problems(file: &str, table: &toml::Table) -> Vec<String> {
    let mut bad = Vec::new();
    for (key, value) in table {
        let Some(text) = value.as_str() else {
            bad.push(format!("  {file} {key:?}: value is not a string"));
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let mut want = placeholders(key);
        let mut have = placeholders(text);
        want.sort();
        have.sort();
        if want != have {
            bad.push(format!("  {file} {key:?}: expected {want:?}, translation has {have:?}"));
        }
    }
    bad
}

/// (b) A translation uses exactly the placeholders (`{name}`, `%s`) of its key (any order).
#[test]
fn placeholders_match() {
    let bad = placeholder_problems("de", &catalog());
    assert!(bad.is_empty(), "placeholder mismatches in locales/de.toml:\n{}", bad.join("\n"));
}

#[test]
fn placeholder_scanner_sees_braces_and_printf() {
    assert_eq!(placeholders("{n} of {total}"), ["n", "total"]);
    assert_eq!(placeholders("Open %s in %d tabs"), ["%s", "%d"]);
    assert_eq!(placeholders("%1$s then %2$s"), ["%1$s", "%2$s"]);
    // A lone percent sign (or one after a placeholder) is text.
    assert!(placeholders("{percent} %").contains(&"percent".to_owned()));
    assert_eq!(placeholders("{percent} %").len(), 1);
    assert!(placeholders("100% sure").is_empty());
}

/// (c) No duplicate keys (the TOML parser refuses them) and no entry is a table.
#[test]
fn keys_are_unique_strings() {
    for (key, value) in catalog() {
        assert!(value.is_str(), "{key:?} must map to a string");
    }
}

/// The other catalogs: valid TOML, plain string values, placeholders kept. A missing key is fine
/// (it falls back to English), a key the sources do not use is not (typo in the English text).
#[test]
fn secondary_catalogs_are_well_formed() {
    let used: BTreeSet<String> = workspace_keys(&root()).into_keys().collect();
    let mut bad = Vec::new();
    for info in secondary() {
        let table = parse(info);
        bad.extend(placeholder_problems(info.code, &table));
        for key in table.keys() {
            if !used.contains(key) {
                bad.push(format!("  {} {key:?}: no source uses this key", info.code));
            }
        }
    }
    assert!(bad.is_empty(), "problems in the language catalogs:\n{}", bad.join("\n"));
}

/// Every language has a catalog file with the standard header, and the embedded text is the file.
#[test]
fn every_language_has_its_catalog_file() {
    for info in LANGUAGES.iter().filter(|i| i.source.is_some()) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("locales/{}.toml", info.code));
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(Some(on_disk.as_str()), info.source, "{} is not what is embedded", path.display());
        assert!(on_disk.starts_with("# "), "{} lacks the header comment", path.display());
    }
    assert!(
        LANGUAGES.iter().any(|i| i.code == "de") && LANGUAGES.len() == 13,
        "the registry should list English plus the 12 catalogs"
    );
}

/// Prints the share of translated keys per language (`cargo test -p axion-i18n coverage_report --
/// --nocapture`). German must be complete; the others only report.
#[test]
fn coverage_report() {
    let used = workspace_keys(&root());
    let mut lines = vec![format!("{} keys in use", used.len())];
    for info in LANGUAGES.iter().filter(|i| i.source.is_some()) {
        let (done, total) = axion_i18n::coverage(info.language, used.keys().map(String::as_str));
        let percent = if total == 0 { 100.0 } else { 100.0 * done as f64 / total as f64 };
        lines.push(format!("{:>8} {:>5.1}%  {done}/{total}  {}", info.code, percent, info.english_name));
        if info.language == Language::German {
            assert_eq!(done, total, "German must be complete");
        }
    }
    println!("{}", lines.join("\n"));
}
