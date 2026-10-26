//! The browser's side of the tabs' and bookmarks' context menus (`context_menu`) and of the
//! bookmarks bar: what their rows do.

use axion_config::{BookmarksBar, Config};

use super::*;
use crate::context_menu::{BookmarkCommand, TabCommand, TabMenuInfo, bookmark_items, group_items, tab_items};

/// Pages that count as the home page for `layout.bookmarks_bar = "home"`: the configured home
/// page and Axion's own new-tab pages (easy to extend when more arrive).
pub(crate) fn is_home_url(url: &str, config: &Config) -> bool {
    const OWN_HOME_PAGES: [&str; 3] = ["axion://home", "axion://newtab", "about:blank"];
    let trim = |u: &str| u.trim().trim_end_matches('/').to_ascii_lowercase();
    let url = trim(url);
    url.is_empty()
        || OWN_HOME_PAGES.iter().any(|home| url == *home)
        || (!config.general.homepage.trim().is_empty() && url == trim(&config.general.homepage))
}

impl Browser {
    /// A right or middle button press at `p` (logical px): a tab's or bookmark's menu, or a
    /// bookmark opened in a background tab. Returns whether the chrome took it.
    pub(super) fn secondary_click(&mut self, button: winit::event::MouseButton, p: Vec2) -> bool {
        use winit::event::MouseButton as Button;
        let Some(node) = self.ui.hit(p) else { return false };
        let bookmark = self
            .ui
            .hit_action(p)
            .and_then(|n| self.ui.widget(n))
            .and_then(|w| w.action)
            .and_then(ChromeAction::from_id);
        if let Some(ChromeAction::OpenBookmark(i)) = bookmark {
            match button {
                Button::Middle => self.bookmark_command(BookmarkCommand::OpenInNewTab(i)),
                Button::Right => self.open_bookmark_menu(i),
                _ => return false,
            }
            return true;
        }
        if button == Button::Right
            && !self.is_locked()
            && let Some(item) = self.chrome.tab_at(&self.ui, node)
        {
            self.open_tab_menu(item);
            return true;
        }
        if button == Button::Right
            && !self.is_locked()
            && let Some((private, header)) = self.chrome.group_header_at(&self.ui, node)
        {
            self.open_group_menu(private, header);
            return true;
        }
        false
    }

    fn close_other_menus(&mut self) {
        self.menu.close(&mut self.ui, &self.config);
        self.account_menu.close(&mut self.ui, &self.config);
    }

    /// Opens a tab's context menu, flowing out of its segment or row.
    fn open_tab_menu(&mut self, item: TabItem) {
        let group = self.group(item.private);
        let Some(index) = group.index_of(item.view) else { return };
        let tabs = group.all();
        let tab = &tabs[index];
        let info = TabMenuInfo {
            pinned: tab.pinned,
            muted: tab.muted,
            private: item.private,
            others: tabs.iter().enumerate().any(|(i, t)| i != index && !t.pinned),
            right: tabs[index + 1..].iter().any(|t| !t.pinned),
            can_reopen: group.has_closed(),
            can_bookmark: self.store.is_some(),
        };
        let items = tab_items(info, &self.keymap);
        self.close_other_menus();
        self.context_tab = Some((item.view, item.private));
        let plane = self.chrome.tab_plane(&item);
        let anchor = self.ui.frame(item.node);
        let window = self.logical_size();
        self.context_menus.open(
            &mut self.ui,
            &self.config,
            plane,
            anchor,
            window,
            Placement::BelowStart,
            items,
        );
    }

    /// Opens the menu of a sidebar group header ("N Tabs", "Private"), flowing out of the row.
    /// Its per-tab rows act on the group's active tab.
    fn open_group_menu(&mut self, private: bool, header: axion_ui::taffy::NodeId) {
        let group = self.group(private);
        let Some(tab) = group.active() else { return };
        let view = tab.view;
        let info = TabMenuInfo {
            pinned: tab.pinned,
            muted: tab.muted,
            private,
            others: group.all().iter().any(|t| !t.pinned && t.view != view),
            right: false,
            can_reopen: group.has_closed(),
            can_bookmark: self.store.is_some(),
        };
        let items = group_items(info, &self.keymap);
        self.close_other_menus();
        self.context_tab = Some((view, private));
        let plane = self.chrome.sidebar_node();
        let anchor = self.ui.frame(header);
        let window = self.logical_size();
        self.context_menus.open(
            &mut self.ui,
            &self.config,
            plane,
            anchor,
            window,
            Placement::BelowStart,
            items,
        );
    }

