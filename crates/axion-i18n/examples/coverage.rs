//! Prints how much of each catalog is translated (non-empty values over the keys the sources
//! use), plus the keys a catalog still lacks with `-- --missing <code>`.
//!
//! `cargo run -p axion-i18n --example coverage`

use std::path::PathBuf;

use axion_i18n::scan::workspace_keys;
use axion_i18n::{LANGUAGES, coverage, translation};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let used = workspace_keys(&root);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(at) = args.iter().position(|a| a == "--missing") {
        let code = args.get(at + 1).map(String::as_str).unwrap_or("");
        let Some(info) = LANGUAGES.iter().find(|i| i.code == code) else {
            eprintln!("usage: coverage --missing <language code>");
            std::process::exit(2);
        };
        for key in used.keys().filter(|k| translation(info.language, k).is_none()) {
            println!("{key:?}");
        }
        return;
    }
    println!("{} keys in use", used.len());
    for info in LANGUAGES.iter().filter(|i| i.source.is_some()) {
        let (done, total) = coverage(info.language, used.keys().map(String::as_str));
        let percent = if total == 0 { 100.0 } else { 100.0 * done as f64 / total as f64 };
        println!("{:>8}  {:>5.1}%  {done}/{total}  {}", info.code, percent, info.english_name);
    }
}
