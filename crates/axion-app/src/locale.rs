//! Applies `general.language` to the translation catalog.

use axion_config::UiLanguage;
use axion_i18n::Language;

/// The catalog language a setting stands for (`System` follows the environment).
pub fn resolve(setting: UiLanguage) -> Language {
    match concrete(setting) {
        Some(language) => language,
        None => Language::from_environment(),
    }
}

/// The language a setting names, `None` for `System`.
pub fn concrete(setting: UiLanguage) -> Option<Language> {
    Some(match setting {
        UiLanguage::System => return None,
        UiLanguage::English => Language::English,
        UiLanguage::German => Language::German,
        UiLanguage::Spanish => Language::Spanish,
        UiLanguage::French => Language::French,
        UiLanguage::Turkish => Language::Turkish,
        UiLanguage::Japanese => Language::Japanese,
        UiLanguage::PortugueseBrazil => Language::PortugueseBrazil,
        UiLanguage::Italian => Language::Italian,
        UiLanguage::Russian => Language::Russian,
        UiLanguage::ChineseSimplified => Language::ChineseSimplified,
        UiLanguage::Korean => Language::Korean,
        UiLanguage::Dutch => Language::Dutch,
        UiLanguage::Polish => Language::Polish,
    })
}

/// The setting that names `language`.
pub fn setting_for(language: Language) -> UiLanguage {
    UiLanguage::ALL.into_iter().find(|s| concrete(*s) == Some(language)).unwrap_or(UiLanguage::English)
}

/// How the language picker shows a choice: every language in its own name, so it can be found
/// whatever the interface currently says. `System` has no name of its own (the caller translates
/// "System default").
pub fn native_name(setting: UiLanguage) -> Option<&'static str> {
    concrete(setting).map(Language::native_name)
}

/// Makes the interface use the language of `setting`.
pub fn apply(setting: UiLanguage) {
    axion_i18n::set_language(resolve(setting));
}

/// The language `setting` stands for when it differs from `running` (the one this process
/// started in), i.e. a change that waits for a restart; `None` when nothing is pending.
pub fn pending(running: Language, setting: UiLanguage) -> Option<Language> {
    let wanted = resolve(setting);
    (wanted != running).then_some(wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_only_when_the_language_differs() {
        assert_eq!(pending(Language::English, UiLanguage::English), None);
        assert_eq!(pending(Language::English, UiLanguage::German), Some(Language::German));
        assert_eq!(pending(Language::German, UiLanguage::English), Some(Language::English));
        // Switching back to the running language cancels the pending change.
        assert_eq!(pending(Language::German, UiLanguage::German), None);
    }

    #[test]
    fn every_catalog_language_has_a_setting_and_back() {
        for language in Language::all() {
            let setting = setting_for(language);
            assert_eq!(concrete(setting), Some(language), "{}", language.tag());
        }
        assert_eq!(concrete(UiLanguage::System), None);
        assert_eq!(UiLanguage::ALL.len(), Language::all().count() + 1);
    }

    #[test]
    fn names_are_native() {
        assert_eq!(native_name(UiLanguage::Japanese), Some("日本語"));
        assert_eq!(native_name(UiLanguage::ChineseSimplified), Some("简体中文"));
        assert_eq!(native_name(UiLanguage::PortugueseBrazil), Some("Português (Brasil)"));
        assert_eq!(native_name(UiLanguage::System), None);
    }
}
