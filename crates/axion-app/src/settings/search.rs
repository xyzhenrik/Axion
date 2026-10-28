//! Settings search: every setting with the words people might type for it (aliases, also
//! German ones), matched word by word.

use axion_i18n::{tr, tr_mark};

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
    /// The English title (also what search matches, whatever the language).
    pub fn title(&self) -> &'static str {
        match self.target {
            Target::Control(setting) => setting.source_label(),
            Target::Page(title) => title,
        }
    }

    /// The title in the current language.
    pub fn translated_title(&self) -> &'static str {
        tr(self.title())
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
    page(
        C::Data,
        tr_mark("Import bookmarks and history"),
        &[
            "import",
            "migrate",
            "transfer",
            "chrome",
            "chromium",
            "brave",
            "edge",
            "firefox",
            "vivaldi",
            "opera",
            "librewolf",
            "zen",
            "other browser",
            "switch browser",
            "bookmarks file",
            "importieren",
            "lesezeichen",
            "verlauf",
            "browser wechseln",
            "anderer browser",
            "übernehmen",
        ],
    ),
    page(
        C::Account,
        tr_mark("Account"),
        &["accounts", "profile", "profiles", "user", "konto", "konten", "profil", "benutzer", "nutzer"],
    ),
    page(
        C::Account,
        tr_mark("Account name"),
        &["rename", "name", "kontoname", "umbenennen", "name ändern", "profilname", "benutzername"],
    ),
    page(C::Account, tr_mark("Account colour"), &["avatar", "color", "initials", "farbe", "kontofarbe"]),
    page(
        C::Account,
        tr_mark("Account photo"),
        &["avatar", "profile picture", "photo", "image", "profilbild", "foto", "bild"],
    ),
    page(
        C::Account,
        tr_mark("Account password"),
        &[
            "password",
            "change password",
            "set password",
            "master password",
            "private tabs",
            "passwort",
            "passwort ändern",
            "passwort festlegen",
            "kennwort",
            "hauptpasswort",
            "masterpasswort",
            "kontopasswort",
        ],
    ),
    page(
        C::Account,
        tr_mark("Switch or add accounts"),
        &[
            "switch account",
            "add account",
            "new account",
            "new profile",
            "wechseln",
            "konto wechseln",
            "profil wechseln",
            "benutzer wechseln",
            "konto hinzufügen",
            "neues konto",
            "neues profil",
        ],
    ),
    page(
        C::Account,
        tr_mark("Delete account"),
        &["remove account", "delete profile", "konto löschen", "profil löschen", "entfernen", "löschen"],
    ),
    control(
        C::General,
        S::DevTools,
        &["devtools", "inspect", "inspector", "console", "f12", "source", "debug", "entwickler"],
    ),
    control(C::General, S::HomeButton, &["home", "haus", "startseite", "toolbar", "button"]),
    page(C::General, tr_mark("Homepage"), &["start page", "new tab", "startseite", "home", "neuer tab"]),
    control(C::General, S::HomeClock, &["start page", "time", "uhr", "uhrzeit", "startseite", "24", "12"]),
    control(C::General, S::HomeGreeting, &["start page", "hello", "name", "begrüßung", "gruß", "startseite"]),
    control(C::General, S::HomeSearch, &["start page", "search box", "suchfeld", "suche", "startseite"]),
    control(
        C::General,
        S::HomeTopSites,
        &[
            "start page",
            "most visited",
            "favourites",
            "favorites",
            "tiles",
            "meistbesucht",
            "häufig besucht",
            "top seiten",
            "kacheln",
            "startseite",
        ],
    ),
    control(
        C::General,
        S::HomeStats,
        &[
            "start page",
            "blocked",
            "trackers",
            "ads",
            "bandwidth",
            "time saved",
            "statistik",
            "blockiert",
            "werbung",
            "gespart",
            "startseite",
        ],
    ),
    control(
        C::General,
        S::HomeLive,
        &[
            "start page",
            "wallpaper",
            "animated",
            "gif",
            "video",
            "live",
            "hintergrund",
            "animiert",
            "startseite",
        ],
    ),
    control(
        C::General,
        S::HomeLiveIgnoreReduceMotion,
        &[
            "start page",
            "wallpaper",
            "live",
            "animated",
            "reduce motion",
            "animation",
            "bewegung",
            "bewegung reduzieren",
            "hintergrund",
            "startseite",
        ],
    ),
    page(
        C::General,
        tr_mark("Start page background"),
        &[
            "background image",
            "wallpaper",
            "picture",
            "photo",
            "image",
            "add image",
            "remove image",
            "hintergrund",
            "hintergrundbild",
            "bild",
            "foto",
            "bild hinzufügen",
            "bild entfernen",
            "startseite",
        ],
    ),
    control(
        C::General,
        S::HomeDim,
        &[
            "start page",
            "wallpaper",
            "darken",
            "darker",
            "dimming",
            "abdunkeln",
            "dunkler",
            "hintergrund",
            "hintergrundbild",
            "startseite",
        ],
    ),
    control(
        C::General,
        S::HomeBlur,
        &[
            "start page",
            "wallpaper",
            "blurry",
            "soft",
            "unschärfe",
            "unscharf",
            "weichzeichnen",
            "hintergrund",
            "hintergrundbild",
            "startseite",
        ],
    ),
    page(
        C::General,
        tr_mark("Search engine"),
        &["google", "duckduckgo", "bing", "suchmaschine", "default search"],
    ),
    page(
        C::Appearance,
        tr_mark("Appearance"),
        &["dark mode", "light mode", "theme", "night", "dunkel", "hell", "design"],
    ),
    page(
        C::Appearance,
        tr_mark("Glass"),
        &[
            "flat",
            "no glass",
            "liquid glass",
            "solid",
            "performance",
            "weak gpu",
            "fast",
            "transparency",
            "glas",
            "flach",
            "leistung",
        ],
    ),
    page(
        C::Appearance,
        tr_mark("Accent colour"),
        &["color", "accent", "highlight", "akzentfarbe", "farbe", "tint"],
    ),
    page(
        C::Appearance,
        tr_mark("Icon weight"),
        &["icons", "stroke", "bold", "thick", "thin", "line", "symbole", "strichstärke", "dicke", "fett"],
    ),
    control(C::Appearance, S::AdaptToPage, &["dark", "light", "page colour", "adapt", "anpassen"]),
    control(
        C::Appearance,
        S::TextScale,
        &["font", "zoom", "bigger text", "schrift", "schriftgröße", "text size"],
    ),
    control(C::Appearance, S::FloatSidebar, &["sidebar", "overlay", "floating", "seitenleiste", "schweben"]),
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
        tr_mark("Keyboard shortcuts"),
        &["keys", "hotkeys", "keymap", "tastatur", "tastenkürzel", "bindings"],
    ),
    page(C::Tabs, tr_mark("Tab bar layout"), &["compact", "separate", "tab strip", "tableiste", "kompakt"]),
    control(C::Tabs, S::TintFromPage, &["color", "page colour", "tint", "farbe"]),
    control(C::Tabs, S::OpenInBackground, &["new tab", "links", "hintergrund", "middle click"]),
    control(C::Tabs, S::DragOutLinks, &["drag", "tab", "link", "paste", "ziehen", "text field"]),
    control(
        C::Tabs,
        S::MiddleClickClosesTab,
        &["middle click", "mittlere maustaste", "close tab", "mouse wheel"],
    ),
    page(C::Tabs, tr_mark("Close tabs"), &["auto close", "old tabs", "cleanup", "schließen", "aufräumen"]),
    page(
        C::Tabs,
        tr_mark("Bookmarks bar"),
        &[
            "lesezeichenleiste",
            "favoriten",
            "bookmarks bar",
            "favorites bar",
            "lesezeichen",
            "favoritenleiste",
        ],
    ),
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
        tr_mark("Sites that keep their own player"),
        &["youtube", "twitch", "trusted", "exceptions", "ausnahmen"],
    ),
    control(
        C::Media,
        S::PlaylistQuickAction,
        &[
            "playlist",
            "watch later",
            "save video",
            "button",
            "wiedergabeliste",
            "später ansehen",
            "video merken",
        ],
    ),
    control(
        C::Media,
        S::PlaylistAutoDownload,
        &["playlist", "offline", "download video", "herunterladen", "wiedergabeliste", "offline ansehen"],
    ),
    control(
        C::Media,
        S::PlaylistDeleteFiles,
        &["playlist", "remove", "delete files", "löschen", "dateien löschen", "wiedergabeliste", "entfernen"],
    ),
    control(
        C::Reading,
        S::ReaderButton,
        &["reader", "article", "reading", "lesen", "leseansicht", "artikel"],
    ),
    control(C::Reading, S::ReaderRememberPerSite, &["reader", "site", "remember", "website", "merken"]),
    page(C::Reading, tr_mark("Font"), &["reader", "serif", "sans", "mono", "schrift", "font", "typeface"]),
    page(C::Reading, tr_mark("Theme"), &["reader", "sepia", "dark", "light", "design", "dunkel", "hell"]),
    control(
        C::Reading,
        S::TranslateOffer,
        &["translate", "translation", "language", "übersetzen", "übersetzung", "sprache"],
    ),
    control(C::Reading, S::TranslatePrompt, &["translate", "prompt", "offer", "übersetzen", "angebot"]),
    control(C::Reading, S::TranslateChanges, &["translate", "dynamic", "übersetzen", "später"]),
    page(
        C::Reading,
        tr_mark("Translate pages into"),
        &["translate", "target", "language", "übersetzen", "zielsprache"],
    ),
    page(
        C::Reading,
        tr_mark("Downloaded language data"),
        &["translate", "models", "bergamot", "download", "remove", "sprachdaten", "modelle", "löschen"],
    ),
    page(
        C::Media,
        tr_mark("Playlist sort"),
        &[
            "playlist",
            "sort",
            "order",
            "newest",
            "oldest",
            "sortieren",
            "reihenfolge",
            "neueste",
            "älteste",
            "wiedergabeliste",
        ],
    ),
    page(
        C::Media,
        tr_mark("Playlist downloads"),
        &["playlist", "folder", "directory", "ordner", "wiedergabeliste", "speicherort", "download location"],
    ),
    control(
        C::Media,
        S::HardwareDecoding,
        &["gpu", "vaapi", "acceleration", "hardware", "beschleunigung", "video", "decoding", "dekodierung"],
    ),
    control(
        C::General,
        S::AppMenuEntry,
        &["desktop", "icon", "launcher", "menu", "taskbar", "symbol", "startmenü"],
    ),
    control(C::Media, S::DesktopIntegration, &["mpris", "media keys", "medientasten", "lock screen", "kde"]),
    control(
        C::Hardware,
        S::HardwareEnabled,
        &[
            "gpu",
            "acceleration",
            "hardware",
            "software rendering",
            "hardwarebeschleunigung",
            "beschleunigung",
            "grafik",
            "grafikkarte",
        ],
    ),
    control(
        C::Hardware,
        S::GpuRasterization,
        &["webgl", "canvas", "3d", "raster", "gpu", "zero copy", "rasterisierung", "grafik", "ruckeln"],
    ),
    page(
        C::Hardware,
        tr_mark("Page rendering"),
        &[
            "frames",
            "dmabuf",
            "shared memory",
            "osr",
            "software",
            "gpu",
            "amd",
            "radv",
            "nvidia",
            "crash",
            "seiten",
            "speicher",
            "absturz",
            "darstellung",
        ],
    ),
    page(
        C::Hardware,
        tr_mark("Graphics card"),
        &[
            "gpu",
            "integrated",
            "dedicated",
            "discrete",
            "adapter",
            "power saving",
            "performance",
            "amd",
            "nvidia",
            "intel",
            "grafikkarte",
            "grafik",
            "integriert",
            "dediziert",
            "energiesparen",
            "leistung",
        ],
    ),
    page(
        C::Hardware,
        tr_mark("GPU status"),
        &[
            "diagnostics",
            "copy",
            "vaapi",
            "driver",
            "chrome://gpu",
            "adapter",
            "restart",
            "diagnose",
            "kopieren",
            "treiber",
            "neustart",
        ],
    ),
    page(
        C::Shields,
        tr_mark("Shields on sites you haven't set"),
        &["adblock", "ad blocker", "werbung", "tracker", "default"],
    ),
    control(C::Shields, S::HideAdElements, &["cosmetic", "adblock", "werbung", "ads"]),
    control(C::Shields, S::BlockPopups, &["popup", "pop-up", "windows", "fenster", "werbung"]),
    control(C::Shields, S::BlockRedirects, &["redirect", "weiterleitung", "hijack"]),
    control(C::Shields, S::StripTracking, &["utm", "fbclid", "tracking", "privacy", "links"]),
    control(C::Shields, S::ThirdPartyCookies, &["cookies", "tracking", "privacy", "datenschutz"]),
    page(C::Shields, tr_mark("Filter lists"), &["easylist", "update", "lists", "listen", "adblock"]),
    control(
        C::Privacy,
        S::LockPrivateTabs,
        &["password", "passwort", "lock", "incognito", "privat", "sperren", "account", "konto"],
    ),
    page(
        C::Privacy,
        tr_mark("Browsing history"),
        &["clear", "delete", "verlauf", "löschen", "history", "data"],
    ),
    page(
        C::Privacy,
        tr_mark("Site Permissions"),
        &[
            "permission",
            "berechtigung",
            "microphone",
            "mikrofon",
            "camera",
            "kamera",
            "webcam",
            "screen",
            "bildschirm",
            "share",
            "teilen",
            "freigabe",
            "discord",
            "notification",
            "mitteilung",
            "benachrichtigung",
            "location",
            "standort",
            "clipboard",
            "zwischenablage",
            "allow",
            "erlauben",
            "block",
            "blockieren",
        ],
    ),
    page(
        C::Privacy,
        tr_mark("Remembered for Sites"),
        &["permission", "berechtigung", "reset", "zurücksetzen", "forget", "vergessen", "site", "website"],
    ),
    page(
        C::Data,
        tr_mark("Download location"),
        &["downloads", "folder", "directory", "save", "speicherort", "ordner", "herunterladen"],
    ),
    control(C::Data, S::AskWhereToSave, &["downloads", "save as", "dialog", "speichern unter", "fragen"]),
    control(C::Data, S::OpenSafeFiles, &["downloads", "auto open", "pdf", "öffnen", "automatisch"]),
    page(
        C::Data,
        tr_mark("Remove download list items"),
        &["downloads", "list", "clear", "cleanup", "liste", "entfernen", "aufräumen"],
    ),
    page(
        C::Data,
        tr_mark("Clear browsing data"),
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
        tr_mark("Website data"),
        &["cookies", "site data", "storage", "websitedaten", "websites", "manage", "verwalten"],
    ),
    page(
        C::Passwords,
        tr_mark("Saved passwords, cards and addresses"),
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
        S::ClearClipboardAfter,
        &["clipboard", "zwischenablage", "copy", "kopieren", "leeren"],
    ),
    page(
        C::Passwords,
        tr_mark("Import and export"),
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
            "restore",
            "wiederherstellen",
            "device",
            "gerät",
        ],
    ),
    page(
        C::About,
        tr_mark("About & Updates"),
        &[
            "about",
            "version",
            "update",
            "updates",
            "upgrade",
            "über",
            "info",
            "aktualisieren",
            "aktualisierung",
        ],
    ),
    page(
        C::About,
        tr_mark("Check for Updates"),
        &["update", "new version", "release", "neue version", "aktualisierung", "suchen", "prüfen"],
    ),
    page(
        C::About,
        tr_mark("Repository"),
        &["update source", "git", "github", "owner", "releases", "quelle", "projekt"],
    ),
    control(
        C::About,
        S::AutoCheckUpdates,
        &["update", "automatic", "startup", "automatisch", "aktualisierung", "suchen"],
    ),
    control(
        C::About,
        S::UpdatePrereleases,
        &["update", "beta", "channel", "preview", "vorabversion", "kanal", "testversion"],
    ),
];

