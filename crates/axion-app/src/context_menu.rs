//! Context menus: right-clicking a tab (separate tab bar, compact capsule, sidebar row) or a
//! bookmark of the bookmarks bar opens a glass [`Menu`] that flows out of it. Each menu lives
//! in the plane of what was clicked, so its glass merges with the glass there (see
//! [`crate::popover`]); only one is open at a time. Rows that don't apply are left out, and
//! shortcuts are shown from the keymap.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Action, Config, Keymap};
use axion_i18n::tr;
use axion_ui::taffy::NodeId;
use axion_ui::{ActionId, Ui};
use glam::Vec2;

use crate::menu::{MAX_ITEMS, Menu, MenuAction, MenuClick, MenuItem, Placement};

/// The context menus' rows (one block of `MAX_ITEMS` per plane).
pub const CONTEXT_MENU_BASE: u32 = 17_000;

/// What a tab's context menu does with its tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCommand {
    NewTabToTheRight,
    Reload,
    Duplicate,
    Pin,
    Unpin,
    Mute,
    Unmute,
    Close,
    CloseOthers,
    ReopenClosed,
    BookmarkAll,
    /// Bookmarks this one tab.
    Bookmark,
    MoveToPrivate,
    MoveToNormal,
}

/// What a bookmark's context menu (or the bookmarks bar's overflow menu) does; indices are
/// into the bookmarks bar's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkCommand {
    Open(usize),
    OpenInNewTab(usize),
    Delete(usize),
}

/// What decides which rows a tab's menu has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabMenuInfo {
    pub pinned: bool,
    pub muted: bool,
    pub private: bool,
    /// Other unpinned tabs in its group (for "Close Other Tabs").
    pub others: bool,
    /// Unpinned tabs to its right.
    pub right: bool,
    /// Its group remembers closed tabs.
    pub can_reopen: bool,
    /// Bookmarks can be saved (the profile's database is open).
    pub can_bookmark: bool,
}

/// The rows of a tab's context menu: short and compact, only what concerns that one tab.
/// The group-wide commands live in the menu of the group's header ([`group_items`]).
pub fn tab_items(info: TabMenuInfo, keymap: &Keymap) -> Vec<MenuItem> {
    let shortcut = |action: Action| keymap.chords_for(action).first().map(ToString::to_string);
    let item = |icon: Icon, label: &str, command: TabCommand, action: Option<Action>| {
        let item = MenuItem::new(icon, label, MenuAction::Tab(command));
        match action.and_then(shortcut) {
            Some(keys) => item.with_detail(keys),
            None => item,
        }
    };
    let mut items = vec![
        item(Icon::Reload, tr("Reload"), TabCommand::Reload, Some(Action::Reload)),
        item(Icon::Copy, tr("Duplicate"), TabCommand::Duplicate, None),
        if info.muted {
            item(Icon::Volume, tr("Unmute Tab"), TabCommand::Unmute, None)
        } else {
            item(Icon::Muted, tr("Mute Tab"), TabCommand::Mute, None)
        },
    ];
    if info.can_bookmark {
        items.push(item(Icon::Bookmark, tr("Bookmark"), TabCommand::Bookmark, None));
    }
    items.push(item(Icon::Close, tr("Close Tab"), TabCommand::Close, Some(Action::CloseTab)).separated());
    items
}

