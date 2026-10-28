//! The page's own context menu: right-clicking inside a page opens a glass [`crate::menu::Menu`]
//! that drips out of the pointer. Off-screen Chromium has no native menu, so the engine
//! reports what was clicked ([`ContextMenuParams`]) and this module decides the rows: link,
//! image, video or audio, selected text, editable field, or the plain page. Rows that don't
//! apply right now are dimmed, and shortcuts are shown from the keymap. The builder is a pure
//! function of the parameters, so the rows per context are unit-tested; the browser runs the
//! commands (see `browser/page_menu.rs`).

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Action, Keymap};
use axion_engine::{ContextMedia, ContextMenuParams};
use axion_i18n::{tr, tr_fmt};
use axion_media::Player;
use glam::Vec2;

use crate::menu::{MenuAction, MenuItem, WIDTH, WINDOW_MARGIN};

/// What a row of the page's context menu does. The menu's target (the link, the image, the
/// selection) is kept by the browser for as long as the menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCommand {
    OpenLink,
    OpenLinkInBackground,
    OpenLinkInPrivate,
    CopyLink,
    SaveLink,
    OpenImage,
    SaveImage,
    CopyImage,
    CopyImageAddress,
    /// Play or pause the clicked video or audio.
    MediaToggle,
    MediaMute,
    MediaPictureInPicture,
    MediaAddToPlaylist,
    SaveMedia,
    CopyMediaAddress,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    SearchSelection,
    /// Send the selected text (with the page's address) in a new e-mail.
    ShareSelection,
    Back,
    Forward,
    Reload,
    SavePage,
    ViewSource,
    /// Open the developer tools with the clicked element selected.
    Inspect,
}

/// The state of the clicked video or audio, from the media bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaInfo {
    pub playing: bool,
    pub muted: bool,
    /// It is a video (Picture in Picture and the Playlist are for those).
    pub video: bool,
}

/// What besides the click decides the rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageMenuInfo {
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// A private tab can be opened (private tabs aren't locked away).
    pub private_ok: bool,
    /// Videos can be added to the Playlist (a normal tab, and the profile's database is open).
    pub playlist_ok: bool,
    /// The media bridge knows the clicked player (`None`: it isn't tracked, its rows are dimmed).
    pub media: Option<MediaInfo>,
    /// The engine reaches the desktop clipboard by itself (otherwise Chromium can't tell
    /// whether there is anything to paste, and Paste is always offered in a field).
    pub system_clipboard: bool,
    /// The developer tools are enabled (`devtools.enabled`): the menu ends with Inspect.
    pub devtools: bool,
    /// The default search engine's name ("DuckDuckGo").
    pub search_engine: String,
    /// The most characters of the selection shown in the search row.
    pub label_chars: usize,
    /// The selection menu offers Share (`ui.page_context_menu_share`).
    pub share: bool,
}

/// Addresses Axion can open in a tab and download.
pub fn is_web_address(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    ["http://", "https://", "file://", "ftp://"].iter().any(|scheme| url.starts_with(scheme))
}

/// Addresses a download can fetch (not `file:`, nor `blob:` streams a page assembled).
fn is_downloadable(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}

/// Addresses an image can be fetched from for a copy.
fn is_fetchable_image(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    is_downloadable(&url) || url.starts_with("data:image/") || url.starts_with("file://")
}

/// A page whose source and file make sense.
fn is_document(url: &str) -> bool {
    is_web_address(url) && !url.trim().to_ascii_lowercase().starts_with("ftp://")
}

/// `text` on one line, cut to `limit` characters with an ellipsis.
pub fn shorten(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        return line;
    }
    let cut: String = line.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

/// The name of the search engine behind `search_url`: one of Settings' engines, else its host.
pub fn search_engine_name(search_url: &str) -> String {
    crate::settings::SEARCH_ENGINES
        .iter()
        .find(|(_, url)| *url == search_url)
        .map(|(name, _)| (*name).to_owned())
        .unwrap_or_else(|| crate::url::display_host(search_url))
}

/// Pixels a character of a row's label takes (an overestimate, so no label is cut), and what
/// a row has besides its label: its insets, icon, gap and the detail's room.
const CHAR_WIDTH: f32 = 8.0;
const ROW_CHROME: f32 = 72.0;

