//! The one registry of context menus for the chrome.
//!
//! A right click (or the Menu key, Shift+F10) is turned into a [`Target`] by the browser
//! (`browser/registry.rs`: what is under the pointer); [`items`] is the item provider for each
//! kind of target and returns the glass menu's rows; a chosen row arrives as a [`Verb`] that the
//! browser runs on the target it remembered. So a new place for a menu is a new `Target`, a
//! provider arm and a verb arm here and there, never ad-hoc menu code in a widget.
//!
//! Tabs, tab groups, bookmarks of the bar, the address field's tab menu and the page's own menu
//! predate this and keep `context_menu.rs` / `page_menu.rs` (the overview's cards reuse the tab
//! menu's rows). Rows that don't apply are left out or dimmed; shortcuts come from the keymap;
//! destructive rows are red (`MenuItem::destructive`).

use axion_compositor::Icon;
use axion_config::{Action, Keymap};
use axion_i18n::{tr, tr_fmt};

use crate::downloads::{RowFacts, RowState};
use crate::menu::{MenuAction, MenuItem};
use crate::player::Control;

/// The lists whose rows have a menu (in the sidebar or in a sheet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bookmarks,
    History,
    Downloads,
}

/// The sidebar's entries of the "Saved" section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarEntry {
    Bookmarks,
    ReadingList,
    History,
    Playlist,
    Downloads,
    Passwords,
}

/// Toolbar buttons that have a menu, and the config names of the hideable ones
/// (`layout.hidden_toolbar_buttons`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarButton {
    Share,
    NewTab,
    Extensions,
    Downloads,
    Home,
}

impl ToolbarButton {
    /// The name in `layout.hidden_toolbar_buttons`; `None` for a button hidden by its own
    /// setting (`layout.home_button`).
    pub fn config_name(self) -> Option<&'static str> {
        match self {
            ToolbarButton::Share => Some("share"),
            ToolbarButton::NewTab => Some("new_tab"),
            ToolbarButton::Extensions => Some("extensions"),
            ToolbarButton::Downloads => Some("downloads"),
            ToolbarButton::Home => None,
        }
    }
}

/// Every name `layout.hidden_toolbar_buttons` may hold.
#[cfg(test)]
pub const HIDEABLE_BUTTONS: [&str; 4] = ["share", "new_tab", "extensions", "downloads"];

/// What a context menu is about.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Sidebar(SidebarEntry),
    /// A row of a list: in the sidebar panel, or in its sheet.
    Row {
        list: ListKind,
        index: usize,
        url: String,
    },
    Toolbar(ToolbarButton),
    /// The bare toolbar.
    ToolbarBar,
    /// A setting's row in Settings.
    Setting(crate::settings::SettingRef),
    /// Row `i` of the extensions list.
    Extension(usize),
    /// The network chip of the address field.
    Network,
    /// A tile of the start page.
    Tile {
        url: String,
        pinned: bool,
    },
    /// The fullscreen (or inline) media player's controls.
    Player,
}

/// What a chosen row does. The target it acts on is the one the menu was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Open the row's address / the entry's panel.
    Open,
    OpenInNewTab,
    OpenInPrivateTab,
    CopyLink,
    /// Deletes a history entry, removes a bookmark.
    Delete,
    /// Selects (or unselects) the row in its sheet.
    Select,
    /// Deletes the rows selected in the sheet.
    DeleteSelected,
    /// A download's file.
    OpenFile,
    ShowInFolder,
    Pause,
    Resume,
    Cancel,
    RemoveFromList,
    /// Clears the list of the target (History: asks first in its sheet; Downloads: the finished).
    ClearList,
    OpenDownloadsFolder,
    AddToPlaylist,
    LockPasswords,
    HideButton,
    ShowHiddenButtons,
    CustomiseToolbar,
    ResetSetting,
    ExtensionDetails,
    ExtensionToggle,
    ExtensionRemove,
    NetworkSettings,
    NetworkToggle,
    HideNetworkChip,
    PinTile,
    UnpinTile,
    RemoveTile,
    Player(Control),
    CopyPlayerAddress,
}

fn shortcut(keymap: &Keymap, action: Action) -> Option<String> {
    keymap.chords_for(action).first().map(ToString::to_string)
}

