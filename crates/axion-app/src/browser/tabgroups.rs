//! The browser's side of tab groups (named, coloured runs of tabs), tab search and the tabs'
//! speakers: what their menus, sheets and clicks do. The model is in `crate::tabs`, the rows
//! in `crate::chrome`.

use std::collections::HashSet;

use super::*;
use crate::context_menu::{GroupChoice, GroupCommand, tab_group_items};
use crate::group_sheet::{GroupSheetAction, GroupSheetPurpose};
use crate::tab_search::{ResultKind, SearchResult, search};
use crate::tabs::GroupColor;

/// The site-settings key of a muted site.
const MUTED_KEY: &str = "muted";

/// The sites muted by "Mute Site", from the profile.
pub(super) fn load_muted_sites(store: Option<&axion_store::Store>) -> HashSet<String> {
    store
        .and_then(|s| s.site_settings(MUTED_KEY).ok())
        .map(|rows| rows.into_iter().filter(|(_, v)| v == "1").map(|(host, _)| host).collect())
        .unwrap_or_default()
}

impl Browser {
    // ---- options ----

    /// Hands the tab settings to the tab lists and the tab model; true when the lists must be
    /// rebuilt for them.
    pub(super) fn sync_tab_options(&mut self) -> bool {
        let t = &self.config.tabs;
        let changed = self.chrome.set_tab_options(crate::chrome::TabOptions {
            audio_indicator: t.show_audio_indicator,
            group_chips: t.group_chips_in_bar,
        });
        for tabs in [&mut self.tabs, &mut self.parked] {
            tabs.inherit_group = t.new_tab_joins_group;
            tabs.set_closed_limit(t.closed_tabs_remembered as usize);
        }
        changed
    }

    // ---- tab groups ----

    /// The groups a tab can be moved into (the normal tabs'), for its menu.
    pub(super) fn group_choices(&self, current: Option<u32>) -> Vec<GroupChoice> {
        self.group(false)
            .groups()
            .iter()
            .map(|g| GroupChoice { id: g.id, name: g.name.clone(), color: g.color, current: current == Some(g.id) })
            .collect()
    }

    /// Opens the sheet that names a tab group.
    pub(super) fn open_group_sheet(&mut self, purpose: GroupSheetPurpose) {
        let (name, color) = match purpose {
            GroupSheetPurpose::New(_) => (String::new(), self.group(false).next_group_color()),
            GroupSheetPurpose::Edit(id) => match self.group(false).group(id) {
                Some(g) => (g.name.clone(), g.color),
                None => return,
            },
        };
        self.close_other_menus();
        self.context_menus.close(&mut self.ui, &self.config);
        let events = self.group_sheet.open(&mut self.ui, &self.config, purpose, &name, color);
        self.handle_ui_events(events);
    }

    pub(super) fn group_sheet_action(&mut self, action: GroupSheetAction) {
        match action {
            GroupSheetAction::Color(color) => self.group_sheet.set_color(&mut self.ui, color),
            GroupSheetAction::Cancel => self.group_sheet.close(&mut self.ui),
            GroupSheetAction::Save => {
                let Some(purpose) = self.group_sheet.purpose() else { return };
                let name = self.group_sheet.name(&self.ui);
                let color = self.group_sheet.color();
                self.group_sheet.close(&mut self.ui);
                let tabs = self.group_mut(false);
                match purpose {
                    GroupSheetPurpose::New(view) => {
                        if let Some(index) = tabs.index_of(view) {
                            let id = tabs.create_group(&name, color);
                            tabs.set_tab_group(index, Some(id));
                        }
                    }
                    GroupSheetPurpose::Edit(id) => {
                        if let Some(group) = tabs.group_mut(id) {
                            group.name = name;
                            group.color = color;
                        }
                    }
                }
                self.tabs_changed = true;
            }
        }
    }

    /// Folds a tab group's tabs away, or brings them back.
    pub(super) fn toggle_tab_group(&mut self, id: u32) {
        if let Some(group) = self.group_mut(false).group_mut(id) {
            group.collapsed = !group.collapsed;
            self.tabs_changed = true;
        }
    }