/// The rows of a tab group header's menu ("N Tabs" or "Private"); `info` describes the group's
/// active tab, which the per-tab rows (pin, move) act on.
pub fn group_items(info: TabMenuInfo, keymap: &Keymap) -> Vec<MenuItem> {
    let shortcut = |action: Action| keymap.chords_for(action).first().map(ToString::to_string);
    let item = |icon: Icon, label: &str, command: TabCommand, action: Option<Action>| {
        let item = MenuItem::new(icon, label, MenuAction::Tab(command));
        match action.and_then(shortcut) {
            Some(keys) => item.with_detail(keys),
            None => item,
        }
    };
    let mut items = vec![item(Icon::Plus, tr("New Tab"), TabCommand::NewTabToTheRight, Some(Action::NewTab))];
    if info.can_reopen {
        items.push(item(
            Icon::History,
            tr("Reopen Closed Tab"),
            TabCommand::ReopenClosed,
            Some(Action::ReopenClosedTab),
        ));
    }
    if !info.private {
        items.push(if info.pinned {
            item(Icon::Pin, tr("Unpin Current Tab"), TabCommand::Unpin, Some(Action::PinTab)).separated()
        } else {
            item(Icon::Pin, tr("Pin Current Tab"), TabCommand::Pin, Some(Action::PinTab)).separated()
        });
    }
    items.push(if info.private {
        item(Icon::TabGroup, tr("Move Current to Normal"), TabCommand::MoveToNormal, None).separated()
    } else {
        item(Icon::Hand, tr("Move Current to Private"), TabCommand::MoveToPrivate, None)
    });
    if info.can_bookmark {
        items.push(item(Icon::Bookmark, tr("Bookmark All Tabs"), TabCommand::BookmarkAll, None));
    }
    if info.others {
        items.push(item(Icon::Close, tr("Close Other Tabs"), TabCommand::CloseOthers, None).separated());
    }
    items
}

/// The rows of a bookmark's context menu.
pub fn bookmark_items(index: usize) -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            Icon::Plus,
            tr("Open in New Tab"),
            MenuAction::Bookmark(BookmarkCommand::OpenInNewTab(index)),
        ),
        MenuItem::new(Icon::Trash, tr("Delete"), MenuAction::Bookmark(BookmarkCommand::Delete(index)))
            .separated(),
    ]
}

/// How far into the tab (or bookmark) it flows out of a context menu's bridge reaches, as a
/// share of its height: tabs are thin and an inactive one has hardly any glass of its own, so
/// a deeper bridge would cut across its label.
const BRIDGE_INTO_JOIN: f32 = 0.3;

/// One glass menu per plane things can be right-clicked in.
pub struct ContextMenus {
    menus: Vec<(NodeId, Menu)>,
    /// The page's own menu (the last of `menus`).
    page: usize,
    /// The row the open menu flows out of, whose content is faded out (see [`Self::cover_row`]).
    covered: Option<NodeId>,
}

impl ContextMenus {
    /// A menu in each of `planes` (the toolbar, the tab strip, the sidebar, the bookmarks bar),
    /// and the page's menu in `page_plane` (drawn above the page).
    pub fn build(ui: &mut Ui, planes: &[NodeId], page_plane: NodeId) -> Self {
        let menus: Vec<_> = planes
            .iter()
            .chain([&page_plane])
            .enumerate()
            .map(|(k, &plane)| {
                let base = CONTEXT_MENU_BASE + (k * MAX_ITEMS) as u32;
                let mut menu = Menu::build(ui, plane, base, Placement::BelowStart);
                menu.set_bridge_into_join(BRIDGE_INTO_JOIN);
                (plane, menu)
            })
            .collect();
        Self { page: menus.len() - 1, menus, covered: None }
    }

    /// Opens the page's menu, dripping out of `anchor` (a point in the page).
    pub fn open_page(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window: Vec2,
        width: f32,
        items: Vec<MenuItem>,
    ) {
        for (k, (_, menu)) in self.menus.iter_mut().enumerate() {
            if k != self.page {
                menu.close(ui, config);
            }
        }
        self.covered = None;
        let menu = &mut self.menus[self.page].1;
        menu.set_placement(Placement::Over);
        menu.set_min_width(width);
        menu.set_covered_nodes(ui, Vec::new());
        menu.open(ui, config, anchor, window, items);
    }

    /// Whether the page's menu is the one open.
    pub fn page_open(&self) -> bool {
        self.menus[self.page].1.is_open()
    }

