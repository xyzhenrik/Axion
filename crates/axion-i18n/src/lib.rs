//! UI translations, gettext-style: the **English source text is the key**.
//!
//! ```ignore
//! label.set_text(tr("New Tab"));                       // plain
//! let s = tr_fmt!("Delete {count} items?", count = n); // named placeholders
//! let s = tr_n("{n} download", "{n} downloads", n);    // plural (English one/other)
//! const TITLES: [&str; 2] = [tr_mark("Home"), tr_mark("Back")]; // const context: mark, then `tr(title)`
//! ```
//!
//! One catalog per language (`locales/<code>.toml`, `"English" = "Deutsch"`, see [`LANGUAGES`]) is
//! embedded and parsed on first use. A missing or empty entry falls back to the English key, so a
//! half-translated (or still empty) catalog always works.
//! The language is process-wide ([`set_language`]); the app applies `general.language` at start.
//! `cargo test -p axion-i18n` scans the sources for every key (see `tests/catalog.rs`);
//! `cargo run -p axion-i18n --example extract` adds missing ones to every catalog and
//! `cargo run -p axion-i18n --example coverage` prints how much of each is translated.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

pub mod scan;

/// A UI language the catalog can be shown in. The registry ([`LANGUAGES`]) lists them all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    English,
    German,
    Spanish,
    French,
    Turkish,
    Japanese,
    /// Brazilian Portuguese (also what every other Portuguese locale gets).
    PortugueseBrazil,
    Italian,
    Russian,
    /// Simplified Chinese (`zh`, `zh_CN`, `zh_SG`, `zh-Hans`).
    ChineseSimplified,
    Korean,
    Dutch,
    Polish,
}

/// What the registry knows about one interface language.
#[derive(Debug)]
pub struct LanguageInfo {
    pub language: Language,
    /// BCP 47 tag, also the catalog's file name (`locales/<code>.toml`) and the config value.
    pub code: &'static str,
    /// The language's name in itself, as the language picker shows it.
    pub native_name: &'static str,
    /// The English name (used in comments and tools, never shown in the UI).
    pub english_name: &'static str,
    /// The language code page translation uses (ISO 639-1, no region or script).
    pub translation_code: &'static str,
    /// The embedded catalog; `None` for English, which is the key itself.
    pub source: Option<&'static str>,
}

/// Every interface language, English first. The catalog files are embedded here.
pub const LANGUAGES: [LanguageInfo; 13] = [
    LanguageInfo {
        language: Language::English,
        code: "en",
        native_name: "English",
        english_name: "English",
        translation_code: "en",
        source: None,
    },
    LanguageInfo {
        language: Language::German,
        code: "de",
        native_name: "Deutsch",
        english_name: "German",
        translation_code: "de",
        source: Some(include_str!("../locales/de.toml")),
    },
    LanguageInfo {
        language: Language::Spanish,
        code: "es",
        native_name: "Español",
        english_name: "Spanish",
        translation_code: "es",
        source: Some(include_str!("../locales/es.toml")),
    },
    LanguageInfo {
        language: Language::French,
        code: "fr",
        native_name: "Français",
        english_name: "French",
        translation_code: "fr",
        source: Some(include_str!("../locales/fr.toml")),
    },
    LanguageInfo {
        language: Language::Turkish,
        code: "tr",
        native_name: "Türkçe",
        english_name: "Turkish",
        translation_code: "tr",
        source: Some(include_str!("../locales/tr.toml")),
    },
    LanguageInfo {
        language: Language::Japanese,
        code: "ja",
        native_name: "日本語",
        english_name: "Japanese",
        translation_code: "ja",
        source: Some(include_str!("../locales/ja.toml")),
    },
    LanguageInfo {
        language: Language::PortugueseBrazil,
        code: "pt-BR",
        native_name: "Português (Brasil)",
        english_name: "Portuguese (Brazil)",
        translation_code: "pt",
        source: Some(include_str!("../locales/pt-BR.toml")),
    },
    LanguageInfo {
        language: Language::Italian,
        code: "it",
        native_name: "Italiano",
        english_name: "Italian",
        translation_code: "it",
        source: Some(include_str!("../locales/it.toml")),
    },
    LanguageInfo {
        language: Language::Russian,
        code: "ru",
        native_name: "Русский",
        english_name: "Russian",
        translation_code: "ru",
        source: Some(include_str!("../locales/ru.toml")),
    },
    LanguageInfo {
        language: Language::ChineseSimplified,
        code: "zh-Hans",
        native_name: "简体中文",
        english_name: "Chinese (Simplified)",
        translation_code: "zh",
        source: Some(include_str!("../locales/zh-Hans.toml")),
    },
    LanguageInfo {
        language: Language::Korean,
        code: "ko",
        native_name: "한국어",
        english_name: "Korean",
        translation_code: "ko",
        source: Some(include_str!("../locales/ko.toml")),
    },
    LanguageInfo {
        language: Language::Dutch,
        code: "nl",
        native_name: "Nederlands",
        english_name: "Dutch",
        translation_code: "nl",
        source: Some(include_str!("../locales/nl.toml")),
    },
    LanguageInfo {
        language: Language::Polish,
        code: "pl",
        native_name: "Polski",
        english_name: "Polish",
        translation_code: "pl",
        source: Some(include_str!("../locales/pl.toml")),
    },
];