fn row(icon: Icon, label: &str, verb: Verb) -> MenuItem {
    MenuItem::new(icon, label, MenuAction::Context(verb))
}

fn keyed(keymap: &Keymap, icon: Icon, label: &str, verb: Verb, action: Action) -> MenuItem {
    match shortcut(keymap, action) {
        Some(keys) => row(icon, label, verb).with_detail(keys),
        None => row(icon, label, verb),
    }
}

/// What the providers need to know beyond the target.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Private tabs may be opened (the account is not locked).
    pub private_ok: bool,
    /// The bookmarks and history database is open.
    pub has_store: bool,
    /// A download row's state.
    pub download: Option<RowFacts>,
    /// The row is in a sheet (the sheets select rows; the sidebar lists do not).
    pub in_sheet: bool,
    /// The row is selected, and how many rows are selected in its sheet.
    pub row_selected: bool,
    pub selected_count: usize,
    /// The Downloads list has finished entries.
    pub downloads_finished: bool,
    /// The account has a password (so there is something to lock).
    pub has_password: bool,
    /// The page can be added to the Playlist.
    pub playlist_ok: bool,
    /// Names of the toolbar buttons hidden now.
    pub hidden_buttons: Vec<String>,
    /// The Home button is hidden (`layout.home_button` off).
    pub home_hidden: bool,
    /// The setting under the pointer already has its default value.
    pub setting_is_default: bool,
    /// The extension of a row is switched on, and has options.
    pub extension_enabled: bool,
    /// The player's state, while one is the target.
    pub player: Option<PlayerFacts>,
    /// The fullscreen player (else an inline one).
    pub playback_speeds: Vec<f32>,
}

/// The playing video, for the player's menu.
#[derive(Clone, Debug, Default)]
pub struct PlayerFacts {
    pub paused: bool,
    pub muted: bool,
    pub fullscreen: bool,
    pub rate: f64,
    pub has_captions: bool,
}

/// The rows of the menu of `target`.
pub fn items(target: &Target, facts: &Facts, keymap: &Keymap) -> Vec<MenuItem> {
    match target {
        Target::Sidebar(entry) => sidebar_items(*entry, facts, keymap),
        Target::Row { list, .. } => {
            let mut items = row_items(*list, facts);
            items.extend(selection_items(*list, facts));
            items
        }
        Target::Toolbar(button) => toolbar_items(*button, facts),
        Target::ToolbarBar => toolbar_bar_items(facts),
        Target::Setting(_) => vec![
            row(Icon::Reload, tr("Reset to Default"), Verb::ResetSetting).enabled(!facts.setting_is_default),
        ],
        Target::Extension(_) => extension_items(facts),
        Target::Network => vec![
            keyed(
                keymap,
                Icon::Globe,
                tr("Turn On or Off for This Site"),
                Verb::NetworkToggle,
                Action::ToggleNetwork,
            ),
            row(Icon::Settings, tr("Network Settings…"), Verb::NetworkSettings).separated(),
            row(Icon::Close, tr("Hide Network Chip"), Verb::HideNetworkChip),
        ],
        Target::Tile { pinned, .. } => tile_items(*pinned, facts),
        Target::Player => player_items(facts, keymap),
    }
}