    /// Opens the context menu of a tab group's header (sidebar) or chip (bar), flowing out of it.
    pub(super) fn open_tab_group_menu(&mut self, id: u32, node: axion_ui::taffy::NodeId, in_sidebar: bool) {
        let Some(group) = self.group(false).group(id).cloned() else { return };
        self.close_other_menus();
        self.context_tab = None;
        self.context_row = None;
        self.context_group = Some(id);
        let plane = if in_sidebar {
            self.chrome.sidebar_node()
        } else if self.config.layout.tab_bar == axion_config::TabBarStyle::Separate {
            self.chrome.tab_strip_node()
        } else {
            self.chrome.bar_node()
        };
        let anchor = self.ui.frame(node);
        let window = self.logical_size();
        self.context_menus.open(
            &mut self.ui,
            &self.config,
            plane,
            anchor,
            window,
            if in_sidebar { Placement::Over } else { Placement::BelowStart },
            tab_group_items(&group),
            in_sidebar.then_some(node),
        );
    }

    pub(super) fn tab_group_command(&mut self, command: GroupCommand) {
        let Some(id) = self.context_group.take() else { return };
        match command {
            GroupCommand::NewTab => {
                let Some(end) = self.group(false).group_range(id).map(|r| r.end) else { return };
                self.show_group(false);
                let home = self.config.general.effective_homepage().to_owned();
                self.open_tab_at(&home, end, true);
                // Even with "new tabs join the group they open from" off, this one does.
                let view = self.active_view();
                if let Some(index) = view.and_then(|v| self.tabs.index_of(v)) {
                    self.tabs.set_tab_group(index, Some(id));
                }
                self.edit_address();
            }
            GroupCommand::Rename => self.open_group_sheet(GroupSheetPurpose::Edit(id)),
            GroupCommand::SetColor(color) => {
                if let Some(group) = self.group_mut(false).group_mut(id) {
                    group.color = color;
                }
            }
            GroupCommand::Ungroup => self.group_mut(false).ungroup(id),
            GroupCommand::Close => {
                for view in self.group(false).group_views(id) {
                    self.close_tab_in(false, view);
                }
            }
        }
        self.tabs_changed = true;
    }

    // ---- sound ----