impl Language {
    /// Every language, in registry order.
    pub fn all() -> impl Iterator<Item = Language> {
        LANGUAGES.iter().map(|info| info.language)
    }

    fn index(self) -> usize {
        LANGUAGES.iter().position(|info| info.language == self).unwrap_or(0)
    }

    /// This language's registry entry.
    pub fn info(self) -> &'static LanguageInfo {
        &LANGUAGES[self.index()]
    }

    /// The language named by an exact tag (`de`, `pt-BR`, `zh-Hans`; case and `_` vs `-` ignored).
    pub fn from_code(code: &str) -> Option<Language> {
        let wanted = code.trim().replace('_', "-");
        LANGUAGES.iter().find(|info| info.code.eq_ignore_ascii_case(&wanted)).map(|info| info.language)
    }

    /// The language for a locale name such as `de_DE.UTF-8`, `pt_BR`, `zh_CN`, `en_US`; `None` when
    /// Axion has no catalog for it. Every Portuguese locale gets Brazilian Portuguese; Chinese only
    /// the simplified script (`zh`, `zh_CN`, `zh_SG`, `zh-Hans`), not `zh_TW`/`zh_HK`/`zh-Hant`.
    pub fn parse_locale(locale: &str) -> Option<Language> {
        // `de_DE.UTF-8@euro` -> `de_DE`
        let name = locale.split(['.', '@']).next().unwrap_or("");
        let mut parts = name.split(['_', '-']).filter(|p| !p.is_empty());
        let primary = parts.next()?.to_ascii_lowercase();
        let rest: Vec<String> = parts.map(|p| p.to_ascii_lowercase()).collect();
        Some(match primary.as_str() {
            "en" => Language::English,
            "de" => Language::German,
            "es" => Language::Spanish,
            "fr" => Language::French,
            "tr" => Language::Turkish,
            "ja" => Language::Japanese,
            "pt" => Language::PortugueseBrazil,
            "it" => Language::Italian,
            "ru" => Language::Russian,
            "ko" => Language::Korean,
            "nl" => Language::Dutch,
            "pl" => Language::Polish,
            "zh" => {
                if rest.iter().any(|p| matches!(p.as_str(), "tw" | "hk" | "mo" | "hant")) {
                    return None;
                }
                Language::ChineseSimplified
            }
            _ => return None,
        })
    }

    /// The language for a locale name; English when unknown.
    pub fn from_locale(locale: &str) -> Language {
        Language::parse_locale(locale).unwrap_or_default()
    }

    /// The language of the environment (`LC_ALL`, `LC_MESSAGES`, `LANGUAGE`, `LANG`; the first one
    /// set and not `C`/`POSIX` decides). `LANGUAGE` is a colon-separated preference list: its first
    /// entry we have a catalog for wins.
    pub fn from_environment() -> Language {
        Language::from_variables(|name| std::env::var(name).ok())
    }

    /// [`Language::from_environment`] over an arbitrary variable source (for tests).
    pub fn from_variables(get: impl Fn(&str) -> Option<String>) -> Language {
        for var in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
            let Some(value) = get(var) else { continue };
            if value.is_empty() || value == "C" || value == "POSIX" {
                continue;
            }
            return value
                .split(':')
                .filter(|entry| !entry.is_empty() && *entry != "C" && *entry != "POSIX")
                .find_map(Language::parse_locale)
                .unwrap_or_default();
        }
        Language::English
    }

    /// BCP 47 tag (`en`, `de`, `pt-BR`, `zh-Hans`), for `<html lang>` and the like.
    pub fn tag(self) -> &'static str {
        self.info().code
    }

    /// The language in its own words (`Deutsch`, `日本語`).
    pub fn native_name(self) -> &'static str {
        self.info().native_name
    }

    /// The ISO 639-1 code page translation knows this language by (`pt-BR` -> `pt`).
    pub fn translation_code(self) -> &'static str {
        self.info().translation_code
    }

    /// The embedded catalog source (`None` for English).
    pub fn catalog_source(self) -> Option<&'static str> {
        self.info().source
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Switches the language. Bumps [`generation`] when it actually changes.
pub fn set_language(language: Language) {
    let value = language.index() as u8;
    if CURRENT.swap(value, Ordering::Relaxed) != value {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn language() -> Language {
    LANGUAGES.get(CURRENT.load(Ordering::Relaxed) as usize).map_or(Language::English, |i| i.language)
}

/// Counts language changes; UI that caches text compares it to know when to rebuild.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// The embedded German catalog source.
pub const DE_SOURCE: &str = include_str!("../locales/de.toml");

/// The parsed catalog of a language (empty for English and for a catalog nobody has filled yet).
fn catalog(language: Language) -> &'static HashMap<String, String> {
    static CATALOGS: [OnceLock<HashMap<String, String>>; LANGUAGES.len()] =
        [const { OnceLock::new() }; LANGUAGES.len()];
    CATALOGS[language.index()]
        .get_or_init(|| language.catalog_source().map(parse_catalog).unwrap_or_default())
}

/// The translation of `english` into `language`; `None` where that catalog has none (the caller
/// shows the English text then).
pub fn translation(language: Language, english: &str) -> Option<&'static str> {
    catalog(language).get(english).map(String::as_str)
}