fn sidebar_items(entry: SidebarEntry, facts: &Facts, keymap: &Keymap) -> Vec<MenuItem> {
    match entry {
        SidebarEntry::Bookmarks => {
            vec![keyed(keymap, Icon::Book, tr("Open Bookmarks"), Verb::Open, Action::ShowBookmarks)]
        }
        SidebarEntry::ReadingList => vec![row(Icon::Glasses, tr("Open Reading List"), Verb::Open)],
        SidebarEntry::History => {
            let mut items =
                vec![keyed(keymap, Icon::History, tr("Open History"), Verb::Open, Action::ShowHistory)];
            if facts.has_store {
                items.push(row(Icon::Trash, tr("Clear History…"), Verb::ClearList).separated().destructive());
            }
            items
        }
        SidebarEntry::Playlist => {
            let mut items =
                vec![keyed(keymap, Icon::Playlist, tr("Open Playlist"), Verb::Open, Action::OpenPlaylist)];
            if facts.playlist_ok {
                items.push(keyed(
                    keymap,
                    Icon::PlaylistAdd,
                    tr("Add Page to Playlist"),
                    Verb::AddToPlaylist,
                    Action::PlaylistAdd,
                ));
            }
            items
        }
        SidebarEntry::Downloads => {
            let mut items = vec![
                row(Icon::Download, tr("Open Downloads"), Verb::Open),
                row(Icon::Folder, tr("Open Downloads Folder"), Verb::OpenDownloadsFolder),
            ];
            if facts.downloads_finished {
                items.push(row(Icon::Trash, tr("Clear List"), Verb::ClearList).separated().destructive());
            }
            items
        }
        SidebarEntry::Passwords => {
            let mut items =
                vec![keyed(keymap, Icon::Key, tr("Open Passwords"), Verb::Open, Action::OpenPasswords)];
            if facts.has_password {
                items.push(keyed(
                    keymap,
                    Icon::Lock,
                    tr("Lock Passwords"),
                    Verb::LockPasswords,
                    Action::Lock,
                ));
            }
            items
        }
    }
}

fn row_items(list: ListKind, facts: &Facts) -> Vec<MenuItem> {
    match list {
        ListKind::Bookmarks | ListKind::History => {
            let mut items = vec![
                row(Icon::Globe, tr("Open"), Verb::Open),
                row(Icon::Plus, tr("Open in New Tab"), Verb::OpenInNewTab),
            ];
            if facts.private_ok {
                items.push(row(Icon::Hand, tr("Open in Private Tab"), Verb::OpenInPrivateTab));
            }
            items.push(row(Icon::Copy, tr("Copy Link"), Verb::CopyLink).separated());
            items.push(
                if list == ListKind::History {
                    row(Icon::Trash, tr("Delete from History"), Verb::Delete)
                } else {
                    row(Icon::Trash, tr("Remove Bookmark"), Verb::Delete)
                }
                .separated()
                .destructive(),
            );
            items
        }
        ListKind::Downloads => download_items(facts.download.as_ref()),
    }
}

/// "Select" and "Delete N Selected" of a row in a sheet.
fn selection_items(list: ListKind, facts: &Facts) -> Vec<MenuItem> {
    if !facts.in_sheet || list == ListKind::Bookmarks {
        return Vec::new();
    }
    let select = if facts.row_selected { tr("Deselect") } else { tr("Select") };
    let mut items = vec![row(Icon::Check, select, Verb::Select).separated()];
    if facts.selected_count > 0 {
        let n = facts.selected_count;
        let label = if n == 1 {
            tr("Delete 1 Selected").to_owned()
        } else {
            tr_fmt!("Delete {count} Selected", count = n)
        };
        items.push(row(Icon::Trash, &label, Verb::DeleteSelected).destructive());
    }
    items
}

fn download_items(download: Option<&RowFacts>) -> Vec<MenuItem> {
    let Some(download) = download else { return Vec::new() };
    let copy = row(Icon::Copy, tr("Copy Download Link"), Verb::CopyLink);
    match download.state {
        RowState::Running => vec![
            row(Icon::Pause, tr("Pause"), Verb::Pause),
            copy,
            row(Icon::Stop, tr("Cancel Download"), Verb::Cancel).separated().destructive(),
        ],
        RowState::Paused => vec![
            row(Icon::Play, tr("Resume"), Verb::Resume),
            copy,
            row(Icon::Stop, tr("Cancel Download"), Verb::Cancel).separated().destructive(),
        ],
        RowState::Finished { file_exists: true } => vec![
            row(Icon::File, tr("Open"), Verb::OpenFile),
            row(Icon::Folder, tr("Show in Folder"), Verb::ShowInFolder),
            copy.separated(),
            row(Icon::Close, tr("Remove from List"), Verb::RemoveFromList).separated().destructive(),
        ],
        RowState::Finished { file_exists: false } | RowState::Failed => {
            vec![
                copy,
                row(Icon::Close, tr("Remove from List"), Verb::RemoveFromList).separated().destructive(),
            ]
        }
    }
}

