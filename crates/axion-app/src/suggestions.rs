//! Suggestions under the address field while typing: what the input itself means (a search or
//! an address), then matching bookmarks and history — as a glass card, navigable with ↑/↓.

use axion_compositor::{Icon, Rect as Frame};
use axion_store::Store;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, GlassLevel, Paint, TextStyle, Ui, Widget};

use crate::url::{is_address, resolve_input};

const ROW_BASE: u32 = 5000;
const MAX_HISTORY: usize = 6;
const MAX_BOOKMARKS: usize = 2;
const ROW_HEIGHT: f32 = 48.0;
const GAP_BELOW_BAR: f32 = 8.0;
const RADIUS: f32 = 20.0;
/// Highlight of the selected row (accent at this opacity).
const SELECTED_OPACITY: f32 = 0.16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Search,
    Address,
    Bookmark,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub kind: Kind,
    pub title: String,
    /// What opening it loads.
    pub url: String,
}

/// Suggestions for `input`, best first. The first one is always what the input means itself.
pub fn suggest(input: &str, store: Option<&Store>, search_url: &str, now: i64) -> Vec<Suggestion> {
    let query = input.trim();
    let Some(own) = resolve_input(query, search_url) else { return Vec::new() };
    let mut out = vec![if is_address(query) {
        Suggestion { kind: Kind::Address, title: query.to_owned(), url: own }
    } else {
        Suggestion { kind: Kind::Search, title: format!("Search for “{query}”"), url: own }
    }];
    let Some(store) = store else { return out };
    let push = |s: Suggestion, out: &mut Vec<Suggestion>| {
        if !out.iter().any(|o| o.url == s.url) {
            out.push(s);
        }
    };
    match store.search_bookmarks(query, MAX_BOOKMARKS) {
        Ok(found) => {
            for b in found {
                push(Suggestion { kind: Kind::Bookmark, title: b.title, url: b.url }, &mut out);
            }
        }
        Err(e) => tracing::warn!("bookmark search failed: {e}"),
    }
    match store.search_history(query, MAX_HISTORY, now) {
        Ok(found) => {
            for h in found {
                push(Suggestion { kind: Kind::History, title: h.title, url: h.url }, &mut out);
            }
        }
        Err(e) => tracing::warn!("history search failed: {e}"),
    }
    out
}

pub struct SuggestionList {
    root: NodeId,
    rows: Vec<NodeId>,
    items: Vec<Suggestion>,
    selected: Option<usize>,
    query: String,
}

impl SuggestionList {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(6.0),
                    right: length(6.0),
                    top: length(6.0),
                    bottom: length(6.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        Self { root, rows: Vec::new(), items: Vec::new(), selected: None, query: String::new() }
    }

    pub fn row_action(id: ActionId) -> Option<usize> {
        (ROW_BASE..ROW_BASE + 100).contains(&id.0).then(|| (id.0 - ROW_BASE) as usize)
    }

    pub fn is_visible(&self) -> bool {
        !self.items.is_empty()
    }