/// How wide the menu for `items` is: the usual width, wider for a long label (the search row).
pub fn menu_width(items: &[MenuItem], window: Vec2) -> f32 {
    let longest = items
        .iter()
        .map(|i| i.label.chars().count() + i.detail.as_ref().map_or(0, |d| d.chars().count() + 2))
        .max()
        .unwrap_or(0);
    let wanted = ROW_CHROME + longest as f32 * CHAR_WIDTH;
    wanted.max(WIDTH).min((window.x - 2.0 * WINDOW_MARGIN).max(WIDTH))
}

/// Where the menu (`width` wide) opens for a click at `pos` (logical px in the window): a point
/// the glass drips out of. Near the window's right edge the menu opens to the left of the
/// pointer instead (the menu itself also grows upward when it doesn't fit below).
pub fn anchor(pos: Vec2, window: Vec2, width: f32) -> Frame {
    let x = if pos.x + width + WINDOW_MARGIN > window.x { (pos.x - width).max(WINDOW_MARGIN) } else { pos.x };
    Frame::new(x, pos.y, 1.0, 1.0)
}

/// The player under a click: the smallest one whose rectangle holds the point, else the one
/// that plays `source_url`. `css` is the click in CSS pixels of the top frame's viewport.
pub fn player_at<'a>(players: &'a [Player], css: Vec2, source_url: &str) -> Option<&'a Player> {
    let inside = |p: &&Player| css.x >= p.x && css.x <= p.x + p.w && css.y >= p.y && css.y <= p.y + p.h;
    players
        .iter()
        .filter(inside)
        .min_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)))
        .or_else(|| players.iter().find(|p| !source_url.is_empty() && p.src == source_url))
}