fn toolbar_items(button: ToolbarButton, facts: &Facts) -> Vec<MenuItem> {
    let mut items = Vec::new();
    match button {
        ToolbarButton::Downloads => {
            items.push(row(Icon::Download, tr("Show All Downloads"), Verb::Open));
            items.push(row(Icon::Folder, tr("Open Downloads Folder"), Verb::OpenDownloadsFolder));
        }
        ToolbarButton::Extensions => items.push(row(Icon::Settings, tr("Manage Extensions…"), Verb::Open)),
        ToolbarButton::Share => items.push(row(Icon::Copy, tr("Copy Link"), Verb::CopyLink)),
        ToolbarButton::NewTab | ToolbarButton::Home => {}
    }
    let separated = !items.is_empty();
    let hide = row(Icon::Close, tr("Hide Button"), Verb::HideButton);
    items.push(if separated { hide.separated() } else { hide });
    items.push(row(Icon::Settings, tr("Customize Toolbar…"), Verb::CustomiseToolbar));
    if hidden_any(facts) {
        items.push(row(Icon::Plus, tr("Show Hidden Buttons"), Verb::ShowHiddenButtons));
    }
    items
}

fn toolbar_bar_items(facts: &Facts) -> Vec<MenuItem> {
    let mut items = vec![row(Icon::Settings, tr("Customize Toolbar…"), Verb::CustomiseToolbar)];
    if hidden_any(facts) {
        items.push(row(Icon::Plus, tr("Show Hidden Buttons"), Verb::ShowHiddenButtons));
    }
    items
}

fn hidden_any(facts: &Facts) -> bool {
    facts.home_hidden || !facts.hidden_buttons.is_empty()
}

fn extension_items(facts: &Facts) -> Vec<MenuItem> {
    vec![
        row(Icon::Settings, tr("Details…"), Verb::ExtensionDetails),
        if facts.extension_enabled {
            row(Icon::Close, tr("Turn Off"), Verb::ExtensionToggle)
        } else {
            row(Icon::Check, tr("Turn On"), Verb::ExtensionToggle)
        },
        row(Icon::Trash, tr("Remove Extension…"), Verb::ExtensionRemove).separated().destructive(),
    ]
}

fn tile_items(pinned: bool, facts: &Facts) -> Vec<MenuItem> {
    let mut items = vec![
        row(Icon::Globe, tr("Open"), Verb::Open),
        row(Icon::Plus, tr("Open in New Tab"), Verb::OpenInNewTab),
    ];
    if facts.private_ok {
        items.push(row(Icon::Hand, tr("Open in Private Tab"), Verb::OpenInPrivateTab));
    }
    items.push(row(Icon::Copy, tr("Copy Link"), Verb::CopyLink).separated());
    items.push(if pinned {
        row(Icon::Pin, tr("Unpin"), Verb::UnpinTile).separated()
    } else {
        row(Icon::Pin, tr("Pin"), Verb::PinTile).separated()
    });
    items.push(row(Icon::Close, tr("Remove from Start Page"), Verb::RemoveTile).destructive());
    items
}