    /// Updates the list for `query`, placed under `anchor` (the address field, logical px).
    /// Returns without work when the query didn't change.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        query: &str,
        anchor: Frame,
        items: impl FnOnce() -> Vec<Suggestion>,
    ) {
        if query == self.query {
            return;
        }
        self.query = query.to_owned();
        self.items = if query.trim().is_empty() { Vec::new() } else { items() };
        self.selected = None;
        self.rebuild(ui, anchor);
    }

    pub fn hide(&mut self, ui: &mut Ui) {
        self.items.clear();
        self.query.clear();
        self.selected = None;
        ui.set_visible(self.root, false);
    }

    fn rebuild(&mut self, ui: &mut Ui, anchor: Frame) {
        ui.clear_children(self.root);
        self.rows.clear();
        ui.set_visible(self.root, !self.items.is_empty());
        if self.items.is_empty() {
            return;
        }
        let mut style = ui.style(self.root);
        style.inset = Rect {
            left: length(anchor.origin.x),
            right: auto(),
            top: length(anchor.origin.y + anchor.size.y + GAP_BELOW_BAR),
            bottom: auto(),
        };
        style.size = Size { width: length(anchor.size.x.max(320.0)), height: auto() };
        ui.set_style(self.root, style);

        for (i, item) in self.items.iter().enumerate() {
            let row = ui.add(
                self.root,
                Widget::container().with_action(ActionId(ROW_BASE + i as u32)).with_radius(14.0),
                Style {
                    size: Size { width: auto(), height: length(ROW_HEIGHT) },
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(12.0), height: length(0.0) },
                    padding: Rect {
                        left: length(12.0),
                        right: length(12.0),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            let icon = match item.kind {
                Kind::Search => Icon::Search,
                Kind::Address => Icon::Forward,
                Kind::Bookmark => Icon::Bookmark,
                Kind::History => Icon::History,
            };
            ui.add(
                row,
                Widget::icon(icon, 18.0).with_paint(Paint::Secondary),
                Style { flex_shrink: 0.0, ..Default::default() },
            );
            let text = ui.add(
                row,
                Widget::container(),
                Style {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
            let title = if item.title.trim().is_empty() { item.url.clone() } else { item.title.clone() };
            ui.add_label(
                text,
                &title,
                TextStyle::Body,
                Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
            );
            if matches!(item.kind, Kind::Bookmark | Kind::History) {
                let url = ui.add_label(
                    text,
                    &item.url,
                    TextStyle::Footnote,
                    Style { min_size: Size { width: length(0.0), height: auto() }, ..Default::default() },
                );
                if let Some(w) = ui.widget_mut(url) {
                    w.set_paint(Paint::Secondary);
                }
            }
            self.rows.push(row);
        }
        self.paint_selection(ui);
    }

    fn paint_selection(&self, ui: &mut Ui) {
        for (i, &row) in self.rows.iter().enumerate() {
            if let Some(w) = ui.widget_mut(row) {
                w.background = if Some(i) == self.selected {
                    Background::Fill(Paint::Accent, SELECTED_OPACITY)
                } else {
                    Background::None
                };
            }
        }
    }

    /// ↑/↓: moves the selection (`None` = the typed text). Returns the URL to show in the field.
    pub fn move_selection(&mut self, ui: &mut Ui, delta: isize) -> Option<&Suggestion> {
        if self.items.is_empty() {
            return None;
        }
        let n = self.items.len() as isize;
        let current = self.selected.map_or(-1, |i| i as isize);
        let next = (current + delta).clamp(-1, n - 1);
        self.selected = (next >= 0).then_some(next as usize);
        self.paint_selection(ui);
        self.selected.and_then(|i| self.items.get(i))
    }

    pub fn selected(&self) -> Option<&Suggestion> {
        self.selected.and_then(|i| self.items.get(i))
    }

    pub fn get(&self, index: usize) -> Option<&Suggestion> {
        self.items.get(index)
    }

    /// Keeps the list attached to the address field when the window or layout changes.
    pub fn reposition(&mut self, ui: &mut Ui, anchor: Frame) {
        if !self.items.is_empty() {
            let mut style = ui.style(self.root);
            let top = anchor.origin.y + anchor.size.y + GAP_BELOW_BAR;
            if style.inset.left != length(anchor.origin.x) || style.inset.top != length(top) {
                style.inset.left = length(anchor.origin.x);
                style.inset.top = length(top);
                style.size.width = length(anchor.size.x.max(320.0));
                ui.set_style(self.root, style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = "https://search.example/?q={}";

    #[test]
    fn first_row_is_the_input_itself() {
        let s = suggest("liquid glass", None, SEARCH, 0);
        assert_eq!(s[0].kind, Kind::Search);
        assert_eq!(s[0].url, "https://search.example/?q=liquid+glass");
        let s = suggest("example.com", None, SEARCH, 0);
        assert_eq!(s[0].kind, Kind::Address);
        assert_eq!(s[0].url, "https://example.com");
        assert!(suggest("  ", None, SEARCH, 0).is_empty());
    }

    #[test]
    fn adds_bookmarks_and_history_without_duplicates() {
        let store = Store::in_memory().unwrap();
        store.record_visit("https://www.rust-lang.org/", "Rust", 100).unwrap();
        store.record_visit("https://doc.rust-lang.org/", "Docs", 100).unwrap();
        store.add_bookmark("https://www.rust-lang.org/", "Rust", 100).unwrap();
        let s = suggest("rust", Some(&store), SEARCH, 200);
        let urls: Vec<&str> = s.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://search.example/?q=rust",
                "https://www.rust-lang.org/",
                "https://doc.rust-lang.org/"
            ]
        );
        assert_eq!(s[1].kind, Kind::Bookmark);
    }
}
