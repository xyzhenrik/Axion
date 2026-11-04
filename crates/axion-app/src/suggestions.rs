//! Suggestions under the address field while typing: what the input itself means (a search or
//! an address), then matching bookmarks and history — as a glass card, navigable with ↑/↓.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_store::Store;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};

use crate::favicons::{Favicons, favicon_or};
use crate::popover::Popover;

use crate::url::{is_address, resolve_input};

const ROW_BASE: u32 = 5000;
const MAX_HISTORY: usize = 6;
const MAX_BOOKMARKS: usize = 2;
const ROW_HEIGHT: f32 = 48.0;
/// Side of a row's icon (site icon or symbol).
const ICON_SIZE: f32 = 18.0;
const GAP_BELOW_BAR: f32 = 14.0;
const RADIUS: f32 = 22.0;
const PADDING: f32 = 6.0;
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

/// Which page's icon the address field shows while the user edits the address: the selected
/// suggestion's, else the top one's (a search has no site: globe). `None` = globe. Cache lookups
/// only (`favicon_node` falls back to the globe for unknown hosts).
pub fn typing_icon_url<'a>(selected: Option<&'a Suggestion>, items: &'a [Suggestion]) -> Option<&'a str> {
    let pick = selected.or_else(|| items.first())?;
    (pick.kind != Kind::Search).then_some(pick.url.as_str())
}

