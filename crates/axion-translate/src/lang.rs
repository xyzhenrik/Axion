//! Language codes and names. Codes are lower-case ISO 639-1 (`de`); region and script
//! subtags (`de-AT`) are dropped, since the models don't tell them apart. The one exception is
//! Chinese: `zh` is Simplified (Mozilla's `zh-Hans`) and `zht` Traditional (`zh-Hant`), written
//! without a hyphen because the engine page and the model store name a pair `from-to`.

use axion_i18n::tr_mark;

/// The languages Axion offers to translate from and to, with their English names (shown
/// translated). Every one has a complete model for **both** directions from and to English in
/// Mozilla's `translations-models` list (checked against a snapshot of it, see the tests; pairs
/// between two other languages pivot through English). A language Mozilla does not list is not
/// here, so the chooser never offers something that cannot work.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", tr_mark("English")),
    ("de", tr_mark("German")),
    ("fr", tr_mark("French")),
    ("es", tr_mark("Spanish")),
    ("it", tr_mark("Italian")),
    ("pt", tr_mark("Portuguese")),
    ("nl", tr_mark("Dutch")),
    ("pl", tr_mark("Polish")),
    ("ru", tr_mark("Russian")),
    ("uk", tr_mark("Ukrainian")),
    ("cs", tr_mark("Czech")),
    ("sv", tr_mark("Swedish")),
    ("da", tr_mark("Danish")),
    ("nb", tr_mark("Norwegian")),
    ("fi", tr_mark("Finnish")),
    ("hu", tr_mark("Hungarian")),
    ("ro", tr_mark("Romanian")),
    ("bg", tr_mark("Bulgarian")),
    ("el", tr_mark("Greek")),
    ("tr", tr_mark("Turkish")),
    ("ar", tr_mark("Arabic")),
    ("ja", tr_mark("Japanese")),
    ("ko", tr_mark("Korean")),
    ("zh", tr_mark("Chinese (Simplified)")),
    ("zht", tr_mark("Chinese (Traditional)")),
    ("af", tr_mark("Afrikaans")),
    ("az", tr_mark("Azerbaijani")),
    ("be", tr_mark("Belarusian")),
    ("bn", tr_mark("Bengali")),
    ("bs", tr_mark("Bosnian")),
    ("ca", tr_mark("Catalan")),
    ("et", tr_mark("Estonian")),
    ("eu", tr_mark("Basque")),
    ("fa", tr_mark("Persian")),
    ("gl", tr_mark("Galician")),
    ("gu", tr_mark("Gujarati")),
    ("he", tr_mark("Hebrew")),
    ("hi", tr_mark("Hindi")),
    ("hr", tr_mark("Croatian")),
    ("id", tr_mark("Indonesian")),
    ("is", tr_mark("Icelandic")),
    ("kn", tr_mark("Kannada")),
    ("lt", tr_mark("Lithuanian")),
    ("lv", tr_mark("Latvian")),
    ("ml", tr_mark("Malayalam")),
    ("mr", tr_mark("Marathi")),
    ("ms", tr_mark("Malay")),
    ("sk", tr_mark("Slovak")),
    ("sl", tr_mark("Slovenian")),
    ("sq", tr_mark("Albanian")),
    ("sr", tr_mark("Serbian")),
    ("ta", tr_mark("Tamil")),
    ("te", tr_mark("Telugu")),
    ("th", tr_mark("Thai")),
    ("tl", tr_mark("Tagalog")),
    ("ug", tr_mark("Uyghur")),
    ("ur", tr_mark("Urdu")),
    ("vi", tr_mark("Vietnamese")),
];

/// The code Mozilla's model list uses for `code` (`zh` is `zh-Hans`, `zht` is `zh-Hant`).
pub fn model_code(code: &str) -> &str {
    match code {
        "zh" => "zh-Hans",
        "zht" => "zh-Hant",
        other => other,
    }
}

