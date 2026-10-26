//! Keyboard shortcuts.
//!
//! `keymap.toml` maps action names to lists of chords, e.g.
//!
//! ```toml
//! focus_address_bar = ["Alt+D", "Ctrl+K"]
//! toggle_sidebar = ["Ctrl+Shift+L"]
//! tab_overview = []          # unbind
//! ```
//!
//! An action listed there replaces its default bindings; actions not listed keep theirs,
//! except that a chord the user bound explicitly is taken away from the defaults of other
//! actions (so an old `keymap.toml` never collides with newer defaults).
//!
//! Default scheme: everything about tabs is `Alt+key` (or an F key); browser and system
//! things (address bar, reload, sidebar, bookmarks, history, passwords, find, zoom) are
//! `Ctrl+key`. `Alt+Left/Right` stay back/forward.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::str::FromStr;

use crate::ConfigError;

/// Something a shortcut can trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    FocusAddressBar,
    NewTab,
    CloseTab,
    ReopenClosedTab,
    NextTab,
    PreviousTab,
    /// Select the n-th tab (1-based, 1–8).
    SelectTab(u8),
    SelectLastTab,
    Reload,
    HardReload,
    Back,
    Forward,
    ToggleSidebar,
    TabOverview,
    /// Pin or unpin the current tab.
    PinTab,
    /// Bookmark the current page (or remove its bookmark).
    BookmarkPage,
    ShowBookmarks,
    ShowHistory,
    /// Open the Axion Playlist page.
    OpenPlaylist,
    /// Add the page's video to the Playlist.
    PlaylistAdd,
    OpenPasswords,
    /// Locks private tabs and password filling (the account menu's Lock).
    Lock,
    Find,
    FindNext,
    FindPrevious,
    /// Paste into the page (only where Axion hands the clipboard to pages itself, i.e. on
    /// Wayland; elsewhere the page handles its own Ctrl+V).
    Paste,
    /// Paste only the text, without formatting or images.
    PastePlainText,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    OpenSettings,
    ToggleFullscreen,
    /// Chromium's developer tools for the page (toggles).
    DevTools,
    /// The developer tools, on the Console panel.
    DevToolsConsole,
    /// The page's source in a new tab (`view-source:`).
    ViewSource,
    Quit,
    // Media keys: only while a video plays fullscreen with Axion's controls, so plain keys
    // like Space keep reaching the page otherwise.
    MediaPlayPause,
    MediaSeekBack,
    MediaSeekForward,
    MediaVolumeUp,
    MediaVolumeDown,
    MediaMute,
    MediaFullscreen,
    MediaCaptions,
    MediaSlower,
    MediaFaster,
}

impl Action {
    /// Media keys apply only while a video is fullscreen (see the enum).
    pub fn is_media(self) -> bool {
        matches!(
            self,
            Action::MediaPlayPause
                | Action::MediaSeekBack
                | Action::MediaSeekForward
                | Action::MediaVolumeUp
                | Action::MediaVolumeDown
                | Action::MediaMute
                | Action::MediaFullscreen
                | Action::MediaCaptions
                | Action::MediaSlower
                | Action::MediaFaster
        )
    }
}

