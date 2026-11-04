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
}