/// The code a LibreTranslate server uses for `code` (it calls Traditional Chinese `zt`).
pub fn endpoint_code(code: &str) -> &str {
    match code {
        "zht" => "zt",
        other => other,
    }
}

/// The language a tag stands for: `"de-AT"` and `"DE_ch"` become `"de"`; `"no"` is Bokmål;
/// `"zh-TW"`, `"zh-Hant"`, `"zh-HK"` and `"zh-MO"` are Traditional Chinese (`"zht"`), every
/// other `zh` tag Simplified. `None` for tags that are empty or malformed.
pub fn normalize(tag: &str) -> Option<String> {
    let mut parts = tag.trim().split(['-', '_']);
    let primary = parts.next()?.to_ascii_lowercase();
    if primary.len() < 2 || primary.len() > 3 || !primary.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    if primary == "zh" {
        let traditional =
            parts.any(|p| matches!(p.to_ascii_lowercase().as_str(), "hant" | "tw" | "hk" | "mo"));
        return Some(if traditional { "zht" } else { "zh" }.to_owned());
    }
    Some(if primary == "no" { "nb".to_owned() } else { primary })
}

/// The English name of a supported language (`None` for any other code).
pub fn english_name(code: &str) -> Option<&'static str> {
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, name)| *name)
}

/// Whether the local models cover `code`.
pub fn is_supported(code: &str) -> bool {
    english_name(code).is_some()
}

/// The models to chain for `from` → `to`: one pair when English is an end, else a pivot through
/// English. `None` when it is the same language or one isn't supported.
pub fn route(from: &str, to: &str) -> Option<Vec<(String, String)>> {
    if from == to || !is_supported(from) || !is_supported(to) {
        return None;
    }
    Some(if from == "en" || to == "en" {
        vec![(from.to_owned(), to.to_owned())]
    } else {
        vec![(from.to_owned(), "en".to_owned()), ("en".to_owned(), to.to_owned())]
    })
}

/// The languages to offer as translation targets, for the settings chooser: `(code, English
/// name)` of every supported language, in list order. All of them work with local models.
pub fn targets() -> impl Iterator<Item = (&'static str, &'static str)> {
    LANGUAGES.iter().copied()
}

