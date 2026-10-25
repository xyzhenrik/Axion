//! Settings search: every setting with the words people might type for it (aliases, also
//! German ones), matched word by word.

use super::{Category, Setting};

/// What a search result shows: the setting's own control, or a row that opens its page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Control(Setting),
    /// A setting edited with a richer control (field, segments, list) on its page.
    Page(&'static str),
}

pub struct Entry {
    pub category: Category,
    pub target: Target,
    pub aliases: &'static [&'static str],
}

impl Entry {
    pub fn title(&self) -> &'static str {
        match self.target {
            Target::Control(setting) => setting.label(),
            Target::Page(title) => title,
        }
    }
}

const fn control(category: Category, setting: Setting, aliases: &'static [&'static str]) -> Entry {
    Entry { category, target: Target::Control(setting), aliases }
}

const fn page(category: Category, title: &'static str, aliases: &'static [&'static str]) -> Entry {
    Entry { category, target: Target::Page(title), aliases }
}

use Category as C;
use Setting as S;

pub const INDEX: &[Entry] = &[
    page(C::General, "Homepage", &["start page", "new tab", "startseite", "home"]),
    page(C::General, "Search engine", &["google", "duckduckgo", "bing", "suchmaschine", "default search"]),
    page(
        C::Appearance,
        "Appearance",
        &["dark mode", "light mode", "theme", "night", "dunkel", "hell", "design"],
    ),
    page(C::Appearance, "Accent colour", &["color", "accent", "highlight", "akzentfarbe", "farbe", "tint"]),
    control(C::Appearance, S::AdaptToPage, &["dark", "light", "page colour", "adapt", "anpassen"]),
    control(
        C::Appearance,
        S::TextScale,
        &["font", "zoom", "bigger text", "schrift", "schriftgröße", "text size"],
    ),
    control(C::Appearance, S::PageCorner, &["rounded", "radius", "corners", "ecken", "abrunden"]),
    control(C::LiquidGlass, S::ReduceTransparency, &["accessibility", "opaque", "transparenz", "glass off"]),
    control(C::LiquidGlass, S::ReduceMotion, &["animation", "accessibility", "bewegung", "springs"]),
    control(C::LiquidGlass, S::Refraction, &["distortion", "lens", "brechung", "liquid glass"]),
    control(C::LiquidGlass, S::Blur, &["frosted", "unschärfe", "weichzeichnen", "milky"]),
    control(C::LiquidGlass, S::Dispersion, &["rainbow", "chromatic", "prism", "farbsaum"]),
    control(C::LiquidGlass, S::Squash, &["stretch", "jelly", "wobble", "physics", "physik", "dehnen"]),
    control(C::LiquidGlass, S::MergeDistance, &["blob", "connect", "drops", "verschmelzen", "goo"]),
    page(
        C::Keyboard,
        "Keyboard shortcuts",
        &["keys", "hotkeys", "keymap", "tastatur", "tastenkürzel", "bindings"],
    ),
    page(C::Tabs, "Tab bar layout", &["compact", "separate", "tab strip", "tableiste", "kompakt"]),
    control(C::Tabs, S::TintFromPage, &["color", "page colour", "tint", "farbe"]),
    control(C::Tabs, S::HideBarOnScroll, &["fullscreen", "autohide", "ausblenden", "scroll"]),
    control(C::Tabs, S::OpenInBackground, &["new tab", "links", "hintergrund", "middle click"]),
    page(C::Tabs, "Close tabs", &["auto close", "old tabs", "cleanup", "schließen", "aufräumen"]),
    control(C::Tabs, S::FreezeAfter, &["sleep", "suspend", "pause", "einfrieren", "battery", "cpu"]),
    control(C::Tabs, S::DiscardAfter, &["unload", "sleep", "memory", "entladen", "ram"]),
    control(C::Tabs, S::LimitMemory, &["ram", "memory saver", "speicher", "arbeitsspeicher"]),
    control(C::Tabs, S::MemoryBudget, &["ram", "memory", "gb", "speicher", "arbeitsspeicher", "limit"]),
    control(C::Tabs, S::KeepMediaAwake, &["music", "audio", "video", "musik", "playing"]),
    control(C::Tabs, S::RestoreSession, &["startup", "reopen", "session", "wiederherstellen", "sitzung"]),
    control(C::Media, S::OwnFullscreenControls, &["video", "player", "fullscreen", "vollbild"]),
    control(C::Media, S::HideControlsAfter, &["video", "player", "overlay", "timeout", "steuerung"]),
    page(
        C::Media,
        "Sites that keep their own player",
        &["youtube", "twitch", "trusted", "exceptions", "ausnahmen"],
    ),
    control(C::Media, S::HardwareDecoding, &["gpu", "vaapi", "acceleration", "hardware", "beschleunigung"]),
    control(C::Media, S::DesktopIntegration, &["mpris", "media keys", "medientasten", "lock screen", "kde"]),
    page(
        C::Shields,
        "Shields on sites you haven't set",
        &["adblock", "ad blocker", "werbung", "tracker", "default"],
    ),
    control(C::Shields, S::HideAdElements, &["cosmetic", "adblock", "werbung", "ads"]),
    control(C::Shields, S::BlockPopups, &["popup", "pop-up", "windows", "fenster", "werbung"]),
    control(C::Shields, S::BlockRedirects, &["redirect", "weiterleitung", "hijack"]),
    control(C::Shields, S::StripTracking, &["utm", "fbclid", "tracking", "privacy", "links"]),
    control(C::Shields, S::ThirdPartyCookies, &["cookies", "tracking", "privacy", "datenschutz"]),
    page(C::Shields, "Filter lists", &["easylist", "update", "lists", "listen", "adblock"]),
    page(C::Privacy, "Private tabs password", &["password", "passwort", "lock", "incognito", "privat"]),
    control(C::Privacy, S::LockPrivateTabs, &["password", "passwort", "lock", "incognito", "sperren"]),
    control(C::Privacy, S::LockAfter, &["timeout", "auto lock", "sperren", "time"]),
    page(C::Privacy, "Browsing history", &["clear", "delete", "verlauf", "löschen", "history", "data"]),
    page(
        C::Data,
        "Download location",
        &["downloads", "folder", "directory", "save", "speicherort", "ordner", "herunterladen"],
    ),
    control(C::Data, S::AskWhereToSave, &["downloads", "save as", "dialog", "speichern unter", "fragen"]),
    control(C::Data, S::OpenSafeFiles, &["downloads", "auto open", "pdf", "öffnen", "automatisch"]),
    page(
        C::Data,
        "Remove download list items",
        &["downloads", "list", "clear", "cleanup", "liste", "entfernen", "aufräumen"],
    ),
    page(
        C::Data,
        "Clear browsing data",
        &["delete", "cache", "cookies", "history", "löschen", "daten", "browserdaten", "verlauf"],
    ),
    control(C::Data, S::ClearHistoryItem, &["clear", "verlauf", "history", "löschen"]),
    control(C::Data, S::ClearCookiesItem, &["clear", "cookies", "site data", "websitedaten", "löschen"]),
    control(C::Data, S::ClearCacheItem, &["clear", "cache", "zwischenspeicher", "speicherplatz", "löschen"]),
    control(C::Data, S::ClearDownloadsItem, &["clear", "downloads", "liste", "löschen"]),
    control(
        C::Data,
        S::ClearCookiesOnQuit,
        &["cookies", "quit", "exit", "beenden", "privacy", "datenschutz"],
    ),
    page(
        C::Data,
        "Website data",
        &["cookies", "site data", "storage", "websitedaten", "websites", "manage", "verwalten"],
    ),
    page(
        C::Passwords,
        "Saved passwords, cards and addresses",
        &[
            "passwords",
            "passwörter",
            "logins",
            "anmeldedaten",
            "cards",
            "credit card",
            "kreditkarte",
            "karten",
            "addresses",
            "adresse",
            "adressen",
            "notes",
            "notizen",
            "vault",
            "tresor",
            "master password",
            "masterpasswort",
            "hauptpasswort",
            "password manager",
            "passwortmanager",
        ],
    ),
    control(
        C::Passwords,
        S::OfferToSavePasswords,
        &["save password", "passwort speichern", "remember", "merken", "login"],
    ),
    control(
        C::Passwords,
        S::AutofillPasswords,
        &["autofill", "auto fill", "ausfüllen", "automatisch ausfüllen", "fill", "login"],
    ),
    control(
        C::Passwords,
        S::PasswordsLockAfter,
        &["auto lock", "timeout", "sperren", "master password", "passwörter sperren"],
    ),
    control(
        C::Passwords,
        S::ClearClipboardAfter,
        &["clipboard", "zwischenablage", "copy", "kopieren", "leeren"],
    ),
    page(
        C::Passwords,
        "Import and export",
        &[
            "import",
            "export",
            "csv",
            "chrome",
            "firefox",
            "bitwarden",
            "importieren",
            "exportieren",
            "backup",
        ],
    ),
];

/// Lower-case, with German umlauts also findable as ae/oe/ue/ss.
fn normalize(text: &str) -> String {
    text.to_lowercase().replace('ä', "ae").replace('ö', "oe").replace('ü', "ue").replace('ß', "ss")
}

/// How well `entry` matches every word of the query (`None`: it doesn't), and whether some
/// word matched only the page's name.
fn score(entry: &Entry, words: &[String]) -> Option<(u32, bool)> {
    let title = normalize(entry.title());
    let category = normalize(entry.category.title());
    let aliases: Vec<String> = entry.aliases.iter().map(|a| normalize(a)).collect();
    let mut total = 0;
    let mut by_page = false;
    for word in words {
        let starts_word =
            |text: &str| text.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(word));
        total += if starts_word(&title) {
            4
        } else if aliases.iter().any(|a| starts_word(a)) {
            3
        } else if title.contains(word.as_str()) || aliases.iter().any(|a| a.contains(word.as_str())) {
            2
        } else if category.starts_with(word.as_str()) {
            // The page's name ("shields", "tabs") lists the whole page; its later words
            // ("memory" in "Tabs & Memory") don't.
            by_page = true;
            1
        } else {
            return None;
        };
    }
    Some((total, by_page))
}