/// The rows of the page's context menu for a click described by `params`.
pub fn items(params: &ContextMenuParams, info: &PageMenuInfo, keymap: &Keymap) -> Vec<MenuItem> {
    let shortcut = |action: Action| keymap.chords_for(action).first().map(ToString::to_string);
    let row =
        |icon: Icon, label: &str, command: PageCommand| MenuItem::new(icon, label, MenuAction::Page(command));
    let keyed = |icon: Icon, label: &str, command: PageCommand, action: Action| {
        let item = row(icon, label, command);
        match shortcut(action) {
            Some(keys) => item.with_detail(keys),
            None => item,
        }
    };

    let mut groups: Vec<Vec<MenuItem>> = Vec::new();

    // A link (also around an image).
    let link = params.link_url.trim();
    if !link.is_empty() {
        let mut group = Vec::new();
        if is_web_address(link) {
            group.push(row(Icon::Plus, tr("Open Link in New Tab"), PageCommand::OpenLink));
            group.push(row(Icon::TabGroup, tr("Open in Background Tab"), PageCommand::OpenLinkInBackground));
            if info.private_ok {
                group.push(row(Icon::Hand, tr("Open in Private Tab"), PageCommand::OpenLinkInPrivate));
            }
        }
        group.push(row(Icon::Copy, tr("Copy Link Address"), PageCommand::CopyLink));
        if is_downloadable(link) {
            group.push(row(Icon::Download, tr("Save Link As…"), PageCommand::SaveLink));
        }
        groups.push(group);
    }

    // An image, video or audio element.
    match params.media {
        ContextMedia::Image => {
            let source = params.source_url.trim();
            groups.push(vec![
                row(Icon::Plus, tr("Open Image in New Tab"), PageCommand::OpenImage)
                    .enabled(is_web_address(source) || source.starts_with("data:")),
                row(Icon::Download, tr("Save Image As…"), PageCommand::SaveImage)
                    .enabled(is_downloadable(source)),
                row(Icon::Copy, tr("Copy Image"), PageCommand::CopyImage)
                    .enabled(params.has_image_contents && is_fetchable_image(source)),
                row(Icon::Globe, tr("Copy Image Address"), PageCommand::CopyImageAddress)
                    .enabled(!source.is_empty() && !source.starts_with("data:")),
            ]);
        }
        ContextMedia::Video | ContextMedia::Audio => {
            let video = params.media == ContextMedia::Video;
            let source = params.source_url.trim();
            let known = info.media.is_some();
            let state = info.media.unwrap_or_default();
            let mut group = vec![
                if state.playing {
                    row(Icon::Pause, tr("Pause"), PageCommand::MediaToggle)
                } else {
                    row(Icon::Play, tr("Play"), PageCommand::MediaToggle)
                }
                .enabled(known),
                if state.muted {
                    row(Icon::Volume, tr("Unmute"), PageCommand::MediaMute)
                } else {
                    row(Icon::Muted, tr("Mute"), PageCommand::MediaMute)
                }
                .enabled(known),
            ];
            if video {
                group.push(
                    row(Icon::PictureInPicture, tr("Picture in Picture"), PageCommand::MediaPictureInPicture)
                        .enabled(known),
                );
                if info.playlist_ok {
                    group.push(
                        row(Icon::PlaylistAdd, tr("Add to Playlist"), PageCommand::MediaAddToPlaylist)
                            .enabled(known),
                    );
                }
            }
            group.push(
                row(
                    Icon::Download,
                    if video { tr("Save Video As…") } else { tr("Save Audio As…") },
                    PageCommand::SaveMedia,
                )
                .enabled(is_downloadable(source))
                .separated(),
            );
            group.push(
                row(
                    Icon::Globe,
                    if video { tr("Copy Video Address") } else { tr("Copy Audio Address") },
                    PageCommand::CopyMediaAddress,
                )
                .enabled(!source.is_empty() && !source.starts_with("blob:")),
            );
            groups.push(group);
        }
        ContextMedia::None | ContextMedia::Canvas | ContextMedia::Other => {}
    }

    let selection = params.selection_text.trim();
    // The rows about the selected text, after Copy (and Cut …) in every menu that has a
    // selection. Further selection actions (Translate …) go in `selection_rows`.
    let selection_rows = || selection_rows(selection, info);
    if params.editable {
        let edit = params.edit;
        let mut group = vec![
            row(Icon::Back, tr("Undo"), PageCommand::Undo).enabled(edit.undo),
            row(Icon::Forward, tr("Redo"), PageCommand::Redo).enabled(edit.redo),
            row(Icon::Close, tr("Cut"), PageCommand::Cut).enabled(edit.cut).separated(),
            row(Icon::Copy, tr("Copy"), PageCommand::Copy).enabled(edit.copy),
            keyed(Icon::Note, tr("Paste"), PageCommand::Paste, Action::Paste)
                .enabled(edit.paste || !info.system_clipboard),
            row(Icon::Overview, tr("Select All"), PageCommand::SelectAll)
                .enabled(edit.select_all)
                .separated(),
        ];
        if !selection.is_empty() {
            group.extend(
                selection_rows().into_iter().enumerate().map(|(i, r)| if i == 0 { r.separated() } else { r }),
            );
        }
        groups.push(group);
    } else if !selection.is_empty() {
        let mut group = vec![row(Icon::Copy, tr("Copy"), PageCommand::Copy)];
        group.extend(selection_rows());
        groups.push(group);
    }

    // Nothing in particular under the pointer: the page itself.
    if groups.is_empty() {
        let page = params.page_url.trim();
        groups.push(vec![
            keyed(Icon::Back, tr("Back"), PageCommand::Back, Action::Back).enabled(info.can_go_back),
            keyed(Icon::Forward, tr("Forward"), PageCommand::Forward, Action::Forward)
                .enabled(info.can_go_forward),
            keyed(Icon::Reload, tr("Reload"), PageCommand::Reload, Action::Reload),
        ]);
        groups.push(vec![
            row(Icon::Download, tr("Save Page As…"), PageCommand::SavePage).enabled(is_downloadable(page)),
            row(Icon::Eye, tr("View Page Source"), PageCommand::ViewSource)
                .enabled(is_document(page) && !page.starts_with("view-source:")),
        ]);
        groups.push(vec![row(Icon::Overview, tr("Select All"), PageCommand::SelectAll)]);
    }

    if info.devtools {
        groups.push(vec![row(Icon::Code, tr("Inspect"), PageCommand::Inspect)]);
    }

    let mut out = Vec::new();
    for group in groups {
        for (i, item) in group.into_iter().enumerate() {
            out.push(if i == 0 && !out.is_empty() { item.separated() } else { item });
        }
    }
    out
}

/// The rows that act on the selected text (`selection`, trimmed and not empty): search, share.
/// This is the one place to add further selection actions: push a row with its own
/// [`PageCommand`] (and run it in `browser/page_context.rs`).
fn selection_rows(selection: &str, info: &PageMenuInfo) -> Vec<MenuItem> {
    let mut rows = Vec::new();
    let label = tr_fmt!(
        "Search {engine} for “{text}”",
        engine = info.search_engine,
        text = shorten(selection, info.label_chars)
    );
    rows.push(MenuItem::new(Icon::Search, &label, MenuAction::Page(PageCommand::SearchSelection)));
    if info.share {
        rows.push(MenuItem::new(Icon::Share, tr("Share…"), MenuAction::Page(PageCommand::ShareSelection)));
    }
    rows
}