/// The translation language that stands for an interface language: its code when the local
/// models cover it (`None` otherwise, so the offer is not made for a language nothing can
/// translate into).
pub fn for_interface(language: axion_i18n::Language) -> Option<&'static str> {
    let code = language.translation_code();
    LANGUAGES.iter().map(|(c, _)| *c).find(|c| *c == code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TranslateError;
    use crate::models::pair_files;

    /// Mozilla's `translations-models` list as of 2026-11-04: per language pair and file type,
    /// the newest released record (alpha/beta versions left out, like the code does), with the
    /// attachment trimmed to dummies.
    const SNAPSHOT: &str = include_str!("../tests/fixtures/models-snapshot.json");

    #[test]
    fn normalizes_tags() {
        assert_eq!(normalize("de-AT").as_deref(), Some("de"));
        assert_eq!(normalize(" EN_us ").as_deref(), Some("en"));
        assert_eq!(normalize("no").as_deref(), Some("nb"));
        assert_eq!(normalize("pt-BR").as_deref(), Some("pt"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("x"), None);
        assert_eq!(normalize("12"), None);
    }

    #[test]
    fn chinese_scripts_are_told_apart() {
        assert_eq!(normalize("zh").as_deref(), Some("zh"));
        assert_eq!(normalize("zh-CN").as_deref(), Some("zh"));
        assert_eq!(normalize("zh-Hans-SG").as_deref(), Some("zh"));
        assert_eq!(normalize("zh_TW").as_deref(), Some("zht"));
        assert_eq!(normalize("zh-HK").as_deref(), Some("zht"));
        assert_eq!(normalize("ZH-hant").as_deref(), Some("zht"));
        assert_eq!(model_code("zh"), "zh-Hans");
        assert_eq!(model_code("zht"), "zh-Hant");
        assert_eq!(model_code("ja"), "ja");
        assert_eq!(endpoint_code("zht"), "zt");
        assert_eq!(endpoint_code("zh"), "zh");
    }

    #[test]
    fn routes_through_english() {
        assert_eq!(route("de", "en"), Some(vec![("de".into(), "en".into())]));
        assert_eq!(route("en", "ja"), Some(vec![("en".into(), "ja".into())]));
        assert_eq!(route("de", "fr"), Some(vec![("de".into(), "en".into()), ("en".into(), "fr".into())]));
        assert_eq!(route("ja", "zh"), Some(vec![("ja".into(), "en".into()), ("en".into(), "zh".into())]));
        assert_eq!(route("de", "de"), None);
        assert_eq!(route("de", "xx"), None);
        assert_eq!(route("xx", "en"), None);
    }

    #[test]
    fn list_has_no_duplicates_and_english_first() {
        let mut seen = std::collections::BTreeSet::new();
        for (code, name) in LANGUAGES {
            assert!(seen.insert(*code), "duplicate code {code}");
            assert!(!name.is_empty());
            assert_eq!(normalize(code).as_deref(), Some(*code), "{code} is not in normal form");
        }
        assert_eq!(LANGUAGES[0].0, "en");
        assert_eq!(targets().count(), LANGUAGES.len());
        // Codes become `from-to` directory and engine keys: no hyphen allowed.
        assert!(LANGUAGES.iter().all(|(c, _)| !c.contains('-')));
    }

    /// Every language we list has a complete model both from and to English in the snapshot of
    /// Mozilla's list; a pivot pair is two such pairs.
    #[test]
    fn every_listed_language_has_models_both_ways() {
        for (code, _) in LANGUAGES.iter().filter(|(c, _)| *c != "en") {
            let model = model_code(code);
            for (from, to) in [("en", model), (model, "en")] {
                match pair_files(SNAPSHOT, from, to) {
                    Ok(files) => assert!(!files.is_empty()),
                    Err(e) => panic!("{code}: no model {from} -> {to} in Mozilla's list: {e}"),
                }
            }
        }
    }

    /// The reverse: the languages Mozilla offers that the list leaves out (none expected; when
    /// Mozilla adds some, this tells which to add).
    #[test]
    fn nothing_offered_by_mozilla_is_left_out() {
        let value: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
        let mut offered = std::collections::BTreeSet::new();
        for record in value["data"].as_array().unwrap() {
            for key in ["fromLang", "toLang"] {
                if let Some(code) = record[key].as_str() {
                    offered.insert(code.to_owned());
                }
            }
        }
        let listed: std::collections::BTreeSet<String> =
            LANGUAGES.iter().map(|(c, _)| model_code(c).to_owned()).collect();
        let missing: Vec<_> = offered.difference(&listed).collect();
        assert!(missing.is_empty(), "Mozilla offers languages the list lacks: {missing:?}");
    }

    #[test]
    fn languages_the_list_does_not_have_are_rejected() {
        // A pair Mozilla does not list is a clean error, not a panic.
        assert!(matches!(pair_files(SNAPSHOT, "en", "xx"), Err(TranslateError::NoModel { .. })));
        assert!(!is_supported("xx"));
        assert_eq!(english_name("xx"), None);
        assert_eq!(english_name("ja"), Some("Japanese"));
    }

    /// Every interface language of Axion can also be translated into, so the "Interface" choice
    /// of the chooser always works.
    #[test]
    fn every_interface_language_is_a_translation_target() {
        for language in axion_i18n::Language::all() {
            let code = for_interface(language)
                .unwrap_or_else(|| panic!("{} has no translation models", language.tag()));
            assert!(is_supported(code));
            assert!(route("en", code).is_some() || code == "en");
        }
        assert_eq!(for_interface(axion_i18n::Language::PortugueseBrazil), Some("pt"));
        assert_eq!(for_interface(axion_i18n::Language::ChineseSimplified), Some("zh"));
    }
}