    /// Opens the menu of `plane` next to `anchor` (logical px), closing any other.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        plane: NodeId,
        anchor: Frame,
        window: Vec2,
        placement: Placement,
        items: Vec<MenuItem>,
        hide: Option<NodeId>,
    ) {
        // The first menu of the plane (the page's menu shares the toolbar's plane).
        let target = self.menus.iter().position(|(p, _)| *p == plane);
        for (k, (_, menu)) in self.menus.iter_mut().enumerate() {
            if Some(k) != target {
                menu.close(ui, config);
            }
        }
        if let Some((_, menu)) = target.and_then(|k| self.menus.get_mut(k)) {
            menu.set_placement(placement);
            // The row it flows out of keeps its glass, but its content gives way to the menu.
            menu.set_covered_nodes(ui, hide.map(|row| ui.children(row)).unwrap_or_default());
            self.covered = hide;
            menu.open(ui, config, anchor, window, items);
        } else {
            tracing::warn!("no context menu for this plane");
        }
    }

    /// Fades out the content of `row`, the row the open menu flows out of. The chrome rebuilds
    /// its tab rows whenever a title or icon changes, so the browser calls this every frame
    /// with the row's current node; it does nothing unless that changed.
    pub fn cover_row(&mut self, ui: &mut Ui, row: Option<NodeId>) {
        if self.covered == row {
            return;
        }
        self.covered = row;
        let nodes = row.map(|row| ui.children(row)).unwrap_or_default();
        if let Some((_, menu)) = self.menus.iter_mut().find(|(_, m)| m.is_open()) {
            menu.set_covered_nodes(ui, nodes);
        }
    }

    pub fn is_open(&self) -> bool {
        self.menus.iter().any(|(_, m)| m.is_open())
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        for (_, menu) in &mut self.menus {
            if menu.is_open() {
                menu.close(ui, config);
            }
        }
    }

    pub fn owns(&self, id: ActionId) -> bool {
        self.menus.iter().any(|(_, m)| m.owns(id))
    }

    /// A click on one of the open menu's rows (see [`Menu::click`]).
    pub fn click(&mut self, ui: &mut Ui, config: &Config, id: ActionId) -> Option<MenuClick> {
        let (_, menu) = self.menus.iter_mut().find(|(_, m)| m.owns(id))?;
        menu.click(ui, config, id)
    }

    /// Whether `p` (logical px) is inside an open menu.
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.menus.iter().any(|(_, m)| m.is_open() && m.contains(ui, p))
    }

    /// Advances the drips and submenus; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32) {
        for (_, menu) in &mut self.menus {
            menu.track_hover(ui, config);
            menu.animate(ui, dt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(info: TabMenuInfo) -> Vec<String> {
        tab_items(info, &Keymap::default()).into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn tab_menu_is_short() {
        let all = TabMenuInfo { can_bookmark: true, can_reopen: true, others: true, ..Default::default() };
        assert_eq!(labels(all), ["Reload", "Duplicate", "Mute Tab", "Bookmark", "Close Tab"]);
        let muted = TabMenuInfo { muted: true, ..all };
        assert!(labels(muted).contains(&"Unmute Tab".to_owned()));
        // No database: nothing to bookmark into.
        assert_eq!(labels(TabMenuInfo::default()), ["Reload", "Duplicate", "Mute Tab", "Close Tab"]);
    }

    fn group_labels(info: TabMenuInfo) -> Vec<String> {
        group_items(info, &Keymap::default()).into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn group_menu_rows() {
        let normal = group_labels(TabMenuInfo::default());
        assert_eq!(normal, ["New Tab", "Pin Current Tab", "Move Current to Private"]);
        let full = group_labels(TabMenuInfo {
            pinned: true,
            can_reopen: true,
            can_bookmark: true,
            others: true,
            ..Default::default()
        });
        for label in ["Reopen Closed Tab", "Unpin Current Tab", "Bookmark All Tabs", "Close Other Tabs"] {
            assert!(full.iter().any(|l| l == label), "{label} missing in {full:?}");
        }
        let private = group_labels(TabMenuInfo { private: true, ..Default::default() });
        assert_eq!(private, ["New Tab", "Move Current to Normal"]);
    }

    #[test]
    fn shortcuts_come_from_the_keymap() {
        let items = tab_items(TabMenuInfo::default(), &Keymap::default());
        let close = items.iter().find(|i| i.label == "Close Tab").and_then(|i| i.detail.clone());
        assert_eq!(close.as_deref(), Some("Alt+Q"));
        assert!(items.iter().find(|i| i.label == "Duplicate").is_some_and(|i| i.detail.is_none()));
    }
}
