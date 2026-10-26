//! UI translations, gettext-style: the **English source text is the key**.
//!
//! ```ignore
//! label.set_text(tr("New Tab"));                       // plain
//! let s = tr_fmt!("Delete {count} items?", count = n); // named placeholders
//! let s = tr_n("{n} download", "{n} downloads", n);    // plural (English one/other)
//! const TITLES: [&str; 2] = [tr_mark("Home"), tr_mark("Back")]; // const context: mark, then `tr(title)`
//! ```
//!
//! The catalog (`locales/de.toml`, `"English" = "Deutsch"`) is embedded and parsed once. A missing
//! or empty entry falls back to the English key, so a half-translated catalog always works.
//! The language is process-wide ([`set_language`]); the app applies `general.language` at start.
//! `cargo test -p axion-i18n` scans the sources for every key (see `tests/catalog.rs`) and
//! `cargo run -p axion-i18n --example extract` adds missing ones to `de.toml`.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

pub mod scan;

/// A UI language the catalog can be shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    German,
}

impl Language {
    /// The language for a locale name such as `de_DE.UTF-8`, `de`, `en_US`; English when unknown.
    pub fn from_locale(locale: &str) -> Language {
        let code = locale.split(['_', '-', '.', '@', ':']).next().unwrap_or("").to_ascii_lowercase();
        match code.as_str() {
            "de" => Language::German,
            _ => Language::English,
        }
    }

    /// The language of the environment (`LC_ALL`, `LC_MESSAGES`, `LANGUAGE`, `LANG`, first set wins).
    pub fn from_environment() -> Language {
        for var in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
            if let Ok(value) = std::env::var(var)
                && !value.is_empty()
                && value != "C"
                && value != "POSIX"
            {
                return Language::from_locale(&value);
            }
        }
        Language::English
    }

    /// BCP 47 tag (`en`, `de`), for `<html lang>` and the like.
    pub fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::German => "de",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Switches the language. Bumps [`generation`] when it actually changes.
pub fn set_language(language: Language) {
    let value = match language {
        Language::English => 0,
        Language::German => 1,
    };
    if CURRENT.swap(value, Ordering::Relaxed) != value {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn language() -> Language {
    if CURRENT.load(Ordering::Relaxed) == 1 { Language::German } else { Language::English }
}

/// Counts language changes; UI that caches text compares it to know when to rebuild.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// The embedded German catalog source.
pub const DE_SOURCE: &str = include_str!("../locales/de.toml");

fn german() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| parse_catalog(DE_SOURCE))
}

/// Parses a catalog file into its non-empty `English = translation` pairs.
pub fn parse_catalog(source: &str) -> HashMap<String, String> {
    match source.parse::<toml::Table>() {
        Ok(table) => table
            .into_iter()
            .filter_map(|(key, value)| match value {
                toml::Value::String(text) if !text.is_empty() => Some((key, text)),
                _ => None,
            })
            .collect(),
        Err(e) => {
            tracing::error!("translation catalog unreadable: {e}");
            HashMap::new()
        }
    }
}

/// Translates `english` into the current language (itself when untranslated).
pub fn tr(english: &'static str) -> &'static str {
    lookup(english).unwrap_or(english)
}

/// The translation of `english` if the current language has one.
fn lookup(english: &str) -> Option<&'static str> {
    match language() {
        Language::English => None,
        Language::German => german().get(english).map(String::as_str),
    }
}

/// Like [`tr`] for text that is not a `'static` literal (the key must still be in the catalog).
pub fn tr_dyn(english: &str) -> String {
    lookup(english).unwrap_or(english).to_owned()
}

/// Marks a literal as translatable without translating it (for `const`/`static` tables; the
/// text is translated with [`tr`] where it is shown). Identity at runtime; the extractor sees it.
pub const fn tr_mark(english: &'static str) -> &'static str {
    english
}

/// Replaces `{name}` placeholders in `template`. Unknown placeholders stay as written.
pub fn format_named(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        if let Some(close) = tail.find('}')
            && let Some((_, value)) = args.iter().find(|(name, _)| *name == &tail[..close])
        {
            out.push_str(&value.to_string());
            rest = &tail[close + 1..];
        } else {
            out.push('{');
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// Plural: `one` when `n == 1`, else `other`; both are catalog keys that may contain `{n}`.
pub fn tr_n(one: &'static str, other: &'static str, n: usize) -> String {
    let template = tr(if n == 1 { one } else { other });
    format_named(template, &[("n", &n)])
}

/// Translated text with named placeholders: `tr_fmt!("Delete {count} items?", count = n)`.
/// The placeholders of the translation may be reordered freely.
#[macro_export]
macro_rules! tr_fmt {
    ($text:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::format_named($crate::tr($text), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

/// `tr_fmt!` for a text that is not a literal: the key is declared elsewhere with `tr_mark`.
#[macro_export]
macro_rules! tr_fmt_dyn {
    ($text:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::format_named(&$crate::tr_dyn($text), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_names() {
        assert_eq!(Language::from_locale("de_DE.UTF-8"), Language::German);
        assert_eq!(Language::from_locale("de"), Language::German);
        assert_eq!(Language::from_locale("en_US.UTF-8"), Language::English);
        assert_eq!(Language::from_locale("fr_FR"), Language::English);
        assert_eq!(Language::from_locale(""), Language::English);
    }

    #[test]
    fn named_placeholders_reorder() {
        let (a, b) = (3, "x");
        assert_eq!(format_named("{b} has {a}", &[("a", &a), ("b", &b)]), "x has 3");
        assert_eq!(format_named("keep {unknown} and { open", &[("a", &a)]), "keep {unknown} and { open");
    }

    #[test]
    fn untranslated_falls_back() {
        assert_eq!(tr("Definitely not in the catalog"), "Definitely not in the catalog");
        assert_eq!(tr_n("{n} thing", "{n} things", 1), "1 thing");
        assert_eq!(tr_n("{n} thing", "{n} things", 2), "2 things");
        assert_eq!(tr_fmt!("{a}-{b}", a = 1, b = 2), "1-2");
    }

    #[test]
    fn catalog_parses_and_skips_empty() {
        let c = parse_catalog("\"A\" = \"B\"\n\"C\" = \"\"\n");
        assert_eq!(c.get("A").map(String::as_str), Some("B"));
        assert!(!c.contains_key("C"));
    }
}