/// Lower-case, with German umlauts also findable as ae/oe/ue/ss.
fn normalize(text: &str) -> String {
    text.to_lowercase().replace('ä', "ae").replace('ö', "oe").replace('ü', "ue").replace('ß', "ss")
}

/// How well `entry` matches every word of the query (`None`: it doesn't), and whether some
/// word matched only the page's name.
fn score(entry: &Entry, words: &[String]) -> Option<(u32, bool)> {
    // Both the translated and the English texts match, so "Lesezeichen" and "bookmarks" both
    // work (the language is fixed per run, so this is the index built from the `tr` texts).
    let title = format!("{} {}", normalize(entry.translated_title()), normalize(entry.title()));
    let categories = [normalize(entry.category.title()), normalize(entry.category.source_title())];
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
        } else if categories.iter().any(|c| c.starts_with(word.as_str())) {
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
        assert!(titles("mikrofon").contains(&"Site Permissions"));
        assert!(titles("discord").contains(&"Site Permissions"));
        assert!(titles("schriftgroesse").contains(&"Text size"));
        assert!(titles("youtube").contains(&"Sites that keep their own player"));
        assert!(titles("cache").contains(&"Cached images and files"));
        assert!(titles("speicherort").contains(&"Download location"));
        assert!(titles("kreditkarte").contains(&"Saved passwords, cards and addresses"));
        assert!(titles("ausfüllen").contains(&"Offer to fill saved passwords"));
        assert!(titles("konto").contains(&"Account"));
        assert!(titles("profil wechseln").contains(&"Switch or add accounts"));
        assert!(titles("passwort ändern").contains(&"Account password"));
        assert!(titles("benutzer").contains(&"Account"));
        assert!(titles("konto löschen").contains(&"Delete account"));
        assert!(titles("lesezeichenleiste").contains(&"Bookmarks bar"));
        assert!(titles("favoriten").contains(&"Bookmarks bar"));
        assert!(titles("hintergrundbild").contains(&"Start page background"));
        assert!(titles("abdunkeln").contains(&"Dim background image"));
        assert!(titles("hintergrundbild unschärfe").contains(&"Blur background image"));
        assert!(titles("meistbesucht").contains(&"Top sites"));
        assert!(titles("uhrzeit").contains(&"Clock"));
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
