//! Applies `general.language` to the translation catalog.

use axion_config::UiLanguage;
use axion_i18n::Language;

/// The catalog language a setting stands for (`System` follows the environment).
pub fn resolve(setting: UiLanguage) -> Language {
    match setting {
        UiLanguage::System => Language::from_environment(),
        UiLanguage::English => Language::English,
        UiLanguage::German => Language::German,
    }
}

/// Makes the interface use the language of `setting`.
pub fn apply(setting: UiLanguage) {
    axion_i18n::set_language(resolve(setting));
}