    /// Mutes or unmutes one tab (by hand: it is no longer muted by its site's setting).
    pub(super) fn set_tab_muted(&mut self, view: ViewId, muted: bool) {
        if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
            tab.muted = muted;
            tab.site_muted = false;
            self.engine.set_audio_muted(view, muted);
            self.tabs_changed = true;
        }
    }

    /// A tab's speaker was clicked: the `index`th tab of the shown list (`other`: of the other).
    pub(super) fn toggle_tab_mute_at(&mut self, other: bool, index: usize) {
        let tabs = if other { &self.parked } else { &self.tabs };
        if let Some(tab) = tabs.all().get(index) {
            let (view, muted) = (tab.view, !tab.muted);
            self.set_tab_muted(view, muted);
        }
    }

    /// Whether the site of `url` is muted by "Mute Site".
    pub(super) fn site_is_muted(&self, url: &str) -> bool {
        axion_store::site_host(url).is_some_and(|h| self.muted_sites.contains(&h))
    }

    /// "Mute Site" / "Unmute Site": every tab of the site, and (if remembered) the site.
    pub(super) fn set_site_muted(&mut self, url: &str, muted: bool) {
        let Some(host) = axion_store::site_host(url) else { return };
        if self.config.tabs.remember_site_mute {
            if muted {
                self.muted_sites.insert(host.clone());
            } else {
                self.muted_sites.remove(&host);
            }
            if let Some(store) = &self.store
                && let Err(e) = store.set_site_setting(&host, MUTED_KEY, muted.then_some("1"))
            {
                tracing::warn!("saving a muted site failed: {e}");
            }
        }
        let mut views = Vec::new();
        for tabs in [&mut self.tabs, &mut self.parked] {
            for tab in tabs.all_mut() {
                if axion_store::site_host(&tab.url).as_deref() == Some(host.as_str()) {
                    tab.muted = muted;
                    tab.site_muted = muted;
                    views.push(tab.view);
                }
            }
        }
        for view in views {
            self.engine.set_audio_muted(view, muted);
        }
        self.tabs_changed = true;
    }

    /// A tab went to another address: a muted site's tab starts muted, and a tab muted by its
    /// site's setting is let go again when it leaves the site.
    pub(super) fn apply_site_mute(&mut self, view: ViewId) {
        let Some(tab) = find_tab(&self.tabs, &self.parked, view) else { return };
        let (muted, by_site) = (tab.muted, tab.site_muted);
        let site_muted = self.config.tabs.remember_site_mute && self.site_is_muted(&tab.url);
        if site_muted && !muted {
            if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                tab.muted = true;
                tab.site_muted = true;
            }
            self.engine.set_audio_muted(view, true);
        } else if !site_muted && by_site {
            if let Some(tab) = tab_mut(&mut self.tabs, &mut self.parked, view) {
                tab.muted = false;
                tab.site_muted = false;
            }
            self.engine.set_audio_muted(view, false);
        }
    }

    // ---- tab search ----

    pub(super) fn toggle_tab_search(&mut self) {
        if self.tab_search.is_open() {
            self.tab_search.close(&mut self.ui);
            return;
        }
        self.menu.close(&mut self.ui, &self.config);
        self.account_menu.close(&mut self.ui, &self.config);
        self.context_menus.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.close_find();
        self.chrome.set_editing(&mut self.ui, false);
        let events = self.tab_search.open(&mut self.ui, &self.config);
        self.handle_ui_events(events);
        self.tab_search_query = None;
        self.refresh_tab_search();
    }

    /// Searches again when the field changed. Call every frame.
    pub(super) fn poll_tab_search(&mut self) {
        if !self.tab_search.is_open() {
            return;
        }
        let query = self.tab_search.query(&self.ui);
        if self.tab_search_query.as_deref() != Some(query.as_str()) {
            self.refresh_tab_search();
        }
    }

    fn refresh_tab_search(&mut self) {
        let query = self.tab_search.query(&self.ui);
        // Private browsing searches its own tabs only: no history, no closed tabs of the
        // normal ones.
        let private = self.private_mode;
        let closed: Vec<(String, String)> = if private {
            Vec::new()
        } else {
            self.tabs.recently_closed().map(|(u, t)| (u.to_owned(), t.to_owned())).collect()
        };
        let store = if private { None } else { self.store.as_ref() };
        let limit = self.config.tabs.tab_search_history_results as usize;
        let results = search(&query, &[(&self.tabs, private)], &closed, store, limit, unix_now());
        self.tab_search.set_results(&mut self.ui, results, &self.favicons);
        self.tab_search_query = Some(query);
    }

    /// Opens a search result: switches to the tab, or opens the page in a new tab.
    pub(super) fn open_search_result(&mut self, result: SearchResult) {
        self.tab_search.close(&mut self.ui);
        match result.kind {
            ResultKind::Tab { view, private } => {
                if let Some(index) = self.group(private).index_of(view) {
                    self.show_group(private);
                    self.select_tab(index);
                }
            }
            ResultKind::Closed | ResultKind::History => {
                if result.kind == ResultKind::Closed {
                    self.group_mut(false).forget_closed(&result.url);
                }
                if let Err(e) = self.open_tab(&result.url, true) {
                    tracing::error!("opening a new tab failed: {e:#}");
                }
            }
        }
    }

    /// The selected result was chosen (Enter), or row `row` clicked.
    pub(super) fn choose_search_result(&mut self, row: Option<usize>) {
        let result = match row {
            Some(row) => self.tab_search.result(row).cloned(),
            None => self.tab_search.selected().cloned(),
        };
        if let Some(result) = result {
            self.open_search_result(result);
        }
    }

    // ---- session ----

    /// The saved groups for the session.
    pub(super) fn session_groups(tabs: &Tabs) -> Vec<axion_store::SessionGroup> {
        tabs.groups()
            .iter()
            .map(|g| axion_store::SessionGroup {
                id: g.id,
                name: g.name.clone(),
                color: g.color.key().to_owned(),
                collapsed: g.collapsed,
            })
            .collect()
    }

    /// Brings back the saved groups that still have tabs.
    pub(super) fn restore_groups(&mut self, saved: Vec<axion_store::SessionGroup>) {
        for g in saved {
            if self.tabs.all().iter().any(|t| t.group == Some(g.id)) {
                self.tabs.restore_group(crate::tabs::TabGroup {
                    id: g.id,
                    name: g.name,
                    color: GroupColor::from_key(&g.color).unwrap_or(GroupColor::Blue),
                    collapsed: g.collapsed,
                });
            }
        }
        // Tabs of a group that was not saved (or dropped) are ungrouped.
        let known: Vec<u32> = self.tabs.groups().iter().map(|g| g.id).collect();
        for tab in self.tabs.all_mut() {
            if tab.group.is_some_and(|g| !known.contains(&g)) {
                tab.group = None;
            }
        }
    }
}