fn player_items(facts: &Facts, keymap: &Keymap) -> Vec<MenuItem> {
    let Some(player) = &facts.player else { return Vec::new() };
    let mut items = vec![
        keyed(
            keymap,
            if player.paused { Icon::Play } else { Icon::Pause },
            if player.paused { tr("Play") } else { tr("Pause") },
            Verb::Player(Control::PlayPause),
            Action::MediaPlayPause,
        ),
        keyed(
            keymap,
            if player.muted { Icon::Volume } else { Icon::Muted },
            if player.muted { tr("Unmute") } else { tr("Mute") },
            Verb::Player(Control::Mute),
            Action::MediaMute,
        ),
    ];
    if !facts.playback_speeds.is_empty() {
        let rows = facts
            .playback_speeds
            .iter()
            .enumerate()
            .map(|(i, speed)| {
                let mut item = row(
                    Icon::Gauge,
                    &crate::player::format_rate(f64::from(*speed)),
                    Verb::Player(Control::SpeedChoice(i)),
                );
                item.icon = None;
                item.checked = (f64::from(*speed) - player.rate).abs() < 0.01;
                item
            })
            .collect();
        items.push(row(Icon::Gauge, tr("Playback Speed"), Verb::Player(Control::Speed)).with_submenu(rows));
    }
    if player.has_captions {
        items.push(keyed(
            keymap,
            Icon::Captions,
            tr("Captions"),
            Verb::Player(Control::Captions),
            Action::MediaCaptions,
        ));
    }
    items.push(
        row(Icon::PictureInPicture, tr("Picture in Picture"), Verb::Player(Control::PictureInPicture))
            .separated(),
    );
    if facts.playlist_ok {
        items.push(row(Icon::PlaylistAdd, tr("Add to Playlist"), Verb::Player(Control::Playlist)));
    }
    items.push(row(Icon::Copy, tr("Copy Video Address"), Verb::CopyPlayerAddress));
    if player.fullscreen {
        items.push(
            keyed(
                keymap,
                Icon::Close,
                tr("Exit Fullscreen"),
                Verb::Player(Control::Fullscreen),
                Action::MediaFullscreen,
            )
            .separated(),
        );
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(target: &Target, facts: &Facts) -> Vec<String> {
        items(target, facts, &Keymap::default()).into_iter().map(|i| i.label).collect()
    }

    fn finished(file_exists: bool) -> Facts {
        Facts {
            download: Some(RowFacts {
                state: RowState::Finished { file_exists },
                url: "https://x/a.zip".into(),
            }),
            ..Facts::default()
        }
    }

    fn download_row() -> Target {
        Target::Row { list: ListKind::Downloads, index: 0, url: String::new() }
    }

    #[test]
    fn a_finished_download_offers_its_file_and_its_folder() {
        assert_eq!(
            labels(&download_row(), &finished(true)),
            ["Open", "Show in Folder", "Copy Download Link", "Remove from List"]
        );
        assert_eq!(labels(&download_row(), &finished(false)), ["Copy Download Link", "Remove from List"]);
    }

    #[test]
    fn a_running_download_can_be_paused_and_cancelled() {
        let facts = Facts {
            download: Some(RowFacts { state: RowState::Running, url: String::new() }),
            ..Facts::default()
        };
        assert_eq!(labels(&download_row(), &facts), ["Pause", "Copy Download Link", "Cancel Download"]);
        let paused = Facts {
            download: Some(RowFacts { state: RowState::Paused, url: String::new() }),
            ..Facts::default()
        };
        assert_eq!(labels(&download_row(), &paused)[0], "Resume");
    }

    /// Rows of a sheet offer to select them; the delete entry names the count and is red.
    #[test]
    fn sheet_rows_offer_selection() {
        let target = Target::Row { list: ListKind::History, index: 3, url: "https://a/".into() };
        let sidebar = Facts::default();
        assert!(
            !labels(&target, &sidebar).contains(&"Select".to_owned()),
            "the sidebar list has no selection"
        );
        let mut facts = Facts { in_sheet: true, ..Facts::default() };
        let all = labels(&target, &facts);
        assert_eq!(all.last().map(String::as_str), Some("Select"));
        facts.selected_count = 3;
        facts.row_selected = true;
        let menu = items(&target, &facts, &Keymap::default());
        let labels: Vec<&str> = menu.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(&labels[labels.len() - 2..], ["Deselect", "Delete 3 Selected"]);
        assert!(menu.last().is_some_and(|i| i.destructive));
        // A download row gets them too; a bookmark never.
        let download = Target::Row { list: ListKind::Downloads, index: 0, url: String::new() };
        assert!(!selection_items(ListKind::Downloads, &facts).is_empty());
        assert!(selection_items(ListKind::Bookmarks, &facts).is_empty());
        let _ = download;
    }

    #[test]
    fn history_and_bookmark_rows() {
        let history = Target::Row { list: ListKind::History, index: 0, url: "https://a/".into() };
        let facts = Facts { private_ok: true, ..Facts::default() };
        assert_eq!(
            labels(&history, &facts),
            ["Open", "Open in New Tab", "Open in Private Tab", "Copy Link", "Delete from History"]
        );
        let bookmark = Target::Row { list: ListKind::Bookmarks, index: 0, url: "https://a/".into() };
        assert_eq!(labels(&bookmark, &Facts::default()).last().map(String::as_str), Some("Remove Bookmark"));
    }

    /// Every destructive verb is red and nothing else is: deleting, removing, cancelling, clearing.
    #[test]
    fn destructive_rows_are_red_and_only_they() {
        let targets = [
            Target::Sidebar(SidebarEntry::History),
            Target::Sidebar(SidebarEntry::Downloads),
            Target::Row { list: ListKind::History, index: 0, url: String::new() },
            Target::Row { list: ListKind::Bookmarks, index: 0, url: String::new() },
            download_row(),
            Target::Extension(0),
            Target::Tile { url: String::new(), pinned: false },
        ];
        let destructive = [
            Verb::Delete,
            Verb::Cancel,
            Verb::RemoveFromList,
            Verb::ClearList,
            Verb::ExtensionRemove,
            Verb::RemoveTile,
        ];
        let facts = Facts {
            has_store: true,
            downloads_finished: true,
            private_ok: true,
            download: Some(RowFacts { state: RowState::Finished { file_exists: true }, url: String::new() }),
            ..Facts::default()
        };
        for target in &targets {
            for item in items(target, &facts, &Keymap::default()) {
                let MenuAction::Context(verb) = item.action else { panic!("{target:?}: not a context row") };
                assert_eq!(item.destructive, destructive.contains(&verb), "{target:?}: {}", item.label);
            }
        }
    }

    #[test]
    fn toolbar_menus_offer_show_hidden_only_when_something_is_hidden() {
        let none = Facts::default();
        assert!(
            !labels(&Target::Toolbar(ToolbarButton::Share), &none).contains(&"Show Hidden Buttons".into())
        );
        let hidden = Facts { hidden_buttons: vec!["share".into()], ..Facts::default() };
        assert!(
            labels(&Target::Toolbar(ToolbarButton::Share), &hidden).contains(&"Show Hidden Buttons".into())
        );
        assert_eq!(labels(&Target::ToolbarBar, &hidden), ["Customize Toolbar…", "Show Hidden Buttons"]);
        assert_eq!(labels(&Target::ToolbarBar, &none), ["Customize Toolbar…"]);
        let home_hidden = Facts { home_hidden: true, ..Facts::default() };
        assert!(labels(&Target::ToolbarBar, &home_hidden).contains(&"Show Hidden Buttons".into()));
    }

    #[test]
    fn the_hideable_buttons_match_their_config_names() {
        for button in
            [ToolbarButton::Share, ToolbarButton::NewTab, ToolbarButton::Extensions, ToolbarButton::Downloads]
        {
            assert!(HIDEABLE_BUTTONS.contains(&button.config_name().unwrap_or("")), "{button:?}");
        }
        assert_eq!(ToolbarButton::Home.config_name(), None);
    }

    #[test]
    fn shortcuts_come_from_the_keymap() {
        let items = items(&Target::Sidebar(SidebarEntry::History), &Facts::default(), &Keymap::default());
        assert_eq!(items[0].detail.as_deref(), Some("Ctrl+H"));
        let facts = Facts {
            player: Some(PlayerFacts { fullscreen: true, ..PlayerFacts::default() }),
            playback_speeds: vec![1.0, 2.0],
            playlist_ok: true,
            ..Facts::default()
        };
        let player = super::items(&Target::Player, &facts, &Keymap::default());
        assert_eq!(player.first().and_then(|i| i.detail.as_deref()), Some("Space"));
        assert_eq!(player.last().map(|i| i.label.as_str()), Some("Exit Fullscreen"));
        // The speeds are a submenu with the current one checked.
        let speeds = &player.iter().find(|i| i.label == "Playback Speed").map(|i| i.submenu.len());
        assert_eq!(*speeds, Some(2));
    }

    #[test]
    fn a_setting_at_its_default_cannot_be_reset() {
        let at_default = Facts { setting_is_default: true, ..Facts::default() };
        let items =
            items(&Target::Setting(crate::settings::SettingRef::for_test()), &at_default, &Keymap::default());
        assert!(items[0].disabled);
        let changed = super::items(
            &Target::Setting(crate::settings::SettingRef::for_test()),
            &Facts::default(),
            &Keymap::default(),
        );
        assert!(!changed[0].disabled);
    }
}