const SIMPLE_ACTIONS: &[(Action, &str)] = &[
    (Action::FocusAddressBar, "focus_address_bar"),
    (Action::NewTab, "new_tab"),
    (Action::CloseTab, "close_tab"),
    (Action::ReopenClosedTab, "reopen_closed_tab"),
    (Action::NextTab, "next_tab"),
    (Action::PreviousTab, "previous_tab"),
    (Action::SelectLastTab, "select_last_tab"),
    (Action::Reload, "reload"),
    (Action::HardReload, "hard_reload"),
    (Action::Back, "back"),
    (Action::Forward, "forward"),
    (Action::ToggleSidebar, "toggle_sidebar"),
    (Action::TabOverview, "tab_overview"),
    (Action::PinTab, "pin_tab"),
    (Action::BookmarkPage, "bookmark_page"),
    (Action::ShowBookmarks, "show_bookmarks"),
    (Action::ShowHistory, "show_history"),
    (Action::OpenPlaylist, "open_playlist"),
    (Action::PlaylistAdd, "playlist_add"),
    (Action::OpenPasswords, "open_passwords"),
    (Action::Lock, "lock"),
    (Action::Find, "find"),
    (Action::FindNext, "find_next"),
    (Action::FindPrevious, "find_previous"),
    (Action::Paste, "paste"),
    (Action::PastePlainText, "paste_plain_text"),
    (Action::ZoomIn, "zoom_in"),
    (Action::ZoomOut, "zoom_out"),
    (Action::ZoomReset, "zoom_reset"),
    (Action::OpenSettings, "open_settings"),
    (Action::ToggleFullscreen, "toggle_fullscreen"),
    (Action::DevTools, "devtools"),
    (Action::DevToolsConsole, "devtools_console"),
    (Action::ViewSource, "view_source"),
    (Action::Quit, "quit"),
    (Action::MediaPlayPause, "media_play_pause"),
    (Action::MediaSeekBack, "media_seek_back"),
    (Action::MediaSeekForward, "media_seek_forward"),
    (Action::MediaVolumeUp, "media_volume_up"),
    (Action::MediaVolumeDown, "media_volume_down"),
    (Action::MediaMute, "media_mute"),
    (Action::MediaFullscreen, "media_fullscreen"),
    (Action::MediaCaptions, "media_captions"),
    (Action::MediaSlower, "media_slower"),
    (Action::MediaFaster, "media_faster"),
];
const SELECT_TAB_PREFIX: &str = "select_tab_";

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Action::SelectTab(n) = self {
            return write!(f, "{SELECT_TAB_PREFIX}{n}");
        }
        let name = SIMPLE_ACTIONS.iter().find(|(a, _)| a == self).map_or("?", |(_, n)| n);
        f.write_str(name)
    }
}

impl FromStr for Action {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(n) = s.strip_prefix(SELECT_TAB_PREFIX).and_then(|n| n.parse::<u8>().ok())
            && (1..=8).contains(&n)
        {
            return Ok(Action::SelectTab(n));
        }
        SIMPLE_ACTIONS
            .iter()
            .find(|(_, name)| *name == s)
            .map(|(a, _)| *a)
            .ok_or_else(|| ConfigError::UnknownAction(s.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// The Super / Windows / Command key.
    pub logo: bool,
}

/// A key independent of modifiers: a character (lower-case) or a named key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Char(char),
    /// Canonical names: `Left`, `Right`, `Up`, `Down`, `Tab`, `Enter`, `Escape`, `Space`,
    /// `Backspace`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `F1`…`F24`.
    Named(String),
}

/// A key plus modifiers, written like `Ctrl+Shift+L`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub modifiers: Modifiers,
    pub key: Key,
}

const NAMED_KEYS: &[&str] = &[
    "Left",
    "Right",
    "Up",
    "Down",
    "Tab",
    "Enter",
    "Escape",
    "Space",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];

impl Key {
    /// Canonicalizes a key name; accepts common aliases (`Esc`, `Return`, `ArrowLeft`, `Plus`…).
    pub fn parse(name: &str) -> Option<Key> {
        let mut chars = name.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Some(Key::Char(c.to_ascii_lowercase()));
        }
        let lower = name.to_ascii_lowercase();
        let alias = match lower.as_str() {
            "esc" => "Escape",
            "return" => "Enter",
            "arrowleft" => "Left",
            "arrowright" => "Right",
            "arrowup" => "Up",
            "arrowdown" => "Down",
            "del" => "Delete",
            "plus" => return Some(Key::Char('+')),
            "minus" => return Some(Key::Char('-')),
            "comma" => return Some(Key::Char(',')),
            "backslash" => return Some(Key::Char('\\')),
            _ => "",
        };
        if !alias.is_empty() {
            return Some(Key::Named(alias.into()));
        }
        if let Some(named) = NAMED_KEYS.iter().find(|k| k.eq_ignore_ascii_case(name)) {
            return Some(Key::Named((*named).into()));
        }
        let n = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())?;
        (1..=24).contains(&n).then(|| Key::Named(format!("F{n}")))
    }
}

