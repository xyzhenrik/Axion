//! The window's tabs. When background tabs are frozen or unloaded is decided by `axion-tabs`.

use std::collections::HashSet;
use std::time::Instant;

use axion_engine::ViewId;
use axion_tabs::Lifecycle;

/// Each zoom step in Chromium's zoom-level units (1 level = ×1.2).
pub const ZOOM_STEP: f64 = 0.5;
/// Remembered closed tabs for "reopen closed tab" unless the setting says otherwise.
const CLOSED_HISTORY: usize = 25;

/// The colour of a tab group (Safari's palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupColor {
    Blue,
    Red,
    Orange,
    Yellow,
    Green,
    Purple,
    Pink,
    Grey,
}

impl GroupColor {
    pub const ALL: [GroupColor; 8] = [
        GroupColor::Blue,
        GroupColor::Red,
        GroupColor::Orange,
        GroupColor::Yellow,
        GroupColor::Green,
        GroupColor::Purple,
        GroupColor::Pink,
        GroupColor::Grey,
    ];

    /// The name stored in the session.
    pub fn key(self) -> &'static str {
        match self {
            GroupColor::Blue => "blue",
            GroupColor::Red => "red",
            GroupColor::Orange => "orange",
            GroupColor::Yellow => "yellow",
            GroupColor::Green => "green",
            GroupColor::Purple => "purple",
            GroupColor::Pink => "pink",
            GroupColor::Grey => "grey",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }

    /// The English name (translate with `tr`).
    pub fn label(self) -> &'static str {
        match self {
            GroupColor::Blue => axion_i18n::tr_mark("Blue"),
            GroupColor::Red => axion_i18n::tr_mark("Red"),
            GroupColor::Orange => axion_i18n::tr_mark("Orange"),
            GroupColor::Yellow => axion_i18n::tr_mark("Yellow"),
            GroupColor::Green => axion_i18n::tr_mark("Green"),
            GroupColor::Purple => axion_i18n::tr_mark("Purple"),
            GroupColor::Pink => axion_i18n::tr_mark("Pink"),
            GroupColor::Grey => axion_i18n::tr_mark("Grey"),
        }
    }

    /// sRGB, the system accent colours.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            GroupColor::Blue => [0x0A, 0x84, 0xFF],
            GroupColor::Red => [0xFF, 0x45, 0x3A],
            GroupColor::Orange => [0xFF, 0x9F, 0x0A],
            GroupColor::Yellow => [0xFF, 0xD6, 0x0A],
            GroupColor::Green => [0x30, 0xD1, 0x58],
            GroupColor::Purple => [0xBF, 0x5A, 0xF2],
            GroupColor::Pink => [0xFF, 0x37, 0x5F],
            GroupColor::Grey => [0x8E, 0x8E, 0x93],
        }
    }

    pub fn color(self) -> axion_config::Color {
        let [r, g, b] = self.rgb();
        axion_config::Color::rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
    }

    /// The colour after this one in the palette (new groups cycle through it).
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&c| c == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// A named, coloured run of neighbouring tabs. The tabs of a group are always next to each
/// other (after the pinned tabs); the group has no place of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabGroup {
    pub id: u32,
    pub name: String,
    pub color: GroupColor,
    /// Its tabs are folded away in the lists (the active one stays shown).
    pub collapsed: bool,
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub view: ViewId,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub progress: f64,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub zoom: f64,
    /// A background tab keeps rendering until it has a thumbnail, then gets hidden.
    pub needs_snapshot: bool,
    /// Private tabs keep no history.
    pub private: bool,
    /// Requests, popups and redirects Shields blocked on the current page.
    pub blocked: u32,
    pub blocked_urls: HashSet<String>,
    pub lifecycle: Lifecycle,
    /// When the tab was last hidden (`None` while shown).
    pub hidden_since: Option<Instant>,
    /// Plays audible media.
    pub playing_media: bool,
    /// Muted from the address bar's speaker.
    pub muted: bool,
    /// Muted because its site is (Mute Site): let go again when the tab leaves the site.
    pub site_muted: bool,
    /// When the tab was last shown (unix seconds), for closing unused tabs.
    pub last_viewed: i64,
    /// The page's media, as the bridge last reported it.
    pub media: Option<axion_media::PageMedia>,
    /// Pinned tabs sit first, shown as just their favicon in the tab bar.
    pub pinned: bool,
    /// The tab group it belongs to (`TabGroup::id`).
    pub group: Option<u32>,
    /// What it records now (microphone, camera, screen).
    pub capture: crate::permissions::Capture,
}

