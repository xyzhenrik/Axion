//! The browser's side of the context registry (`context_registry`): what a right click (or the
//! Menu key) means at a point, the facts the item providers need, and what a chosen row does.

use super::*;
use crate::context_registry::{self, Facts, ListKind, PlayerFacts, SidebarEntry, ToolbarButton, Verb};
use crate::downloads::RowCommand;
use crate::list_sheet::SheetKind;

type MenuTarget = context_registry::Target;

impl Browser {
    /// What the registry has a menu for at `p` (logical px), if anything.
    pub(super) fn menu_target_at(&self, p: Vec2) -> Option<MenuTarget> {
        if !self.config.general.context_menus || self.is_locked() {
            return None;
        }
        // Sheets lie over everything: their rows, or nothing.
        for kind in [SheetKind::History, SheetKind::Downloads] {
            let sheet = self.sheet(kind);
            if sheet.is_open() {
                let i = sheet.row_at(&self.ui, p)?;
                let url = sheet.item_url(i).unwrap_or_default().to_owned();
                let list = if kind == SheetKind::History { ListKind::History } else { ListKind::Downloads };
                return Some(MenuTarget::Row { list, index: i, url });
            }
        }
        if self.settings.is_open() {
            return self.settings.setting_at(&self.ui, p).map(MenuTarget::Setting);
        }
        if self.passwords.is_open() || self.overview.is_open() {
            return None;
        }
        // A tile of the native start page.
        if let Some(target) = self.start_tile_target(p) {
            return Some(target);
        }
        let hit = self.ui.hit(p)?;
        let action_node = self.ui.hit_action(p);
        let action = action_node.and_then(|n| self.ui.widget(n)).and_then(|w| w.action);
        // The open extensions list: its rows.
        if self.extensions_popover.is_open()
            && let Some(
                ExtensionsAction::Activate(i) | ExtensionsAction::Details(i) | ExtensionsAction::Toggle(i),
            ) = action.and_then(|a| self.extensions_popover.action(a))
        {
            return Some(MenuTarget::Extension(i));
        }
        // The media player: its controls, or the video behind them while it is fullscreen.
        let on_control = action.is_some_and(|a| PlayerControl::from_id(a).is_some());
        let fullscreen_video = self.player.mode() == PlayerMode::Fullscreen
            && self.player.is_shown()
            && self.page_rect().contains(p);
        if (on_control || fullscreen_video) && self.player.target().is_some() {
            return Some(MenuTarget::Player);
        }
        match action.and_then(ChromeAction::from_id) {
            Some(ChromeAction::Bookmarks) => Some(MenuTarget::Sidebar(SidebarEntry::Bookmarks)),
            Some(ChromeAction::ReadingList) => Some(MenuTarget::Sidebar(SidebarEntry::ReadingList)),
            Some(ChromeAction::History) => Some(MenuTarget::Sidebar(SidebarEntry::History)),
            Some(ChromeAction::Playlist) => Some(MenuTarget::Sidebar(SidebarEntry::Playlist)),
            Some(ChromeAction::Downloads) => Some(MenuTarget::Sidebar(SidebarEntry::Downloads)),
            Some(ChromeAction::Passwords) => Some(MenuTarget::Sidebar(SidebarEntry::Passwords)),
            Some(ChromeAction::Network) => Some(MenuTarget::Network),
            Some(
                ChromeAction::OpenPanelItem(i)
                | ChromeAction::DeletePanelItem(i)
                | ChromeAction::PanelButton(i, _),
            ) => self.panel_row_target(i),
            _ => {
                if let Some(button) = self.chrome.toolbar_button_at(&self.ui, hit) {
                    return Some(MenuTarget::Toolbar(button));
                }
                (action.is_none() && self.chrome.in_bar(&self.ui, hit)).then_some(MenuTarget::ToolbarBar)
            }
        }
    }

    /// Row `i` of the open sidebar list.
    fn panel_row_target(&self, index: usize) -> Option<MenuTarget> {
        let (panel, _) = self.panel?;
        let list = match panel {
            Panel::Bookmarks => ListKind::Bookmarks,
            Panel::History => ListKind::History,
            Panel::Downloads => ListKind::Downloads,
            Panel::ReadingList => return None,
        };
        let url = self.panel_urls.get(index).cloned().unwrap_or_default();
        Some(MenuTarget::Row { list, index, url })
    }