impl FromStr for Chord {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason: &str| ConfigError::Chord { chord: s.to_owned(), reason: reason.to_owned() };
        // A trailing "+" is the plus key itself ("Ctrl++").
        let (head, key_name) = match s.strip_suffix("++") {
            Some(head) => (head, "+"),
            None => s.rsplit_once('+').unwrap_or(("", s)),
        };
        let mut modifiers = Modifiers::default();
        for part in head.split('+').filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "shift" => modifiers.shift = true,
                "alt" => modifiers.alt = true,
                "super" | "meta" | "logo" | "cmd" | "win" => modifiers.logo = true,
                other => return Err(err(&format!("unknown modifier {other:?}"))),
            }
        }
        let key = Key::parse(key_name.trim()).ok_or_else(|| err("unknown key"))?;
        Ok(Chord { modifiers, key })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [(m.ctrl, "Ctrl+"), (m.alt, "Alt+"), (m.shift, "Shift+"), (m.logo, "Super+")] {
            if on {
                f.write_str(name)?;
            }
        }
        match &self.key {
            Key::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
            Key::Named(n) => f.write_str(n),
        }
    }
}

/// Two actions bound to the same chord; the later one (in action order) is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeymapConflict {
    pub chord: Chord,
    pub kept: Action,
    pub ignored: Action,
}

/// The effective shortcuts: defaults merged with the user's `keymap.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    bindings: BTreeMap<Action, Vec<Chord>>,
    lookup: HashMap<Chord, Action>,
    conflicts: Vec<KeymapConflict>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::from_bindings(default_bindings())
    }
}

impl Keymap {
    /// Merges user overrides (`action name → chords`) over the defaults.
    pub fn with_overrides(overrides: &BTreeMap<String, Vec<String>>) -> Result<Self, ConfigError> {
        let mut bindings = default_bindings();
        let mut explicit: Vec<Action> = Vec::new();
        for (name, chords) in overrides {
            let action: Action = name.parse()?;
            let chords = chords.iter().map(|c| c.parse()).collect::<Result<Vec<Chord>, _>>()?;
            bindings.insert(action, chords);
            explicit.push(action);
        }
        // Explicit user chords win over the defaults of actions the user didn't touch.
        let taken: Vec<Chord> = explicit.iter().flat_map(|a| bindings[a].clone()).collect();
        for (action, chords) in &mut bindings {
            if !explicit.contains(action) {
                chords.retain(|c| !taken.contains(c));
            }
        }
        Ok(Self::from_bindings(bindings))
    }

    fn from_bindings(bindings: BTreeMap<Action, Vec<Chord>>) -> Self {
        let mut lookup = HashMap::new();
        let mut conflicts = Vec::new();
        for (action, chords) in &bindings {
            for chord in chords {
                match lookup.get(chord) {
                    Some(&kept) => {
                        conflicts.push(KeymapConflict { chord: chord.clone(), kept, ignored: *action })
                    }
                    None => {
                        lookup.insert(chord.clone(), *action);
                    }
                }
            }
        }
        Self { bindings, lookup, conflicts }
    }

    pub fn action_for(&self, chord: &Chord) -> Option<Action> {
        self.lookup.get(chord).copied()
    }

    pub fn chords_for(&self, action: Action) -> &[Chord] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn conflicts(&self) -> &[KeymapConflict] {
        &self.conflicts
    }
}

