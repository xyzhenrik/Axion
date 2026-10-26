//! Context menus: right-clicking a tab (separate tab bar, compact capsule, sidebar row) or a
//! bookmark of the bookmarks bar opens a glass [`Menu`] that flows out of it. Each menu lives
//! in the plane of what was clicked, so its glass merges with the glass there (see
//! [`crate::popover`]); only one is open at a time. Rows that don't apply are left out, and
//! shortcuts are shown from the keymap.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Action, Config, Keymap};
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
    CloseToTheRight,
    ReopenClosed,
    BookmarkAll,
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

/// The rows of a tab's context menu, with the keymap's shortcuts where one applies.
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
        item(Icon::Plus, "New Tab to the Right", TabCommand::NewTabToTheRight, Some(Action::NewTab)),
        item(Icon::Reload, "Reload", TabCommand::Reload, Some(Action::Reload)).separated(),
        item(Icon::Copy, "Duplicate", TabCommand::Duplicate, None),
        if info.pinned {
            item(Icon::Pin, "Unpin", TabCommand::Unpin, Some(Action::PinTab))
        } else {
            item(Icon::Pin, "Pin", TabCommand::Pin, Some(Action::PinTab))
        },
        if info.muted {
            item(Icon::Volume, "Unmute Tab", TabCommand::Unmute, None)
        } else {
            item(Icon::Muted, "Mute Tab", TabCommand::Mute, None)
        },
    ];
    if info.can_bookmark {
        items.push(item(Icon::Bookmark, "Bookmark All Tabs", TabCommand::BookmarkAll, None).separated());
    }
    let move_item = if info.private {
        item(Icon::TabGroup, "Move to Normal Tabs", TabCommand::MoveToNormal, None)
    } else {
        item(Icon::Hand, "Move to Private Tabs", TabCommand::MoveToPrivate, None)
    };
    // It starts the group when "Bookmark All Tabs" isn't there.
    items.push(if info.can_bookmark { move_item } else { move_item.separated() });
    items.push(item(Icon::Close, "Close Tab", TabCommand::Close, Some(Action::CloseTab)).separated());
    if info.others {
        items.push(item(Icon::Close, "Close Other Tabs", TabCommand::CloseOthers, None));
    }
    if info.right {
        items.push(item(Icon::Forward, "Close Tabs to the Right", TabCommand::CloseToTheRight, None));
    }
    if info.can_reopen {
        items.push(item(
            Icon::History,
            "Reopen Closed Tab",
            TabCommand::ReopenClosed,
            Some(Action::ReopenClosedTab),
        ));
    }
    items
}

/// The rows of a bookmark's context menu.
pub fn bookmark_items(index: usize) -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            Icon::Plus,
            "Open in New Tab",
            MenuAction::Bookmark(BookmarkCommand::OpenInNewTab(index)),
        ),
        MenuItem::new(Icon::Trash, "Delete", MenuAction::Bookmark(BookmarkCommand::Delete(index)))
            .separated(),
    ]
}

/// One glass menu per plane things can be right-clicked in.
pub struct ContextMenus {
    menus: Vec<(NodeId, Menu)>,
}

impl ContextMenus {
    /// A menu in each of `planes` (the toolbar, the tab strip, the sidebar, the bookmarks bar).
    pub fn build(ui: &mut Ui, planes: &[NodeId]) -> Self {
        let menus = planes
            .iter()
            .enumerate()
            .map(|(k, &plane)| {
                let base = CONTEXT_MENU_BASE + (k * MAX_ITEMS) as u32;
                (plane, Menu::build(ui, plane, base, Placement::BelowStart))
            })
            .collect();
        Self { menus }
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
    ) {
        for (p, menu) in &mut self.menus {
            if *p != plane {
                menu.close(ui, config);
            }
        }
        if let Some((_, menu)) = self.menus.iter_mut().find(|(p, _)| *p == plane) {
            menu.set_placement(placement);
            menu.open(ui, config, anchor, window, items);
        } else {
            tracing::warn!("no context menu for this plane");
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
    fn only_rows_that_apply() {
        let alone = labels(TabMenuInfo::default());
        assert!(!alone.iter().any(|l| l == "Close Other Tabs" || l == "Close Tabs to the Right"));
        assert!(!alone.iter().any(|l| l == "Reopen Closed Tab" || l == "Bookmark All Tabs"));
        assert!(alone.iter().any(|l| l == "Pin") && alone.iter().any(|l| l == "Mute Tab"));
        assert!(alone.iter().any(|l| l == "Move to Private Tabs"));

        let full = labels(TabMenuInfo {
            pinned: true,
            muted: true,
            private: true,
            others: true,
            right: true,
            can_reopen: true,
            can_bookmark: true,
        });
        for label in
            ["Unpin", "Unmute Tab", "Move to Normal Tabs", "Close Other Tabs", "Close Tabs to the Right"]
        {
            assert!(full.iter().any(|l| l == label), "{label} missing in {full:?}");
        }
        assert!(full.iter().all(|l| !l.is_empty()));
    }

    #[test]
    fn shortcuts_come_from_the_keymap() {
        let items = tab_items(TabMenuInfo::default(), &Keymap::default());
        let close = items.iter().find(|i| i.label == "Close Tab").and_then(|i| i.detail.clone());
        assert_eq!(close.as_deref(), Some("Alt+Q"));
        assert!(items.iter().find(|i| i.label == "Duplicate").is_some_and(|i| i.detail.is_none()));
    }
}