    /// Opens bookmark `i`'s context menu, flowing out of it in the bookmarks bar.
    fn open_bookmark_menu(&mut self, i: usize) {
        let Some(anchor) = self.chrome.bookmark_frame(&self.ui, i) else { return };
        self.close_other_menus();
        let plane = self.chrome.bookmarks_bar_node();
        let window = self.logical_size();
        let items = bookmark_items(i);
        self.context_menus.open(
            &mut self.ui,
            &self.config,
            plane,
            anchor,
            window,
            Placement::BelowStart,
            items,
        );
    }

    /// The bookmarks bar's chevron: the bookmarks that don't fit, as a menu.
    pub(super) fn toggle_bookmarks_overflow(&mut self) {
        if self.context_menus.is_open() {
            self.context_menus.close(&mut self.ui, &self.config);
            return;
        }
        let items: Vec<MenuItem> = self
            .chrome
            .bookmarks_overflow()
            .filter_map(|i| {
                let (_, url, title) = self.bar_bookmarks.get(i)?;
                let label = if title.trim().is_empty() { display_host(url) } else { title.clone() };
                Some(MenuItem::new(Icon::Globe, &label, MenuAction::Bookmark(BookmarkCommand::Open(i))))
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.close_other_menus();
        let anchor = self.chrome.bookmarks_more_frame(&self.ui);
        let plane = self.chrome.bookmarks_bar_node();
        let window = self.logical_size();
        self.context_menus.open(&mut self.ui, &self.config, plane, anchor, window, Placement::Below, items);
    }

    pub(super) fn bookmark_command(&mut self, command: BookmarkCommand) {
        let index = match command {
            BookmarkCommand::Open(i) | BookmarkCommand::OpenInNewTab(i) | BookmarkCommand::Delete(i) => i,
        };
        let Some((id, url, _)) = self.bar_bookmarks.get(index).cloned() else { return };
        match command {
            BookmarkCommand::Open(_) => self.navigate_to(url),
            BookmarkCommand::OpenInNewTab(_) => {
                if self.is_locked() {
                    return;
                }
                if let Err(e) = self.open_tab(&url, false) {
                    tracing::error!("opening a bookmark in a new tab failed: {e:#}");
                }
            }
            BookmarkCommand::Delete(_) => {
                if let Some(store) = &self.store
                    && let Err(e) = store.remove_bookmark(id)
                {
                    tracing::error!("removing a bookmark failed: {e}");
                }
                self.bookmarks_changed = true;
            }
        }
    }

    /// Shows the tab group `private` (the menu's tab may be in the one not shown).
    fn show_group(&mut self, private: bool) {
        if private != self.private_mode {
            self.toggle_private_mode();
        }
    }

    /// Opens `url` in a new tab of the shown group at `index` (kept after the pinned tabs).
    fn open_tab_at(&mut self, url: &str, index: usize, activate: bool) {
        let view = match self.engine.create_view(url, self.view_size(), self.private_mode) {
            Ok(view) => view,
            Err(e) => {
                tracing::error!("opening a new tab failed: {e:#}");
                return;
            }
        };
        let previous = self.active_view();
        let mut tab = Tab::new(view, url);
        tab.private = self.private_mode;
        tab.needs_snapshot = !activate;
        self.tabs.insert_at(index, tab, activate);
        if activate {
            self.on_active_changed(previous);
        }
        self.tabs_changed = true;
    }

    /// Closes `view`'s tab in either group.
    fn close_tab_in(&mut self, private: bool, view: ViewId) {
        if private == self.private_mode {
            if let Some(i) = self.tabs.index_of(view) {
                self.close_tab(i);
            }
            return;
        }
        let Some(i) = self.parked.index_of(view) else { return };
        if self.pip_view() == Some(view) {
            self.end_pip(view, None, false);
        }
        if self.now_playing == Some(view) {
            self.now_playing = None;
        }
        if self.parked.remove(i).is_some() {
            self.engine.close_view(view);
        }
        self.tabs_changed = true;
    }

    pub(super) fn tab_command(&mut self, command: TabCommand) {
        let Some((view, private)) = self.context_tab.take() else { return };
        let Some(index) = self.group(private).index_of(view) else { return };
        let tab = self.group(private).all()[index].clone();
        match command {
            TabCommand::NewTabToTheRight => {
                self.show_group(private);
                let home = self.config.general.homepage.clone();
                self.open_tab_at(&home, index + 1, true);
                self.edit_address();
            }
            TabCommand::Reload => self.engine.reload(view, false),
            TabCommand::Duplicate => {
                self.show_group(private);
                self.open_tab_at(&tab.url, index + 1, true);
            }
            TabCommand::Pin | TabCommand::Unpin => {
                self.group_mut(private).set_pinned(index, command == TabCommand::Pin);
                self.tabs_changed = true;
            }
            TabCommand::Mute | TabCommand::Unmute => {
                let muted = command == TabCommand::Mute;
                if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                    tab.muted = muted;
                }
                self.engine.set_audio_muted(view, muted);
                self.tabs_changed = true;
            }
            TabCommand::Close => self.close_tab_in(private, view),
            TabCommand::CloseOthers => {
                // Pinned tabs stay.
                let views: Vec<ViewId> = self
                    .group(private)
                    .all()
                    .iter()
                    .filter(|t| !t.pinned && t.view != view)
                    .map(|t| t.view)
                    .collect();
                for other in views {
                    self.close_tab_in(private, other);
                }
            }
            TabCommand::ReopenClosed => {
                self.show_group(private);
                self.run(Action::ReopenClosedTab);
            }
            TabCommand::Bookmark => {
                if let Some(store) = &self.store
                    && store.bookmark_for(&tab.url).ok().flatten().is_none()
                    && let Err(e) = store.add_bookmark(&tab.url, &tab.label(), unix_now())
                {
                    tracing::error!("adding a bookmark failed: {e}");
                }
                self.bookmarks_changed = true;
            }
            TabCommand::BookmarkAll => {
                let now = unix_now();
                if let Some(store) = &self.store {
                    for t in self.group(private).all() {
                        if store.bookmark_for(&t.url).ok().flatten().is_some() {
                            continue;
                        }
                        if let Err(e) = store.add_bookmark(&t.url, &t.label(), now) {
                            tracing::error!("adding a bookmark failed: {e}");
                        }
                    }
                }
                self.bookmarks_changed = true;
            }
            TabCommand::MoveToPrivate | TabCommand::MoveToNormal => {
                // Like dropping it on the other group's list (at its end).
                let end = self.group(!private).len();
                match crate::private::drop_into(private, !private, self.private_locked()) {
                    TabDrop::Reopen => {
                        self.move_to_other_group(private, index, end);
                    }
                    TabDrop::NeedsUnlock => {
                        self.pending_move = Some((view, end));
                        self.show_unlock_alert();
                    }
                    TabDrop::Reorder => {}
                }
            }
        }
    }

    /// Reloads the bar's bookmarks when they changed, refills the bar (when they or the tabs
    /// changed: favicons arrive with them) and shows or hides it (`layout.bookmarks_bar`). The
    /// page shrinks to make room for it.
    pub(super) fn sync_bookmarks_bar(&mut self, tabs_changed: bool) {
        let reload = std::mem::take(&mut self.bookmarks_changed);
        if reload {
            self.bar_bookmarks = self
                .store
                .as_ref()
                .and_then(|s| s.bookmarks().ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|b| b.folder.is_none())
                .map(|b| (b.id, b.url, b.title))
                .collect();
        }
        if reload || tabs_changed {
            let items: Vec<(String, String)> =
                self.bar_bookmarks.iter().map(|(_, url, title)| (url.clone(), title.clone())).collect();
            self.chrome.set_bookmarks(&mut self.ui, &items, &self.favicons);
        }
        // Nothing to show: no bar (it appears with the first bookmark).
        let shown = !self.is_locked()
            && !self.bar_bookmarks.is_empty()
            && match self.config.layout.bookmarks_bar {
                BookmarksBar::Always => true,
                BookmarksBar::Home => self.tabs.active().is_some_and(|t| is_home_url(&t.url, &self.config)),
                BookmarksBar::Never => false,
            };
        if self.chrome.set_bookmarks_bar_shown(&mut self.ui, shown) {
            self.update_layout();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_pages() {
        let mut config = Config::default();
        config.general.homepage = "https://start.example/".into();
        assert!(is_home_url("https://start.example", &config));
        assert!(is_home_url("https://start.example/", &config));
        assert!(is_home_url("axion://home", &config));
        assert!(is_home_url("", &config));
        assert!(!is_home_url("https://start.example/news", &config));
        assert!(!is_home_url("https://other.example/", &config));
    }
}
