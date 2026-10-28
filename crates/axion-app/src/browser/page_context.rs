//! The browser's side of the page's context menu (`page_menu`): opening it where the page was
//! right-clicked, and what its rows do.

use axion_engine::{ContextCommand, ContextMenuParams};
use axion_i18n::tr;
use axion_media::{Action as MediaAction, Command as MediaCommand, Kind, Player};

use super::*;
use crate::page_menu::{self, MediaInfo, PageCommand, PageMenuInfo};

/// What the open page menu is about: the view, what was clicked, and the player under it.
#[derive(Clone)]
pub(super) struct PageContext {
    view: ViewId,
    params: ContextMenuParams,
    player: Option<Player>,
}

impl Browser {
    /// The page was right-clicked: opens the glass menu dripping out of the pointer.
    pub(super) fn show_page_menu(&mut self, view: ViewId, params: ContextMenuParams) {
        if Some(view) != self.active_view() || self.is_locked() || self.playlist_open() {
            return;
        }
        let Some(tab) = self.tabs.active() else { return };
        let page = self.page_rect();
        // Where the click was in the window; the pointer itself if the engine's position is off.
        let mut pos = page.origin + params.pos;
        if !page.contains(pos)
            && let Some(pointer) = self.pointer.filter(|p| page.contains(*p))
        {
            pos = pointer;
        }
        // The media bridge's player under the click (its rectangles are in CSS px).
        let player = tab.media.as_ref().and_then(|media| {
            let css = params.pos * self.scale() / media.dpr.max(f32::EPSILON);
            page_menu::player_at(&media.players, css, &params.source_url).cloned()
        });
        let info = PageMenuInfo {
            can_go_back: tab.can_go_back,
            can_go_forward: tab.can_go_forward,
            private_ok: !self.private_locked(),
            playlist_ok: !tab.private && self.store.is_some(),
            media: player.as_ref().map(|p| MediaInfo {
                playing: p.is_playing(),
                muted: p.muted,
                video: p.kind == Kind::Video,
            }),
            system_clipboard: self.engine.has_system_clipboard(),
            search_engine: page_menu::search_engine_name(&self.config.general.search_url),
            devtools: self.config.devtools.enabled,
            label_chars: self.config.ui.page_context_menu_label_chars as usize,
        };
        let items = page_menu::items(&params, &info, &self.keymap);
        if items.is_empty() {
            return;
        }
        self.close_other_menus();
        self.context_tab = None;
        self.context_row = None;
        self.tooltip.cancel(&mut self.ui);
        self.page_context = Some(PageContext { view, params, player });
        let window = self.logical_size();
        let width = page_menu::menu_width(&items, window);
        let anchor = page_menu::anchor(pos, window, width);
        self.context_menus.open_page(&mut self.ui, &self.config, anchor, window, width, items);
    }

    /// Closes the page's menu when its page is no longer the one shown.
    pub(super) fn sync_page_menu(&mut self) {
        if self.context_menus.page_open() && self.page_context.as_ref().map(|c| c.view) != self.active_view()
        {
            self.context_menus.close(&mut self.ui, &self.config);
        }
    }

    /// Runs a row of the page's menu.
    pub(super) fn page_command(&mut self, command: PageCommand) {
        let Some(PageContext { view, params, player }) = self.page_context.take() else { return };
        if Some(view) != self.active_view() {
            return;
        }
        let link = params.link_url.trim().to_owned();
        let source = params.source_url.trim().to_owned();
        let selection = params.selection_text.clone();
        match command {
            PageCommand::OpenLink => self.open_page_link(view, &link, true),
            PageCommand::OpenLinkInBackground => self.open_page_link(view, &link, false),
            PageCommand::OpenLinkInPrivate => self.open_private_tab(&link),
            PageCommand::CopyLink => self.copy_text(&link, tr("Link copied")),
            PageCommand::SaveLink => self.save_to_downloads(view, &link),
            PageCommand::OpenImage => self.open_page_link(view, &source, true),
            PageCommand::SaveImage | PageCommand::SaveMedia => self.save_to_downloads(view, &source),
            PageCommand::CopyImage => {
                self.copy_image = Some(view);
                self.engine.fetch_image(view, &source, self.config.ui.copy_image_max_px);
            }
            PageCommand::CopyImageAddress | PageCommand::CopyMediaAddress => {
                self.copy_text(&source, tr("Address copied"));
            }
            PageCommand::MediaToggle => {
                // Straight to the element (not `media_command`, which clicks site players: a
                // bare `<video>` has nothing to click).
                if let Some(player) = player {
                    let action = if player.is_playing() { MediaAction::Pause } else { MediaAction::Play };
                    let command = MediaCommand::new(&player.id, action);
                    self.engine.run_script(view, &command.script(), command.needs_gesture());
                }
            }
            PageCommand::MediaMute => {
                if let Some(player) = player {
                    self.media_command(
                        view,
                        &MediaCommand::new(&player.id, MediaAction::Mute(!player.muted)),
                    );
                }
            }
            PageCommand::MediaPictureInPicture => {
                if let Some(player) = player {
                    self.start_pip(view, player);
                }
            }
            PageCommand::MediaAddToPlaylist => self.add_player_to_playlist(view, player.as_ref()),
            PageCommand::Undo => self.engine.context_command(view, ContextCommand::Undo),
            PageCommand::Redo => self.engine.context_command(view, ContextCommand::Redo),
            // The selection reaches the desktop clipboard through ours: Chromium's can't, on
            // Wayland. Without selected text the engine's own copy has to do.
            PageCommand::Copy if !selection.trim().is_empty() => self.copy_quietly(&selection),
            PageCommand::Copy => self.engine.context_command(view, ContextCommand::Copy),
            PageCommand::Cut if !selection.trim().is_empty() => {
                self.copy_quietly(&selection);
                self.engine.context_command(view, ContextCommand::Delete);
            }
            PageCommand::Cut => self.engine.context_command(view, ContextCommand::Cut),
            PageCommand::Paste => {
                if self.engine.has_system_clipboard() {
                    self.engine.context_command(view, ContextCommand::Paste);
                } else {
                    let max = u64::from(self.config.clipboard.max_paste_mb) * 1024 * 1024;
                    self.clipboard_reader.read(view, false, max);
                }
            }
            PageCommand::SelectAll => self.engine.context_command(view, ContextCommand::SelectAll),
            PageCommand::SearchSelection => {
                let limit = self.config.ui.page_context_menu_query_chars as usize;
                let query: String = selection.split_whitespace().collect::<Vec<_>>().join(" ");
                let query: String = query.chars().take(limit).collect();
                if let Some(url) = resolve_input(&query, &self.config.general.search_url) {
                    self.open_page_link(view, &url, true);
                }
            }
            PageCommand::Back => self.run(Action::Back),
            PageCommand::Forward => self.run(Action::Forward),
            PageCommand::Reload => self.run(Action::Reload),
            PageCommand::SavePage => self.save_to_downloads(view, params.page_url.trim()),
            PageCommand::Print => self.run(Action::Print),
            PageCommand::Screenshot => self.run(Action::Screenshot),
            PageCommand::Inspect => {
                if self.config.devtools.enabled {
                    // The click in the page's own coordinates (logical px, like the engine's).
                    let (x, y) = (params.pos.x.round() as i32, params.pos.y.round() as i32);
                    tracing::info!(x, y, "inspecting the element");
                    if let Err(e) = self.engine.inspect_element(view, x, y) {
                        tracing::error!("inspecting the element failed: {e}");
                    }
                }
            }
            PageCommand::ViewSource => {
                let url = format!("view-source:{}", params.page_url.trim());
                self.open_page_link(view, &url, true);
            }
        }
    }

