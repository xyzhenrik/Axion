//! The window's tabs. When background tabs are frozen or unloaded is decided by `axion-tabs`.

use std::collections::HashSet;
use std::time::Instant;

use axion_engine::ViewId;
use axion_tabs::Lifecycle;

/// Remembered closed tabs for "reopen closed tab".
const CLOSED_HISTORY: usize = 25;

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
    /// When the tab was last shown (unix seconds), for closing unused tabs.
    pub last_viewed: i64,
    /// The page's media, as the bridge last reported it.
    pub media: Option<axion_media::PageMedia>,
    /// Pinned tabs sit first, shown as just their favicon in the tab bar.
    pub pinned: bool,
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
            pinned: false,
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

#[derive(Default)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    closed: Vec<String>,
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
        index
    }

    /// Removes a tab, remembering its URL; returns it. The neighbour to the right becomes
    /// active, or the left one at the end.
    pub fn remove(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.closed.push(tab.url.clone());
        if self.closed.len() > CLOSED_HISTORY {
            self.closed.remove(0);
        }
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
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
        index
    }

    /// Takes a tab out without remembering it as closed (it moves to the other group).
    pub fn take(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
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
        let at = self.pinned_count();
        self.tabs.insert(at, tab);
        self.active = active.and_then(|v| self.tabs.iter().position(|t| t.view == v)).unwrap_or(0);
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
        self.closed.pop()
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