impl Tab {
    pub fn new(view: ViewId, url: &str) -> Self {
        Self {
            view,
            url: url.to_owned(),
            title: String::new(),
            loading: true,
            progress: 0.0,
            can_go_back: false,
            can_go_forward: false,
            zoom: 0.0,
            needs_snapshot: false,
            private: false,
            blocked: 0,
            blocked_urls: HashSet::new(),
            lifecycle: Lifecycle::Background,
            hidden_since: Some(Instant::now()),
            playing_media: false,
            media: None,
            muted: false,
            site_muted: false,
            pinned: false,
            group: None,
            capture: Default::default(),
            last_viewed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
        }
    }

    /// Whether visits to this tab's pages go into the history (and the top sites): not for
    /// private tabs or the page source.
    pub fn records_history(&self) -> bool {
        !self.private && !self.url.starts_with("view-source:")
    }

    /// Title, or the host while the page has none.
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() { crate::url::display_host(&self.url) } else { self.title.clone() }
    }
}

pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    /// Closed tabs (url, title), oldest first.
    closed: Vec<(String, String)>,
    closed_limit: usize,
    groups: Vec<TabGroup>,
    next_group: u32,
    /// New tabs inserted next to a grouped tab join its group.
    pub inherit_group: bool,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            closed: Vec::new(),
            closed_limit: CLOSED_HISTORY,
            groups: Vec::new(),
            next_group: 1,
            inherit_group: true,
        }
    }
}

impl Tabs {
    pub fn all(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn all_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn by_view_mut(&mut self, view: ViewId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.view == view)
    }

    /// How many tabs are pinned (they are the first ones).
    pub fn pinned_count(&self) -> usize {
        self.tabs.iter().take_while(|t| t.pinned).count()
    }

    /// The indices a tab may take: pinned tabs stay among the pinned ones, the others after them.
    fn allowed_range(&self, pinned: bool, len: usize) -> std::ops::RangeInclusive<usize> {
        let pinned_count = self.pinned_count();
        if pinned { 0..=pinned_count.min(len) } else { pinned_count.min(len)..=len }
    }

    /// Inserts after the active tab (like Safari), but never among the pinned tabs unless it is
    /// pinned itself, and optionally activates it.
    pub fn insert(&mut self, tab: Tab, activate: bool) -> usize {
        let after = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        let range = self.allowed_range(tab.pinned, self.tabs.len());
        let index = after.clamp(*range.start(), *range.end());
        if !self.tabs.is_empty() && index <= self.active && !activate {
            self.active += 1;
        }
        self.tabs.insert(index, tab);
        if activate || self.tabs.len() == 1 {
            self.active = index;
        }
        self.inherit_neighbour_group(index);
        index
    }

    /// A tab without a group inserted inside or at the end of a group's run joins it.
    fn inherit_neighbour_group(&mut self, index: usize) {
        if !self.inherit_group || self.tabs[index].group.is_some() || self.tabs[index].pinned || index == 0 {
            return;
        }
        let before = self.tabs[index - 1].group;
        if before.is_some() {
            self.tabs[index].group = before;
        }
    }

    /// Removes a tab, remembering its URL; returns it. The neighbour to the right becomes
    /// active, or the left one at the end.
    pub fn remove(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.closed.push((tab.url.clone(), tab.title.clone()));
        while self.closed.len() > self.closed_limit.max(1) {
            self.closed.remove(0);
        }
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.prune_groups();
        Some(tab)
    }

    /// Inserts at `index` (clamped to the end) and optionally activates it; the active tab
    /// stays the same tab otherwise.
    pub fn insert_at(&mut self, index: usize, tab: Tab, activate: bool) -> usize {
        let range = self.allowed_range(tab.pinned, self.tabs.len());
        let index = index.clamp(*range.start(), *range.end());
        let was_empty = self.tabs.is_empty();
        self.tabs.insert(index, tab);
        if activate || was_empty {
            self.active = index;
        } else if index <= self.active {
            self.active += 1;
        }
        self.tabs[index].group = None;
        self.inherit_neighbour_group(index);
        index
    }