/// The entries matching `query`, best first (ties keep the pages' order).
pub fn search(query: &str) -> Vec<&'static Entry> {
    let words: Vec<String> = normalize(query).split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u32, bool, usize)> =
        INDEX.iter().enumerate().filter_map(|(i, e)| score(e, &words).map(|(s, p)| (s, p, i))).collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0).then(INDEX[a.2].title().len().cmp(&INDEX[b.2].title().len())).then(a.2.cmp(&b.2))
    });
    hits.into_iter().map(|(_, _, i)| &INDEX[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(query: &str) -> Vec<&'static str> {
        search(query).iter().map(|e| e.title()).collect()
    }

    #[test]
    fn finds_by_title_alias_and_german_word() {
        assert_eq!(titles("memory budget").first(), Some(&"Memory budget"));
        assert!(titles("dark").contains(&"Appearance"));
        assert!(titles("verlauf").contains(&"Browsing history"));
        assert!(titles("schriftgroesse").contains(&"Text size"));
        assert!(titles("youtube").contains(&"Sites that keep their own player"));
        assert!(titles("cache").contains(&"Cached images and files"));
        assert!(titles("speicherort").contains(&"Download location"));
        assert!(titles("kreditkarte").contains(&"Saved passwords, cards and addresses"));
        assert!(titles("ausfüllen").contains(&"Offer to fill saved passwords"));
    }

    #[test]
    fn every_word_must_match() {
        assert!(titles("cookies zzz").is_empty());
        assert!(titles("").is_empty());
    }

    #[test]
    fn page_name_alone_does_not_list_the_whole_page() {
        assert!(!titles("memory").contains(&"Colour in tab bar"));
        assert!(titles("shields").contains(&"Block third-party cookies"));
    }

    #[test]
    fn title_hits_rank_before_alias_hits() {
        let hits = titles("blur");
        assert_eq!(hits.first(), Some(&"Blur"));
    }
}