/// Defaults: tabs on `Alt` / F keys, browser and system things on `Ctrl`.
fn default_bindings() -> BTreeMap<Action, Vec<Chord>> {
    let mut map = BTreeMap::new();
    let mut bind = |action: Action, chords: &[&str]| {
        let parsed = chords.iter().filter_map(|c| c.parse().ok()).collect();
        map.insert(action, parsed);
    };
    bind(Action::FocusAddressBar, &["Alt+D", "F6", "Ctrl+K"]);
    bind(Action::NewTab, &["Alt+T"]);
    bind(Action::CloseTab, &["Alt+Q"]);
    bind(Action::ReopenClosedTab, &["Alt+Shift+T"]);
    bind(Action::NextTab, &["Alt+Down", "Alt+PageDown", "Ctrl+Tab"]);
    bind(Action::PreviousTab, &["Alt+Up", "Alt+PageUp", "Ctrl+Shift+Tab"]);
    for n in 1..=8u8 {
        let chord = format!("Alt+{n}");
        bind(Action::SelectTab(n), &[chord.as_str()]);
    }
    bind(Action::SelectLastTab, &["Alt+9"]);
    bind(Action::PinTab, &["Alt+P"]);
    bind(Action::Reload, &["Ctrl+R", "F5"]);
    bind(Action::HardReload, &["Ctrl+Shift+R", "Ctrl+F5"]);
    bind(Action::Back, &["Alt+Left", "Ctrl+["]);
    bind(Action::Forward, &["Alt+Right", "Ctrl+]"]);
    bind(Action::ToggleSidebar, &["Ctrl+Shift+L"]);
    bind(Action::TabOverview, &["Alt+O", "F9"]);
    bind(Action::BookmarkPage, &["Ctrl+D"]);
    bind(Action::ShowBookmarks, &["Ctrl+B"]);
    bind(Action::ShowHistory, &["Ctrl+H"]);
    bind(Action::OpenPlaylist, &["Ctrl+Shift+P"]);
    bind(Action::PlaylistAdd, &["Ctrl+Shift+D"]);
    bind(Action::OpenPasswords, &["Ctrl+P"]);
    bind(Action::Lock, &["Ctrl+L"]);
    bind(Action::Find, &["Ctrl+F"]);
    bind(Action::FindNext, &["Ctrl+G", "F3"]);
    bind(Action::FindPrevious, &["Ctrl+Shift+G", "Shift+F3"]);
    bind(Action::Paste, &["Ctrl+V", "Shift+Insert"]);
    bind(Action::PastePlainText, &["Ctrl+Shift+V"]);
    bind(Action::ZoomIn, &["Ctrl++", "Ctrl+="]);
    bind(Action::ZoomOut, &["Ctrl+-"]);
    bind(Action::ZoomReset, &["Ctrl+0"]);
    bind(Action::OpenSettings, &["Ctrl+S", "Ctrl+,"]);
    bind(Action::ToggleFullscreen, &["F11"]);
    bind(Action::DevTools, &["F12", "Ctrl+Shift+I"]);
    bind(Action::DevToolsConsole, &["Ctrl+Shift+J"]);
    bind(Action::ViewSource, &["Ctrl+U"]);
    bind(Action::Quit, &["Ctrl+Q"]);
    bind(Action::MediaPlayPause, &["Space", "K"]);
    bind(Action::MediaSeekBack, &["Left", "J"]);
    bind(Action::MediaSeekForward, &["Right", "L"]);
    bind(Action::MediaVolumeUp, &["Up"]);
    bind(Action::MediaVolumeDown, &["Down"]);
    bind(Action::MediaMute, &["M"]);
    bind(Action::MediaFullscreen, &["F", "Escape"]);
    bind(Action::MediaCaptions, &["C"]);
    bind(Action::MediaSlower, &["Shift+,"]);
    bind(Action::MediaFaster, &["Shift+."]);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        s.parse().unwrap()
    }

    #[test]
    fn media_defaults_all_parse() {
        let keymap = Keymap::default();
        for (action, _) in SIMPLE_ACTIONS.iter().filter(|(a, _)| a.is_media()) {
            let expected =
                if *action == Action::MediaPlayPause || *action == Action::MediaFullscreen { 2 } else { 1 };
            assert!(keymap.chords_for(*action).len() >= expected, "{action}");
        }
        assert_eq!(keymap.action_for(&chord("Space")), Some(Action::MediaPlayPause));
        assert_eq!(keymap.action_for(&chord("Shift+.")), Some(Action::MediaFaster));
    }

    #[test]
    fn parses_chords() {
        let c = chord("Ctrl+Shift+L");
        assert!(c.modifiers.ctrl && c.modifiers.shift && !c.modifiers.alt);
        assert_eq!(c.key, Key::Char('l'));
        assert_eq!(chord("ctrl++").key, Key::Char('+'));
        assert_eq!(chord("Alt+ArrowLeft").key, Key::Named("Left".into()));
        assert_eq!(chord("F11").key, Key::Named("F11".into()));
        assert_eq!(chord("Ctrl+Shift+\\").to_string(), "Ctrl+Shift+\\");
        assert!("Hyper+X".parse::<Chord>().is_err());
        assert!("Ctrl+Bogus".parse::<Chord>().is_err());
    }

    #[test]
    fn actions_round_trip() {
        for (action, name) in SIMPLE_ACTIONS {
            assert_eq!(action.to_string(), *name);
            assert_eq!(name.parse::<Action>().unwrap(), *action);
        }
        assert_eq!("select_tab_3".parse::<Action>().unwrap(), Action::SelectTab(3));
        assert!("select_tab_9".parse::<Action>().is_err());
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let keymap = Keymap::default();
        assert!(keymap.conflicts().is_empty(), "{:?}", keymap.conflicts());
        assert_eq!(keymap.action_for(&chord("Ctrl+L")), Some(Action::Lock));
        for keys in ["Alt+D", "F6", "Ctrl+K"] {
            assert_eq!(keymap.action_for(&chord(keys)), Some(Action::FocusAddressBar), "{keys}");
        }
        assert_eq!(keymap.action_for(&chord("Alt+3")), Some(Action::SelectTab(3)));
        assert_eq!(keymap.action_for(&chord("Alt+T")), Some(Action::NewTab));
        assert_eq!(keymap.action_for(&chord("Alt+Q")), Some(Action::CloseTab));
        assert_eq!(keymap.action_for(&chord("Ctrl+P")), Some(Action::OpenPasswords));
        assert_eq!(keymap.action_for(&chord("Ctrl+F")), Some(Action::Find));
        assert_eq!(keymap.action_for(&chord("Alt+Left")), Some(Action::Back));
        for (keys, action) in [
            ("Ctrl+H", Action::ShowHistory),
            ("Ctrl+B", Action::ShowBookmarks),
            ("Ctrl+D", Action::BookmarkPage),
            ("Alt+P", Action::PinTab),
            ("Ctrl+Tab", Action::NextTab),
            ("Ctrl+Shift+Tab", Action::PreviousTab),
            ("Ctrl+S", Action::OpenSettings),
            ("Ctrl+,", Action::OpenSettings),
            ("F12", Action::DevTools),
            ("Ctrl+Shift+I", Action::DevTools),
            ("Ctrl+Shift+J", Action::DevToolsConsole),
            ("Ctrl+U", Action::ViewSource),
        ] {
            assert_eq!(keymap.action_for(&chord(keys)), Some(action), "{keys}");
        }
    }

    #[test]
    fn tab_actions_use_alt_and_browser_actions_use_ctrl() {
        let keymap = Keymap::default();
        let tab_actions = [Action::NewTab, Action::CloseTab, Action::ReopenClosedTab, Action::NextTab]
            .into_iter()
            .chain([Action::PreviousTab, Action::PinTab, Action::TabOverview, Action::SelectTab(2)]);
        for action in tab_actions {
            assert!(keymap.chords_for(action).iter().any(|c| c.modifiers.alt), "{action}");
        }
        for action in [
            Action::OpenPasswords,
            Action::ShowHistory,
            Action::ShowBookmarks,
            Action::Find,
            Action::OpenPlaylist,
            Action::PlaylistAdd,
        ] {
            assert!(keymap.chords_for(action).iter().all(|c| c.modifiers.ctrl), "{action}");
        }
    }

    #[test]
    fn explicit_user_chords_win_over_new_defaults() {
        // An old keymap.toml that put Find on Alt+T: the untouched NewTab default yields.
        let mut overrides = BTreeMap::new();
        overrides.insert("find".to_owned(), vec!["Alt+T".to_owned()]);
        let keymap = Keymap::with_overrides(&overrides).unwrap();
        assert_eq!(keymap.action_for(&chord("Alt+T")), Some(Action::Find));
        assert!(keymap.conflicts().is_empty(), "{:?}", keymap.conflicts());
        assert!(keymap.chords_for(Action::NewTab).is_empty());
    }

    #[test]
    fn overrides_replace_and_report_conflicts() {
        let mut overrides = BTreeMap::new();
        overrides.insert("toggle_sidebar".to_owned(), vec!["Ctrl+B".to_owned()]);
        overrides.insert("find".to_owned(), vec!["Alt+T".to_owned()]);
        overrides.insert("new_tab".to_owned(), vec!["Alt+T".to_owned()]);
        overrides.insert("tab_overview".to_owned(), vec![]);
        let keymap = Keymap::with_overrides(&overrides).unwrap();
        assert_eq!(keymap.action_for(&chord("Ctrl+B")), Some(Action::ToggleSidebar));
        assert_eq!(keymap.action_for(&chord("Ctrl+Shift+L")), None);
        assert!(keymap.chords_for(Action::TabOverview).is_empty());
        assert_eq!(keymap.conflicts().len(), 1);
        assert_eq!(keymap.conflicts()[0].kept, Action::NewTab);
    }
}