    /// Opens an address from the page in a new tab of the page's own group.
    fn open_page_link(&mut self, opener: ViewId, url: &str, activate: bool) {
        if url.is_empty() {
            return;
        }
        if let Err(e) = self.open_tab_from(opener, url, activate) {
            tracing::error!("opening {url} failed: {e:#}");
        }
    }

    /// Opens an address in a new private tab (in the background unless private tabs are shown).
    fn open_private_tab(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        if self.private_locked() {
            self.show_toast(Icon::Lock, tr("Private tabs are locked"));
            return;
        }
        let result = if self.private_mode {
            self.open_tab(url, false)
        } else {
            self.engine
                .create_view(url, self.view_size(), true)
                .map(|view| {
                    let mut tab = Tab::new(view, url);
                    tab.private = true;
                    tab.needs_snapshot = true;
                    self.parked.insert(tab, false);
                    self.tabs_changed = true;
                })
                .map_err(Into::into)
        };
        if let Err(e) = result {
            tracing::error!("opening {url} in a private tab failed: {e:#}");
        }
    }

    /// Downloads `url` through the page's network stack into the downloads folder.
    fn save_to_downloads(&mut self, view: ViewId, url: &str) {
        if url.is_empty() {
            return;
        }
        self.engine.start_download(view, url, downloads::download_dir(&self.config));
        self.show_toast(Icon::Download, tr("Downloading…"));
    }

    fn copy_quietly(&mut self, text: &str) {
        if let Some(clipboard) = &mut self.clipboard
            && let Err(e) = clipboard.set_text(text.to_owned())
        {
            tracing::warn!("copying failed: {e}");
        }
    }

    fn copy_text(&mut self, text: &str, toast: &str) {
        let Some(clipboard) = &mut self.clipboard else { return };
        match clipboard.set_text(text.to_owned()) {
            Ok(()) => self.show_toast(Icon::Check, toast),
            Err(e) => tracing::warn!("copying failed: {e}"),
        }
    }

    fn add_player_to_playlist(&mut self, view: ViewId, player: Option<&Player>) {
        let Some(player) = player else { return };
        let candidate = find_tab(&self.tabs, &self.parked, view)
            .and_then(|tab| self.tab_candidates(tab).into_iter().find(|c| c.player == player.id));
        match candidate {
            Some(candidate) => self.add_to_playlist(view, candidate, false),
            None => self.show_toast(Icon::Playlist, tr("No video to add")),
        }
    }

    /// The image of "Copy Image" arrived: puts it on the clipboard.
    pub(super) fn image_fetched(&mut self, view: ViewId, width: u32, height: u32, rgba: Vec<u8>) {
        if self.copy_image.take() != Some(view) {
            return;
        }
        tracing::info!(width, height, "copying the image");
        let Some(clipboard) = &mut self.clipboard else { return };
        let image = arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Owned(rgba),
        };
        match clipboard.set_image(image) {
            Ok(()) => self.show_toast(Icon::Check, tr("Image copied")),
            Err(e) => {
                tracing::warn!("copying the image failed: {e}");
                self.show_toast(Icon::Close, tr("Couldn’t copy the image"));
            }
        }
    }

    pub(super) fn image_failed(&mut self, view: ViewId) {
        if self.copy_image.take() == Some(view) {
            self.show_toast(Icon::Close, tr("Couldn’t copy the image"));
        }
    }
}