/// The `mailto:` address that sends `text` (and the address of the page it is from).
pub fn share_url(text: &str, page_url: &str) -> String {
    let mut body = text.trim().to_owned();
    if is_web_address(page_url) {
        body.push_str("\n\n");
        body.push_str(page_url.trim());
    }
    format!("mailto:?body={}", crate::url::encode_query(&body).replace('+', "%20"))
}

#[cfg(test)]
mod tests {
    use axion_engine::EditFlags;

    use super::*;

    fn info() -> PageMenuInfo {
        PageMenuInfo {
            can_go_back: true,
            private_ok: true,
            playlist_ok: true,
            media: Some(MediaInfo { video: true, ..Default::default() }),
            system_clipboard: true,
            search_engine: "DuckDuckGo".into(),
            label_chars: 12,
            share: true,
            ..Default::default()
        }
    }

    fn page() -> ContextMenuParams {
        ContextMenuParams { page_url: "https://example.com/a".into(), ..Default::default() }
    }

    fn menu(params: &ContextMenuParams, info: &PageMenuInfo) -> Vec<(String, bool)> {
        items(params, info, &Keymap::default()).into_iter().map(|i| (i.label, !i.disabled)).collect()
    }

    fn labels(params: &ContextMenuParams, info: &PageMenuInfo) -> Vec<String> {
        menu(params, info).into_iter().map(|(l, _)| l).collect()
    }

    fn enabled(rows: &[(String, bool)], label: &str) -> bool {
        rows.iter().find(|(l, _)| l == label).unwrap_or_else(|| panic!("{label} missing in {rows:?}")).1
    }

    #[test]
    fn inspect_ends_every_menu_when_devtools_are_enabled() {
        let on = PageMenuInfo { devtools: true, ..info() };
        let contexts = [
            page(),
            ContextMenuParams { link_url: "https://example.com/x".into(), ..page() },
            ContextMenuParams {
                media: ContextMedia::Image,
                source_url: "https://example.com/a.png".into(),
                ..page()
            },
            ContextMenuParams {
                media: ContextMedia::Video,
                source_url: "https://e.com/v.mp4".into(),
                ..page()
            },
            ContextMenuParams { selection_text: "word".into(), ..page() },
            ContextMenuParams { editable: true, ..page() },
        ];
        for params in &contexts {
            let items = items(params, &on, &Keymap::default());
            let last = items.last().expect("rows");
            assert_eq!(last.label, "Inspect");
            assert!(last.separated && !last.disabled);
            assert_eq!(items.iter().filter(|i| i.label == "Inspect").count(), 1);
            // Hidden when the developer tools are disabled.
            assert!(!labels(params, &info()).contains(&"Inspect".to_owned()));
        }
        let names = labels(&page(), &on);
        assert_eq!(names.last().map(String::as_str), Some("Inspect"));
        assert_eq!(names.len(), 7);
    }

    #[test]
    fn plain_page() {
        let rows = menu(&page(), &info());
        let names: Vec<_> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(names, ["Back", "Forward", "Reload", "Save Page As…", "View Page Source", "Select All"]);
        assert!(enabled(&rows, "Back"));
        assert!(!enabled(&rows, "Forward"));
        assert!(enabled(&rows, "Reload"));
        assert!(enabled(&rows, "View Page Source"));
    }

    #[test]
    fn internal_and_source_pages_have_no_source_or_file() {
        for url in ["axion://home/", "view-source:https://example.com/", "about:blank"] {
            let params = ContextMenuParams { page_url: url.into(), ..Default::default() };
            let rows = menu(&params, &info());
            assert!(!enabled(&rows, "Save Page As…"), "{url}");
            assert!(!enabled(&rows, "View Page Source"), "{url}");
        }
    }

    #[test]
    fn navigation_shows_shortcuts_from_the_keymap() {
        let items = items(&page(), &info(), &Keymap::default());
        let detail = |label: &str| items.iter().find(|i| i.label == label).and_then(|i| i.detail.clone());
        assert_eq!(detail("Back").as_deref(), Some("Alt+Left"));
        assert_eq!(detail("Reload").as_deref(), Some("Ctrl+R"));
        assert!(items.iter().find(|i| i.label == "Select All").is_some_and(|i| i.detail.is_none()));
    }