    /// The facts the item providers need about `target`.
    fn menu_facts(&self, target: &MenuTarget) -> Facts {
        let mut facts = Facts {
            private_ok: !self.private_locked(),
            has_store: self.store.is_some(),
            downloads_finished: self.downloads.has_finished()
                || self.store.as_ref().is_some_and(|s| s.downloads(1).is_ok_and(|d| !d.is_empty())),
            has_password: self.passwords.has_password(),
            playlist_ok: self.store.is_some() && self.tabs.active().is_some_and(|t| !t.private),
            hidden_buttons: self.config.layout.hidden_toolbar_buttons.clone(),
            home_hidden: !self.config.layout.home_button,
            playback_speeds: self.config.media.playback_speeds.clone(),
            ..Facts::default()
        };
        match target {
            MenuTarget::Row { list: ListKind::Downloads, index, .. } => {
                facts.download = self.downloads.row_facts(List::Panel, *index);
            }
            MenuTarget::Setting(setting) => {
                facts.setting_is_default = self.settings.is_default(*setting, &self.config);
            }
            MenuTarget::Extension(i) => {
                facts.extension_enabled = self.extensions.views().get(*i).is_some_and(|e| e.enabled);
            }
            MenuTarget::Player => {
                facts.player = self.player.target().map(|(_, p)| PlayerFacts {
                    paused: p.paused,
                    muted: p.muted,
                    fullscreen: self.player.mode() == PlayerMode::Fullscreen,
                    rate: p.rate,
                    has_captions: !p.captions.is_empty(),
                });
            }
            _ => {}
        }
        facts
    }

    /// Opens the menu of `target` dripping out of `at` (logical px).
    pub(super) fn open_registry_menu(&mut self, target: MenuTarget, at: Vec2) {
        let facts = self.menu_facts(&target);
        let items = context_registry::items(&target, &facts, &self.keymap);
        if items.is_empty() {
            return;
        }
        self.close_other_menus();
        self.context_tab = None;
        self.context_row = None;
        self.page_context = None;
        self.tooltip.cancel(&mut self.ui);
        let window = self.logical_size();
        let width = crate::page_menu::menu_width(&items, window);
        let anchor = crate::page_menu::anchor(at, window, width);
        self.menu_target = Some(target);
        self.context_menus.open_overlay(&mut self.ui, &self.config, anchor, window, width, items);
    }

    /// A right click at `p` on something the registry knows. Returns whether it opened a menu.
    pub(super) fn registry_click(&mut self, p: Vec2) -> bool {
        let target = self.menu_target_at(p);
        tracing::debug!(?p, ?target, "context menu target");
        match target {
            Some(target) => {
                self.open_registry_menu(target, p);
                true
            }
            None => false,
        }
    }

    /// The Menu key and Shift+F10: the menu of what the pointer is over (the same as a right
    /// click there); without a pointer over the chrome, the current tab's menu out of the address
    /// field. Returns false when the pointer is over the page (the key goes on to it).
    pub(super) fn open_menu_from_keyboard(&mut self) -> bool {
        if self.is_locked() || self.ui.focused().is_some() {
            return false;
        }
        if let Some(p) = self.pointer {
            let over_page = self.page_rect().contains(p) && self.player.mode() != PlayerMode::Fullscreen;
            if over_page {
                return false;
            }
            if self.secondary_click(winit::event::MouseButton::Right, p) {
                return true;
            }
        }
        if self.config.tabs.address_context_menu
            && let Some(view) = self.group(self.private_mode).active().map(|t| t.view)
        {
            self.open_address_menu(view);
            return true;
        }
        false
    }

    /// The tab overview's card `index`: the tab's menu (as in the tab strip) over the overview.
    pub(super) fn open_card_menu(&mut self, index: usize, at: Vec2) {
        let Some(view) = self.tabs.all().get(index).map(|t| t.view) else { return };
        let private = self.private_mode;
        let Some((info, tab_group)) = self.tab_menu_info(private, view) else { return };
        let choices = self.group_choices(tab_group);
        let items = crate::context_menu::tab_items(info, &choices, &self.keymap);
        self.close_other_menus();
        self.context_tab = Some((view, private));
        self.context_row = None;
        self.menu_target = None;
        let window = self.logical_size();
        let width = crate::page_menu::menu_width(&items, window);
        let anchor = crate::page_menu::anchor(at, window, width);
        self.context_menus.open_overlay(&mut self.ui, &self.config, anchor, window, width, items);
    }

    /// The new tab's address for a row's open commands.
    fn open_in_tab(&mut self, url: &str, private: bool, background: bool) {
        if url.is_empty() {
            return;
        }
        if private {
            self.open_private_tab(url);
        } else if let Err(e) = self.open_tab(url, !background) {
            tracing::error!("opening {url} failed: {e:#}");
        }
    }

