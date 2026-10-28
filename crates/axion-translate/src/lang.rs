//! Language codes and names. Codes are lower-case ISO 639-1 (`de`); region and script
//! subtags (`de-AT`) are dropped, since the models don't tell them apart.

use axion_i18n::tr_mark;

/// The languages Axion offers to translate from and to, with their English names (shown
/// translated). All of them have models in both directions from and to English.
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
];

/// The language a tag stands for: `"de-AT"` and `"DE_ch"` become `"de"`; `"no"` is Bokmål.
/// `None` for tags that are empty or malformed.
pub fn normalize(tag: &str) -> Option<String> {
    let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
    if primary.len() < 2 || primary.len() > 3 || !primary.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tags() {
        assert_eq!(normalize("de-AT").as_deref(), Some("de"));
        assert_eq!(normalize(" EN_us ").as_deref(), Some("en"));
        assert_eq!(normalize("no").as_deref(), Some("nb"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("x"), None);
        assert_eq!(normalize("12"), None);
    }

    #[test]
    fn routes_through_english() {
        assert_eq!(route("de", "en"), Some(vec![("de".into(), "en".into())]));
        assert_eq!(route("de", "fr"), Some(vec![("de".into(), "en".into()), ("en".into(), "fr".into())]));
        assert_eq!(route("de", "de"), None);
        assert_eq!(route("de", "xx"), None);
    }
}