/// How much of a catalog is translated: `(translated, total)` over the given keys.
pub fn coverage<'a>(language: Language, keys: impl IntoIterator<Item = &'a str>) -> (usize, usize) {
    let mut translated = 0;
    let mut total = 0;
    for key in keys {
        total += 1;
        if translation(language, key).is_some() {
            translated += 1;
        }
    }
    (translated, total)
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
    translation(language(), english)
}

/// Like [`tr`] for text that is not a `'static` literal (the key must still be in the catalog).
pub fn tr_dyn(english: &str) -> String {
    lookup(english).unwrap_or(english).to_owned()
}

/// The current language's translations of the keys used by the given page sources (the start
/// page's HTML and script), for handing to the page. Empty for English.
pub fn page_catalog(sources: &[(&str, scan::Dialect)]) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for (text, dialect) in sources {
        for found in scan::keys(text, *dialect) {
            if let Some(translation) = lookup(&found.key) {
                map.insert(found.key, translation.to_owned());
            }
        }
    }
    map
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
        assert_eq!(Language::from_locale("fr_FR"), Language::French);
        assert_eq!(Language::from_locale("es_MX.UTF-8"), Language::Spanish);
        assert_eq!(Language::from_locale("ja_JP.eucJP"), Language::Japanese);
        assert_eq!(Language::from_locale("tr_TR@x"), Language::Turkish);
        assert_eq!(Language::from_locale("pt_BR.UTF-8"), Language::PortugueseBrazil);
        assert_eq!(Language::from_locale("pt_PT"), Language::PortugueseBrazil);
        assert_eq!(Language::from_locale("zh_CN.UTF-8"), Language::ChineseSimplified);
        assert_eq!(Language::from_locale("zh-Hans"), Language::ChineseSimplified);
        assert_eq!(Language::from_locale("zh"), Language::ChineseSimplified);
        assert_eq!(Language::from_locale("zh_TW.UTF-8"), Language::English);
        assert_eq!(Language::from_locale("zh-Hant-HK"), Language::English);
        assert_eq!(Language::from_locale("sv_SE"), Language::English);
        assert_eq!(Language::from_locale(""), Language::English);
        assert_eq!(Language::parse_locale("sv_SE"), None);
    }

    #[test]
    fn environment_precedence_and_lists() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_owned())
        };
        assert_eq!(Language::from_variables(env(&[])), Language::English);
        assert_eq!(Language::from_variables(env(&[("LANG", "ru_RU.UTF-8")])), Language::Russian);
        // LC_ALL beats LANG; C is skipped.
        assert_eq!(Language::from_variables(env(&[("LC_ALL", "C"), ("LANG", "it_IT")])), Language::Italian);
        assert_eq!(
            Language::from_variables(env(&[("LC_ALL", "ko_KR"), ("LANG", "de_DE")])),
            Language::Korean
        );
        // The first entry of the preference list we can show wins.
        assert_eq!(Language::from_variables(env(&[("LANGUAGE", "sv:nl:de")])), Language::Dutch);
        assert_eq!(Language::from_variables(env(&[("LANGUAGE", "sv:da")])), Language::English);
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(LANGUAGES[0].language, Language::English);
        let mut codes = std::collections::BTreeSet::new();
        for info in &LANGUAGES {
            assert!(codes.insert(info.code), "duplicate code {}", info.code);
            assert_eq!(Language::from_code(info.code), Some(info.language));
            assert_eq!(info.language.info().code, info.code);
            assert_eq!(info.language.tag(), info.code);
            assert_eq!(info.source.is_none(), info.language == Language::English);
            // Chosen language -> set -> read back.
            set_language(info.language);
            assert_eq!(language(), info.language);
        }
        set_language(Language::English);
        assert_eq!(Language::from_code("pt_br"), Some(Language::PortugueseBrazil));
        assert_eq!(Language::from_code("xx"), None);
        assert_eq!(Language::PortugueseBrazil.translation_code(), "pt");
        assert_eq!(Language::ChineseSimplified.translation_code(), "zh");
        assert_eq!(Language::Japanese.native_name(), "日本語");
        assert_eq!(Language::all().count(), LANGUAGES.len());
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
        // An empty or unfilled catalog gives nothing, never an error.
        assert_eq!(translation(Language::English, "New Tab"), None);
        assert_eq!(translation(Language::German, "Definitely not in the catalog"), None);
    }

    #[test]
    fn catalog_parses_and_skips_empty() {
        let c = parse_catalog("\"A\" = \"B\"\n\"C\" = \"\"\n");
        assert_eq!(c.get("A").map(String::as_str), Some("B"));
        assert!(!c.contains_key("C"));
    }

    #[test]
    fn coverage_counts_translated_keys() {
        assert_eq!(coverage(Language::English, ["New Tab", "Nope"]), (0, 2));
        assert_eq!(coverage(Language::German, ["New Tab", "Definitely not in the catalog"]), (1, 2));
    }
}