    /// Runs the chosen row of the registry's menu on the target it was opened for.
    pub(super) fn context_command(&mut self, verb: Verb) {
        let Some(target) = self.menu_target.take() else { return };
        match target {
            MenuTarget::Sidebar(entry) => self.sidebar_command(entry, verb),
            MenuTarget::Row { list, index, url } => self.row_command(list, index, &url, verb),
            MenuTarget::Toolbar(button) => self.toolbar_command(Some(button), verb),
            MenuTarget::ToolbarBar => self.toolbar_command(None, verb),
            MenuTarget::Setting(setting) => {
                if verb == Verb::ResetSetting {
                    let outcome = self.settings.reset(setting, &mut self.config);
                    self.settings_outcome(outcome);
                }
            }
            MenuTarget::Extension(i) => match verb {
                Verb::ExtensionDetails | Verb::ExtensionRemove => {
                    self.extensions_action(ExtensionsAction::Details(i));
                }
                Verb::ExtensionToggle => self.extensions_action(ExtensionsAction::Toggle(i)),
                _ => {}
            },
            MenuTarget::Network => match verb {
                Verb::NetworkToggle => self.run(Action::ToggleNetwork),
                Verb::NetworkSettings => self.open_settings_page(Category::Network),
                Verb::HideNetworkChip => {
                    self.config.network.show_chip = false;
                    self.settings_outcome(Outcome::Changed);
                }
                _ => {}
            },
            MenuTarget::Tile { url, pinned } => self.tile_command(&url, pinned, verb),
            MenuTarget::Player => match verb {
                Verb::Player(control) => self.player_control(control),
                Verb::CopyPlayerAddress => {
                    let src = self.player.target().map(|(_, p)| p.src.clone()).unwrap_or_default();
                    self.copy_text(&src, tr("Address copied"));
                }
                _ => {}
            },
        }
    }

    fn open_settings_page(&mut self, category: Category) {
        self.settings.set_category(category);
        if self.settings.is_open() {
            self.refresh_settings();
        } else {
            self.run(Action::OpenSettings);
        }
    }

    fn sidebar_command(&mut self, entry: SidebarEntry, verb: Verb) {
        match (entry, verb) {
            (SidebarEntry::Bookmarks, Verb::Open) => self.show_panel(Panel::Bookmarks),
            (SidebarEntry::ReadingList, Verb::Open) => self.show_panel(Panel::ReadingList),
            (SidebarEntry::History, Verb::Open) => self.show_list(SheetKind::History),
            (SidebarEntry::History, Verb::ClearList) => {
                // The list's own clear button asks before it clears: open it armed.
                self.show_list(SheetKind::History);
                if self.history_sheet.is_open() {
                    self.list_sheet_click(SheetKind::History, crate::list_sheet::Click::Clear);
                } else {
                    self.clear_panel();
                }
            }
            (SidebarEntry::Downloads, Verb::Open) => self.show_list(SheetKind::Downloads),
            (SidebarEntry::Downloads, Verb::ClearList) => {
                Downloads::clear_list(self.store.as_ref());
                self.refresh_lists();
            }
            (SidebarEntry::Downloads, Verb::OpenDownloadsFolder) => {
                downloads::open_path(&downloads::download_dir(&self.config));
            }
            (SidebarEntry::Playlist, Verb::Open) => {
                if self.playlist_open() {
                    self.close_playlist();
                } else {
                    self.open_playlist();
                }
            }
            (SidebarEntry::Playlist, Verb::AddToPlaylist) => self.run(Action::PlaylistAdd),
            (SidebarEntry::Passwords, Verb::Open) => self.open_passwords(),
            (SidebarEntry::Passwords, Verb::LockPasswords) => self.run(Action::Lock),
            _ => {}
        }
    }