/// Suggestions for `input`, best first. The first one is always what the input means itself.
pub fn suggest(input: &str, store: Option<&Store>, search_url: &str, now: i64) -> Vec<Suggestion> {
    let query = input.trim();
    let Some(own) = resolve_input(query, search_url) else { return Vec::new() };
    let mut out = vec![if is_address(query) {
        Suggestion { kind: Kind::Address, title: query.to_owned(), url: own }
    } else {
        Suggestion {
            kind: Kind::Search,
            title: axion_i18n::tr_fmt!("Search for “{query}”", query = query),
            url: own,
        }
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

/// What can be shown for `input` at once, before the stored matches arrive (see
/// `suggest_worker`): what the input itself means, then those of `previous` (the list shown for
/// what was typed before) that still match every word of it. Keeps the card steady while typing
/// instead of shrinking to one row and growing again at each key.
pub fn provisional(input: &str, search_url: &str, previous: &[Suggestion]) -> Vec<Suggestion> {
    let mut out = suggest(input, None, search_url, 0);
    let words: Vec<String> = input.split_whitespace().map(str::to_lowercase).collect();
    for old in previous.iter().filter(|s| matches!(s.kind, Kind::Bookmark | Kind::History)) {
        let (url, title) = (old.url.to_lowercase(), old.title.to_lowercase());
        let matches = words.iter().all(|w| url.contains(w.as_str()) || title.contains(w.as_str()));
        if matches && !out.iter().any(|o| o.url == old.url) {
            out.push(old.clone());
        }
    }
    out
}

pub struct SuggestionList {
    popover: Popover,
    /// The glass panel the rows sit on.
    plate: NodeId,
    rows: Vec<NodeId>,
    items: Vec<Suggestion>,
    selected: Option<usize>,
    query: String,
    /// An answer that arrived while a row was held: the rows are replaced once it is let go
    /// (rebuilding them under the pointer would cancel the click, and the row's meaning).
    pending: Option<(String, Vec<Suggestion>)>,
}

impl SuggestionList {
    /// `parent` is the toolbar, so the card's glass flows out of the bar.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            RADIUS,
            Style {
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(PADDING),
                    bottom: length(PADDING),
                },
                flex_direction: FlexDirection::Column,
                ..Default::default()
            },
        );
        let plate = popover.root();
        Self {
            popover,
            plate,
            rows: Vec::new(),
            items: Vec::new(),
            selected: None,
            query: String::new(),
            pending: None,
        }
    }

    pub fn row_action(id: ActionId) -> Option<usize> {
        (ROW_BASE..ROW_BASE + 100).contains(&id.0).then(|| (id.0 - ROW_BASE) as usize)
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_open()
    }

    /// Where row `i` is drawn (test aid for pointer tests).
    #[cfg(test)]
    pub(crate) fn row_center(&self, ui: &Ui, i: usize) -> Option<glam::Vec2> {
        self.rows.get(i).map(|&row| ui.frame(row).center())
    }

    /// The card's frame (test aid).
    #[cfg(test)]
    pub(crate) fn card(&self, ui: &Ui) -> Frame {
        ui.frame(self.plate)
    }

    /// Updates the list for `query`, placed under `anchor` (the address field, logical px).
    /// Returns without work when the query didn't change.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        query: &str,
        anchor: Frame,
        favicons: &Favicons,
        items: impl FnOnce() -> Vec<Suggestion>,
    ) {
        if query == self.query {
            return;
        }
        self.query = query.to_owned();
        self.items = if query.trim().is_empty() { Vec::new() } else { items() };
        self.selected = None;
        self.rebuild(ui, config, anchor, favicons);
    }

    /// The query the list was last built for.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The rows shown now.
    pub fn items(&self) -> &[Suggestion] {
        &self.items
    }

    /// The full list for `query` arrived (from the worker): replaces the rows when the list is
    /// still for that query. A selected row stays selected while it is still there.
    pub fn set_items(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        query: &str,
        items: Vec<Suggestion>,
        anchor: Frame,
        favicons: &Favicons,
    ) {
        if query != self.query || items == self.items {
            return;
        }
        if self.row_held(ui) {
            self.pending = Some((query.to_owned(), items));
            return;
        }
        let selected = self.selected.and_then(|i| self.items.get(i)).map(|s| s.url.clone());
        self.items = if query.trim().is_empty() { Vec::new() } else { items };
        self.selected = selected.and_then(|url| self.items.iter().position(|s| s.url == url));
        self.rebuild(ui, config, anchor, favicons);
    }

    /// Whether the pointer holds one of the rows (pressed, not yet let go).
    fn row_held(&self, ui: &Ui) -> bool {
        ui.pressed_action()
            .and_then(|n| ui.widget(n))
            .and_then(|w| w.action)
            .is_some_and(|a| Self::row_action(a).is_some())
    }

    /// Applies the answer that waited for a held row to be let go. Call every frame.
    pub fn flush_pending(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, favicons: &Favicons) {
        if self.pending.is_some()
            && !self.row_held(ui)
            && let Some((query, items)) = self.pending.take()
        {
            self.set_items(ui, config, &query, items, anchor, favicons);
        }
    }

    pub fn hide(&mut self, ui: &mut Ui, config: &Config) {
        self.pending = None;
        self.items.clear();
        self.query.clear();
        self.selected = None;
        self.popover.close(ui, config);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    fn target(&self, anchor: Frame) -> Frame {
        let height = self.items.len() as f32 * ROW_HEIGHT + 2.0 * PADDING;
        Frame::new(
            anchor.origin.x,
            anchor.origin.y + anchor.size.y + GAP_BELOW_BAR,
            anchor.size.x.max(320.0),
            height,
        )
    }

    fn rebuild(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, favicons: &Favicons) {
        ui.clear_children(self.plate);
        self.rows.clear();
        if self.items.is_empty() {
            self.popover.close(ui, config);
            return;
        }
        let target = self.target(anchor);
        self.popover.open(ui, config, anchor, target);

        for (i, item) in self.items.iter().enumerate() {
            let row = ui.add(
                self.plate,
                Widget::container()
                    .with_action(ActionId(ROW_BASE + i as u32))
                    .with_radius(CAPSULE)
                    // The field stays focused while a row is pressed: blurring would close the
                    // card (and drop its rows) before the click is done.
                    .keeping_focus(),
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
            match item.kind {
                // Pages show their site's icon (the generic one while none is cached).
                Kind::Bookmark => {
                    favicon_or(ui, row, &item.url, ICON_SIZE, favicons, Icon::Bookmark);
                }
                Kind::History => {
                    favicon_or(ui, row, &item.url, ICON_SIZE, favicons, Icon::History);
                }
                Kind::Search | Kind::Address => {
                    let icon = if item.kind == Kind::Search { Icon::Search } else { Icon::Forward };
                    ui.add(
                        row,
                        Widget::icon(icon, ICON_SIZE).with_paint(Paint::Secondary),
                        Style { flex_shrink: 0.0, ..Default::default() },
                    );
                }
            }
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

    /// The icon source for the address field while typing (see `typing_icon_url`).
    pub fn typing_icon_url(&self) -> Option<&str> {
        typing_icon_url(self.selected(), &self.items)
    }

    pub fn selected(&self) -> Option<&Suggestion> {
        self.selected.and_then(|i| self.items.get(i))
    }

    pub fn get(&self, index: usize) -> Option<&Suggestion> {
        self.items.get(index)
    }

    /// Keeps the list attached to the address field when the window or layout changes.
    pub fn reposition(&mut self, ui: &mut Ui, anchor: Frame) {
        if self.is_visible() {
            let target = self.target(anchor);
            if ui.frame(self.plate) != target {
                self.popover.retarget(ui, target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn sug(kind: Kind, url: &str) -> Suggestion {
        Suggestion { kind, title: String::new(), url: url.into() }
    }

    #[test]
    fn provisional_rows_keep_what_still_matches() {
        let mut kept = sug(Kind::History, "https://music.example/");
        kept.title = "Music".into();
        let previous = [
            sug(Kind::Search, "https://s/?q=mu"),
            kept.clone(),
            sug(Kind::Bookmark, "https://mail.example/"),
        ];
        let rows = provisional("mus", "https://s/?q={}", &previous);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, Kind::Search, "what the input means comes first");
        assert_eq!(rows[1], kept);
    }

    #[test]
    fn typing_icon_choice() {
        let items = [
            sug(Kind::Search, "https://s/?q=x"),
            sug(Kind::History, "https://a.com/"),
            sug(Kind::Address, "https://b.com/"),
        ];
        assert_eq!(typing_icon_url(None, &items), None);
        assert_eq!(typing_icon_url(Some(&items[1]), &items), Some("https://a.com/"));
        assert_eq!(typing_icon_url(Some(&items[0]), &items), None);
        assert_eq!(typing_icon_url(None, &items[2..]), Some("https://b.com/"));
        assert_eq!(typing_icon_url(None, &[]), None);
    }

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
