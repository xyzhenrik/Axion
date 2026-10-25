//! The window's tabs. When background tabs are frozen or unloaded is decided by `axion-tabs`.

use std::collections::HashSet;
use std::time::Instant;

use axion_engine::ViewId;
use axion_tabs::Lifecycle;

/// Each zoom step in Chromium's zoom-level units (1 level = ×1.2).
pub const ZOOM_STEP: f64 = 0.5;
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
    /// The page's media, as the bridge last reported it.
    pub media: Option<axion_media::PageMedia>,
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
        }
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

    /// Inserts after the active tab (like Safari) and optionally activates it.
    pub fn insert(&mut self, tab: Tab, activate: bool) -> usize {
        let index = if self.tabs.is_empty() { 0 } else { self.active + 1 };
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
    fn relative_selection_wraps() {
        let mut t = tabs(3);
        t.select(2);
        t.select_relative(1);
        assert_eq!(t.active_index(), 0);
        t.select_relative(-1);
        assert_eq!(t.active_index(), 2);
    }
}