    #[test]
    fn groups_are_separated() {
        let items = items(&page(), &info(), &Keymap::default());
        let separated: Vec<_> = items.iter().filter(|i| i.separated).map(|i| i.label.as_str()).collect();
        assert_eq!(separated, ["Save Page As…", "Select All"]);
    }

    #[test]
    fn link() {
        let params = ContextMenuParams { link_url: "https://example.com/x".into(), ..page() };
        assert_eq!(
            labels(&params, &info()),
            [
                "Open Link in New Tab",
                "Open in Background Tab",
                "Open in Private Tab",
                "Copy Link Address",
                "Save Link As…"
            ]
        );
        let locked = PageMenuInfo { private_ok: false, ..info() };
        assert!(!labels(&params, &locked).contains(&"Open in Private Tab".to_owned()));
        // A mail link can only be copied.
        let mail = ContextMenuParams { link_url: "mailto:a@example.com".into(), ..page() };
        assert_eq!(labels(&mail, &info()), ["Copy Link Address"]);
    }

    #[test]
    fn image() {
        let params = ContextMenuParams {
            media: ContextMedia::Image,
            source_url: "https://example.com/a.png".into(),
            has_image_contents: true,
            ..page()
        };
        let rows = menu(&params, &info());
        let names: Vec<_> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(names, ["Open Image in New Tab", "Save Image As…", "Copy Image", "Copy Image Address"]);
        assert!(rows.iter().all(|(_, on)| *on));
        // Not decoded yet (or a blob): can't be copied or saved.
        let blob = ContextMenuParams { source_url: "blob:https://example.com/1".into(), ..params.clone() };
        let rows = menu(&blob, &info());
        assert!(!enabled(&rows, "Save Image As…"));
        assert!(!enabled(&rows, "Copy Image"));
        assert!(enabled(&rows, "Copy Image Address"));
        let broken = ContextMenuParams { has_image_contents: false, ..params };
        assert!(!enabled(&menu(&broken, &info()), "Copy Image"));
    }

    #[test]
    fn image_in_a_link_has_both() {
        let params = ContextMenuParams {
            media: ContextMedia::Image,
            source_url: "https://example.com/a.png".into(),
            link_url: "https://example.com/".into(),
            ..page()
        };
        let names = labels(&params, &info());
        assert_eq!(names.first().map(String::as_str), Some("Open Link in New Tab"));
        assert!(names.contains(&"Save Image As…".to_owned()));
    }

    #[test]
    fn video() {
        let params = ContextMenuParams {
            media: ContextMedia::Video,
            source_url: "https://example.com/v.mp4".into(),
            ..page()
        };
        let names = labels(&params, &info());
        assert_eq!(
            names,
            ["Play", "Mute", "Picture in Picture", "Add to Playlist", "Save Video As…", "Copy Video Address"]
        );
        // Playing and muted flip the first rows.
        let playing =
            PageMenuInfo { media: Some(MediaInfo { playing: true, muted: true, video: true }), ..info() };
        let names = labels(&params, &playing);
        assert_eq!(&names[..2], ["Pause", "Unmute"]);
        // Private tabs have no Playlist.
        let private = PageMenuInfo { playlist_ok: false, ..info() };
        assert!(!labels(&params, &private).contains(&"Add to Playlist".to_owned()));
        // A player the bridge doesn't know, playing a stream: only the address is left.
        let stream = ContextMenuParams { source_url: "blob:https://example.com/2".into(), ..params };
        let unknown = PageMenuInfo { media: None, ..info() };
        let rows = menu(&stream, &unknown);
        assert!(!enabled(&rows, "Play"));
        assert!(!enabled(&rows, "Picture in Picture"));
        assert!(!enabled(&rows, "Save Video As…"));
        assert!(!enabled(&rows, "Copy Video Address"));
    }

    #[test]
    fn audio_has_no_picture_in_picture_or_playlist() {
        let params = ContextMenuParams {
            media: ContextMedia::Audio,
            source_url: "https://example.com/a.mp3".into(),
            ..page()
        };
        let info = PageMenuInfo { media: Some(MediaInfo::default()), ..info() };
        assert_eq!(labels(&params, &info), ["Play", "Mute", "Save Audio As…", "Copy Audio Address"]);
    }

    #[test]
    fn selection() {
        let params = ContextMenuParams { selection_text: "  hello\n  wide world of text ".into(), ..page() };
        let names = labels(&params, &info());
        assert_eq!(names, ["Copy", "Search DuckDuckGo for “hello wide w…”", "Share…"]);
    }