    /// Takes a tab out without remembering it as closed (it moves to the other group).
    pub fn take(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let mut tab = self.tabs.remove(index);
        tab.group = None;
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.prune_groups();
        Some(tab)
    }

    /// Moves the tab at `from` to `to` (an index among the other tabs, clamped). The active
    /// tab stays active. Returns whether the order changed.
    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tabs.len() {
            return false;
        }
        // Pinned tabs stay first: a tab only moves within its own part.
        let pinned_count = self.pinned_count();
        let to = if self.tabs[from].pinned {
            to.min(pinned_count.saturating_sub(1))
        } else {
            to.clamp(pinned_count, self.tabs.len() - 1)
        };
        if from == to {
            return false;
        }
        let active = self.tabs.get(self.active).map(|t| t.view);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = active.and_then(|v| self.tabs.iter().position(|t| t.view == v)).unwrap_or(0);
        self.gather_groups();
        true
    }

    /// Pins or unpins the tab at `index`: it moves to the end of the pinned tabs, or to just
    /// after them. The active tab stays active. Returns its new index.
    pub fn set_pinned(&mut self, index: usize, pinned: bool) -> Option<usize> {
        let tab = self.tabs.get(index)?;
        if tab.pinned == pinned {
            return Some(index);
        }
        let active = self.tabs.get(self.active).map(|t| t.view);
        let mut tab = self.tabs.remove(index);
        tab.pinned = pinned;
        tab.group = None;
        let at = self.pinned_count();
        self.tabs.insert(at, tab);
        self.active = active.and_then(|v| self.tabs.iter().position(|t| t.view == v)).unwrap_or(0);
        self.prune_groups();
        Some(at)
    }

    /// The index of `view`'s tab.
    pub fn index_of(&self, view: ViewId) -> Option<usize> {
        self.tabs.iter().position(|t| t.view == view)
    }

    /// Whether closed tabs are remembered (for "reopen closed tab").
    pub fn has_closed(&self) -> bool {
        !self.closed.is_empty()
    }

    pub fn pop_closed(&mut self) -> Option<String> {
        self.closed.pop().map(|(url, _)| url)
    }

    /// Closed tabs as (url, title), the most recent first.
    pub fn recently_closed(&self) -> impl Iterator<Item = (&str, &str)> {
        self.closed.iter().rev().map(|(u, t)| (u.as_str(), t.as_str()))
    }

    /// Forgets the closed tab with this URL (it was reopened from the tab search).
    pub fn forget_closed(&mut self, url: &str) {
        if let Some(i) = self.closed.iter().rposition(|(u, _)| u == url) {
            self.closed.remove(i);
        }
    }

    /// How many closed tabs are remembered.
    pub fn set_closed_limit(&mut self, limit: usize) {
        self.closed_limit = limit.max(1);
        while self.closed.len() > self.closed_limit {
            self.closed.remove(0);
        }
    }

    // ---- tab groups ----

    pub fn groups(&self) -> &[TabGroup] {
        &self.groups
    }

    pub fn group(&self, id: u32) -> Option<&TabGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: u32) -> Option<&mut TabGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Adds a group (without tabs; give it some with [`Self::set_tab_group`]).
    pub fn create_group(&mut self, name: &str, color: GroupColor) -> u32 {
        let id = self.next_group;
        self.next_group += 1;
        self.groups.push(TabGroup { id, name: name.to_owned(), color, collapsed: false });
        id
    }

    /// Re-creates a restored group under its saved id.
    pub fn restore_group(&mut self, group: TabGroup) {
        self.next_group = self.next_group.max(group.id + 1);
        self.groups.retain(|g| g.id != group.id);
        self.groups.push(group);
    }

    /// The colour a new group takes: the one after the last created group's.
    pub fn next_group_color(&self) -> GroupColor {
        self.groups.last().map_or(GroupColor::Blue, |g| g.color.next())
    }

    /// The indices of a group's tabs.
    pub fn group_range(&self, id: u32) -> Option<std::ops::Range<usize>> {
        let start = self.tabs.iter().position(|t| t.group == Some(id))?;
        let len = self.tabs[start..].iter().take_while(|t| t.group == Some(id)).count();
        Some(start..start + len)
    }

    /// Whether the tab at `index` is shown in the lists: not folded away in a collapsed group
    /// (the active tab always is).
    pub fn is_shown(&self, index: usize) -> bool {
        let Some(tab) = self.tabs.get(index) else { return false };
        match tab.group.and_then(|id| self.group(id)) {
            Some(g) if g.collapsed => index == self.active,
            _ => true,
        }
    }

    /// Moves the tab at `index` into group `group` (at the end of its run) or out of its
    /// group (to right behind it). The active tab stays active. Returns the tab's new index.
    pub fn set_tab_group(&mut self, index: usize, group: Option<u32>) -> Option<usize> {
        let tab = self.tabs.get(index)?;
        if tab.pinned || tab.group == group {
            return Some(index);
        }
        let active = self.tabs.get(self.active).map(|t| t.view);
        let old = tab.group;
        let mut tab = self.tabs.remove(index);
        tab.group = group;
        let at = match group.or(old) {
            // Its group has other tabs: behind them; otherwise it stays where it is.
            Some(id) => self.group_range(id).map_or(index, |r| r.end),
            None => index,
        };
        let at = at.clamp(self.pinned_count(), self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = active.and_then(|v| self.tabs.iter().position(|t| t.view == v)).unwrap_or(0);
        self.prune_groups();
        Some(at)
    }

    /// A drop: puts the tab at `to` (an index among the other tabs, clamped) and into `group`.
    /// Returns the tab's new index.
    pub fn place(&mut self, from: usize, to: usize, group: Option<u32>) -> Option<usize> {
        if from >= self.tabs.len() || self.tabs[from].pinned {
            return None;
        }
        let active = self.tabs.get(self.active).map(|t| t.view);
        let mut tab = self.tabs.remove(from);
        tab.group = group;
        let at = to.clamp(self.pinned_count(), self.tabs.len());
        let view = tab.view;
        self.tabs.insert(at, tab);
        self.active = active.and_then(|v| self.tabs.iter().position(|t| t.view == v)).unwrap_or(0);
        self.gather_groups();
        self.prune_groups();
        self.index_of(view)
    }

    /// Dissolves a group: its tabs stay where they are.
    pub fn ungroup(&mut self, id: u32) {
        for tab in self.tabs.iter_mut().filter(|t| t.group == Some(id)) {
            tab.group = None;
        }
        self.groups.retain(|g| g.id != id);
    }

    /// The views of a group's tabs.
    pub fn group_views(&self, id: u32) -> Vec<ViewId> {
        self.tabs.iter().filter(|t| t.group == Some(id)).map(|t| t.view).collect()
    }

    /// Drops groups without tabs.
    fn prune_groups(&mut self) {
        let tabs = &self.tabs;
        self.groups.retain(|g| tabs.iter().any(|t| t.group == Some(g.id)));
    }

    /// Makes every group's tabs one run again (a tab moved into the middle of one): later
    /// pieces join the first.
    fn gather_groups(&mut self) {
        let active = self.tabs.get(self.active).map(|t| t.view);
        let mut rest = std::mem::take(&mut self.tabs);
        let mut out: Vec<Tab> = Vec::with_capacity(rest.len());
        let mut placed: Vec<u32> = Vec::new();
        let mut i = 0;
        while i < rest.len() {
            match rest[i].group {
                Some(g) if !placed.contains(&g) => {
                    placed.push(g);
                    let (mine, others): (Vec<Tab>, Vec<Tab>) =
                        rest.drain(i..).partition(|x| x.group == Some(g));
                    out.extend(mine);
                    rest.extend(others);
                }
                Some(_) => i += 1,
                None => {
                    out.push(rest.remove(i));
                }
            }
        }
        // Whatever is left belongs to groups placed before the walk reached it.
        let _ = i;
        self.tabs = out;
        self.active = active.and_then(|v| self.tabs.iter().position(|t| t.view == v)).unwrap_or(0);
    }

    pub fn select(&mut self, index: usize) -> bool {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            true
        } else {
            false
        }
    }

    pub fn select_relative(&mut self, delta: isize) -> bool {
        let n = self.tabs.len() as isize;
        if n < 2 {
            return false;
        }
        self.select((self.active as isize + delta).rem_euclid(n) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(n: u32) -> Tabs {
        let mut t = Tabs::default();
        for i in 0..n {
            t.insert(Tab::new(ViewId(i), &format!("https://t{i}.example")), true);
        }
        t
    }

    #[test]
    fn insert_after_active() {
        let mut t = tabs(3);
        t.select(0);
        let i = t.insert(Tab::new(ViewId(9), "https://new.example"), false);
        assert_eq!(i, 1);
        assert_eq!(t.active_index(), 0);
    }

    #[test]
    fn closing_moves_selection_sensibly() {
        let mut t = tabs(3);
        assert_eq!(t.active_index(), 2);
        t.remove(2);
        assert_eq!(t.active_index(), 1);
        t.select(0);
        t.remove(1);
        assert_eq!(t.active_index(), 0);
        t.remove(0);
        assert!(t.is_empty());
        assert_eq!(t.pop_closed().as_deref(), Some("https://t0.example"));
    }

    #[test]
    fn closing_left_of_active_keeps_it() {
        let mut t = tabs(3);
        t.select(2);
        t.remove(0);
        assert_eq!(t.active().map(|t| t.view), Some(ViewId(2)));
    }

    #[test]
    fn moving_keeps_the_active_tab() {
        let mut t = tabs(4);
        t.select(1);
        assert!(t.move_tab(1, 3));
        assert_eq!(t.all().iter().map(|t| t.view.0).collect::<Vec<_>>(), [0, 2, 3, 1]);
        assert_eq!(t.active().map(|t| t.view), Some(ViewId(1)));
        // Another tab moving past the active one.
        assert!(t.move_tab(0, usize::MAX));
        assert_eq!(t.all().iter().map(|t| t.view.0).collect::<Vec<_>>(), [2, 3, 1, 0]);
        assert_eq!(t.active().map(|t| t.view), Some(ViewId(1)));
        assert!(!t.move_tab(2, 2));
        assert!(!t.move_tab(9, 0));
    }

    #[test]
    fn taking_and_inserting_between_groups() {
        let mut normal = tabs(3);
        let mut private = Tabs::default();
        normal.select(2);
        let tab = normal.take(0).unwrap();
        // Not remembered as closed: it lives on in the other group.
        assert_eq!(normal.pop_closed(), None);
        assert_eq!(normal.active().map(|t| t.view), Some(ViewId(2)));
        assert_eq!(private.insert_at(usize::MAX, tab, false), 0);
        assert_eq!(private.active_index(), 0);
        let i = private.insert_at(0, Tab::new(ViewId(9), "https://x.example"), false);
        assert_eq!(i, 0);
        // The active tab stays the same tab.
        assert_eq!(private.active().map(|t| t.view), Some(ViewId(0)));
        assert_eq!(private.index_of(ViewId(9)), Some(0));
    }

    #[test]
    fn pinned_tabs_stay_first() {
        let mut t = tabs(4);
        t.select(2);
        assert_eq!(t.set_pinned(2, true), Some(0));
        assert_eq!(t.active().map(|t| t.view), Some(ViewId(2)));
        assert_eq!(t.pinned_count(), 1);
        // A new tab from the pinned one opens after the pinned tabs.
        let i = t.insert(Tab::new(ViewId(9), "https://new.example"), false);
        assert_eq!(i, 1);
        assert_eq!(t.active().map(|t| t.view), Some(ViewId(2)));
        // Unpinned tabs can't move in front of the pinned one, nor the pinned one behind them.
        assert!(t.move_tab(3, 0));
        assert_eq!(t.all()[0].view, ViewId(2));
        assert_eq!(t.all()[1].view, ViewId(1));
        assert!(!t.move_tab(0, 4));
        assert_eq!(t.set_pinned(3, true), Some(1));
        assert_eq!(t.pinned_count(), 2);
        assert!(t.move_tab(1, 0));
        // Unpinning puts it right after the pinned tabs.
        let view = t.all()[0].view;
        assert_eq!(t.set_pinned(0, false), Some(1));
        assert_eq!(t.all()[1].view, view);
        assert_eq!(t.pinned_count(), 1);
        // Moved in from the other group: never among the pinned tabs.
        assert_eq!(t.insert_at(0, Tab::new(ViewId(10), "https://x.example"), false), 1);
    }

    fn order(t: &Tabs) -> Vec<u32> {
        t.all().iter().map(|t| t.view.0).collect()
    }

    #[test]
    fn groups_keep_their_tabs_together() {
        let mut t = tabs(5);
        t.select(0);
        let g = t.create_group("Work", GroupColor::Green);
        assert_eq!(t.set_tab_group(3, Some(g)), Some(3));
        // Joining puts the tab behind the group's others.
        assert_eq!(t.set_tab_group(0, Some(g)), Some(3));
        assert_eq!(order(&t), [1, 2, 3, 0, 4]);
        assert_eq!(t.active().map(|t| t.view), Some(ViewId(0)));
        assert_eq!(t.group_range(g), Some(2..4));
        // Leaving from the middle puts the tab behind the group.
        assert_eq!(t.set_tab_group(2, None), Some(3));
        assert_eq!(order(&t), [1, 2, 0, 3, 4]);
        // The last tab leaving dissolves the group.
        assert_eq!(t.set_tab_group(2, None), Some(2));
        assert!(t.groups().is_empty());
        assert!(t.all().iter().all(|t| t.group.is_none()));
    }

    #[test]
    fn tabs_open_next_to_a_grouped_tab_join_it() {
        let mut t = tabs(3);
        let g = t.create_group("A", GroupColor::Red);
        t.set_tab_group(1, Some(g));
        t.select(1);
        t.insert(Tab::new(ViewId(9), "https://n.example"), false);
        assert_eq!(t.all()[2].group, Some(g));
        t.inherit_group = false;
        t.insert(Tab::new(ViewId(10), "https://m.example"), false);
        assert_eq!(t.all()[2].group, None);
        // Closing the last tab of a group removes the group.
        t.inherit_group = true;
        let views = t.group_views(g);
        for v in views {
            let i = t.index_of(v).unwrap();
            t.remove(i);
        }
        assert!(t.groups().is_empty());
    }

    #[test]
    fn dropping_gathers_split_groups() {
        let mut t = tabs(5);
        let g = t.create_group("A", GroupColor::Red);
        t.set_tab_group(1, Some(g));
        t.set_tab_group(2, Some(g));
        // An ungrouped tab dropped in the middle of the group ends up behind it.
        t.place(4, 2, None);
        assert_eq!(t.group_range(g).map(|r| r.len()), Some(2));
        // A drop with a group joins it.
        let i = t.place(0, 1, Some(g)).unwrap();
        assert_eq!(t.all()[i].group, Some(g));
        assert_eq!(t.group_range(g).map(|r| r.len()), Some(3));
    }

    #[test]
    fn collapsed_groups_show_only_the_active_tab() {
        let mut t = tabs(4);
        let g = t.create_group("A", GroupColor::Blue);
        t.set_tab_group(1, Some(g));
        t.set_tab_group(2, Some(g));
        t.group_mut(g).unwrap().collapsed = true;
        t.select(0);
        assert!(t.is_shown(0) && !t.is_shown(1) && !t.is_shown(2) && t.is_shown(3));
        t.select(2);
        assert!(!t.is_shown(1) && t.is_shown(2));
        t.ungroup(g);
        assert!(t.is_shown(1));
    }

    #[test]
    fn pinning_leaves_the_group() {
        let mut t = tabs(3);
        let g = t.create_group("A", GroupColor::Blue);
        t.set_tab_group(1, Some(g));
        t.set_pinned(1, true);
        assert!(t.groups().is_empty());
    }

    #[test]
    fn closed_tabs_keep_titles() {
        let mut t = tabs(2);
        t.all_mut()[1].title = "Two".into();
        t.remove(1);
        assert_eq!(t.recently_closed().next(), Some(("https://t1.example", "Two")));
        t.forget_closed("https://t1.example");
        assert!(!t.has_closed());
    }

    #[test]
    fn relative_selection_wraps() {
        let mut t = tabs(3);
        t.select(2);
        t.select_relative(1);
        assert_eq!(t.active_index(), 0);
        t.select_relative(-1);
        assert_eq!(t.active_index(), 2);
    }
}