    fn row_command(&mut self, list: ListKind, index: usize, url: &str, verb: Verb) {
        if list == ListKind::Downloads {
            let command = match verb {
                Verb::OpenFile => Some(RowCommand::Open),
                Verb::ShowInFolder => Some(RowCommand::ShowInFolder),
                Verb::Pause => Some(RowCommand::Pause),
                Verb::Resume => Some(RowCommand::Resume),
                Verb::Cancel => Some(RowCommand::Cancel),
                Verb::RemoveFromList => Some(RowCommand::Remove),
                _ => None,
            };
            if let Some(command) = command {
                self.downloads.row_command(
                    List::Panel,
                    index,
                    command,
                    self.store.as_ref(),
                    &mut self.engine,
                );
                self.refresh_lists();
            } else if verb == Verb::CopyLink {
                let source = self.downloads.row_facts(List::Panel, index).map(|f| f.url).unwrap_or_default();
                self.copy_text(&source, tr("Link copied"));
            }
            return;
        }
        match verb {
            Verb::Open => {
                self.close_list_sheet(SheetKind::History);
                self.navigate_to(url.to_owned());
            }
            Verb::OpenInNewTab => self.open_in_tab(url, false, false),
            Verb::OpenInPrivateTab => self.open_in_tab(url, true, false),
            Verb::CopyLink => self.copy_text(url, tr("Link copied")),
            Verb::Delete => {
                if let Some(store) = &self.store {
                    let result = if list == ListKind::History {
                        store.delete_history_url(url)
                    } else {
                        store.bookmark_for(url).ok().flatten().map_or(Ok(()), |id| store.remove_bookmark(id))
                    };
                    if let Err(e) = result {
                        tracing::error!("deleting the entry failed: {e}");
                    }
                }
                self.bookmarks_changed = true;
                self.refresh_lists();
            }
            _ => {}
        }
    }

    fn toolbar_command(&mut self, button: Option<ToolbarButton>, verb: Verb) {
        match (button, verb) {
            (Some(ToolbarButton::Downloads), Verb::Open) => self.show_all_downloads(),
            (Some(ToolbarButton::Downloads), Verb::OpenDownloadsFolder) => {
                downloads::open_path(&downloads::download_dir(&self.config));
            }
            (Some(ToolbarButton::Extensions), Verb::Open) => self.toggle_extensions_popover(),
            (Some(ToolbarButton::Share), Verb::CopyLink) => self.copy_link(),
            (Some(button), Verb::HideButton) => {
                match button.config_name() {
                    Some(name) => {
                        let hidden = &mut self.config.layout.hidden_toolbar_buttons;
                        if !hidden.iter().any(|b| b == name) {
                            hidden.push(name.to_owned());
                        }
                    }
                    None => self.config.layout.home_button = false,
                }
                self.settings_outcome(Outcome::Changed);
            }
            (_, Verb::ShowHiddenButtons) => {
                self.config.layout.hidden_toolbar_buttons.clear();
                self.config.layout.home_button = true;
                self.settings_outcome(Outcome::Changed);
            }
            (_, Verb::CustomiseToolbar) => self.open_settings_page(Category::Appearance),
            _ => {}
        }
    }

    /// Opens the start page tile's menu (the page's right click landed on a tile).
    pub(super) fn show_tile_menu(&mut self, url: &str, at: Vec2) -> bool {
        if !self.config.general.context_menus {
            return false;
        }
        let Some(store) = &self.store else { return false };
        let home = &self.config.home;
        let (sites, _) = crate::home::top_sites(
            store,
            home.top_sites_count as usize,
            home.top_sites_window_days,
            unix_now(),
        );
        let host = axion_store::site_host(url);
        let Some(site) =
            sites.iter().find(|s| s.url == url || (host.is_some() && Some(&s.host) == host.as_ref()))
        else {
            return false;
        };
        let target = MenuTarget::Tile { url: site.url.clone(), pinned: site.pinned };
        self.open_registry_menu(target, at);
        true
    }

    fn tile_command(&mut self, url: &str, pinned: bool, verb: Verb) {
        let message = |kind: &str| serde_json::json!({ "type": kind, "url": url, "title": "" }).to_string();
        let Some(view) = self.active_view() else { return };
        match verb {
            Verb::Open => self.navigate_to(url.to_owned()),
            Verb::OpenInNewTab => self.open_in_tab(url, false, false),
            Verb::OpenInPrivateTab => self.open_in_tab(url, true, false),
            Verb::CopyLink => self.copy_text(url, tr("Link copied")),
            Verb::PinTile | Verb::UnpinTile => {
                let kind = if pinned { "unpinSite" } else { "pinSite" };
                self.internal_message(view, "home", &message(kind));
            }
            Verb::RemoveTile => self.internal_message(view, "home", &message("removeSite")),
            _ => {}
        }
    }

    /// A control of the media player, from its menu or its buttons.
    pub(super) fn player_control(&mut self, control: PlayerControl) {
        match self.player.click(&mut self.ui, control, &self.config) {
            PlayerOutcome::Command(view, command) => self.media_command(view, &command),
            PlayerOutcome::PictureInPicture(view, player) => self.start_pip(view, player),
            PlayerOutcome::AddToPlaylist(view, player) => self.playlist_add_player(view, &player),
            PlayerOutcome::Nothing => {}
        }
    }
}