    #[test]
    fn editable_field_follows_the_edit_flags() {
        let params = ContextMenuParams {
            editable: true,
            edit: EditFlags { copy: true, cut: true, select_all: true, ..Default::default() },
            ..page()
        };
        let rows = menu(&params, &info());
        let names: Vec<_> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(names, ["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"]);
        assert!(!enabled(&rows, "Undo"));
        assert!(!enabled(&rows, "Redo"));
        assert!(enabled(&rows, "Cut"));
        assert!(enabled(&rows, "Copy"));
        assert!(!enabled(&rows, "Paste"));
        // Where Chromium can't see the clipboard, a field can always be pasted into.
        let wayland = PageMenuInfo { system_clipboard: false, ..info() };
        assert!(enabled(&menu(&params, &wayland), "Paste"));
        // A selected word in a field can be searched for.
        let selected = ContextMenuParams { selection_text: "word".into(), ..params };
        assert!(labels(&selected, &info()).iter().any(|l| l.starts_with("Search DuckDuckGo")));
    }

    #[test]
    fn shortening() {
        assert_eq!(shorten("a  b\n c", 10), "a b c");
        assert_eq!(shorten("abcdefghij", 4), "abcd…");
        assert_eq!(shorten("ab cd ef", 6), "ab cd…");
    }

    #[test]
    fn engine_names() {
        assert_eq!(search_engine_name("https://duckduckgo.com/?q={}"), "DuckDuckGo");
        assert_eq!(search_engine_name("https://search.example.org/find?q={}"), "search.example.org");
    }

    #[test]
    fn menu_opens_left_of_the_pointer_near_the_right_edge() {
        let window = Vec2::new(1000.0, 800.0);
        assert_eq!(anchor(Vec2::new(100.0, 50.0), window, WIDTH).origin, Vec2::new(100.0, 50.0));
        let near = anchor(Vec2::new(900.0, 50.0), window, WIDTH);
        assert_eq!(near.origin.x, 900.0 - WIDTH);
        assert!(near.origin.x + WIDTH <= window.x);
    }

    #[test]
    fn long_search_row_widens_the_menu() {
        let window = Vec2::new(1000.0, 800.0);
        let plain = items(&page(), &info(), &Keymap::default());
        assert_eq!(menu_width(&plain, window), WIDTH);
        let params = ContextMenuParams { selection_text: "a rather long selected sentence".into(), ..page() };
        let searching = items(&params, &info(), &Keymap::default());
        assert!(menu_width(&searching, window) > WIDTH);
        // Never wider than the window.
        assert!(menu_width(&searching, Vec2::new(300.0, 800.0)) <= 300.0);
    }

    fn player(id: &str, x: f32, y: f32, w: f32, h: f32, src: &str) -> Player {
        Player { id: id.into(), x, y, w, h, src: src.into(), ..Default::default() }
    }

    #[test]
    fn the_smallest_player_under_the_click_wins() {
        let players =
            [player("big", 0.0, 0.0, 800.0, 600.0, "a"), player("small", 100.0, 100.0, 200.0, 100.0, "b")];
        assert_eq!(player_at(&players, Vec2::new(150.0, 150.0), "").map(|p| p.id.as_str()), Some("small"));
        assert_eq!(player_at(&players, Vec2::new(700.0, 500.0), "").map(|p| p.id.as_str()), Some("big"));
        // Off every player: found by the address it plays.
        assert_eq!(player_at(&players, Vec2::new(900.0, 900.0), "b").map(|p| p.id.as_str()), Some("small"));
        assert!(player_at(&players, Vec2::new(900.0, 900.0), "").is_none());
    }

    #[test]
    fn selection_offers_search_and_share() {
        let params = ContextMenuParams { selection_text: "two words".into(), ..page() };
        let names = labels(&params, &info());
        assert_eq!(names, ["Copy", "Search DuckDuckGo for “two words”", "Share…"]);
        let off = PageMenuInfo { share: false, ..info() };
        assert!(!labels(&params, &off).contains(&"Share…".to_owned()));
        let field = ContextMenuParams { editable: true, ..params };
        assert!(labels(&field, &info()).contains(&"Share…".to_owned()));
    }

    #[test]
    fn share_url_carries_text_and_page() {
        assert_eq!(
            share_url("a b+c", "https://x.org/p"),
            "mailto:?body=a%20b%2Bc%0A%0Ahttps%3A%2F%2Fx.org%2Fp"
        );
        assert_eq!(share_url("hi", "axion://home/"), "mailto:?body=hi");
    }
}
