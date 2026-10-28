//! Tab search (Ctrl+Shift+A): a glass card over the dimmed window with a search field and a
//! list of matching open tabs, recently closed tabs and history. Up / Down move the
//! selection, Enter opens it, Escape closes the card.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_engine::ViewId;
use axion_i18n::tr;
use axion_physics::{Animated, Spring};
use axion_store::Store;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

use crate::favicons::{Favicons, favicon_or};
use crate::tabs::Tabs;
use crate::url::display_host;

const BASE: u32 = 80_000;
const BACKDROP: ActionId = ActionId(BASE);
const ROW_BASE: u32 = BASE + 10;
const WIDTH: f32 = 580.0;
const RADIUS: f32 = 24.0;
const PADDING: f32 = 8.0;
const FIELD_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 50.0;
const ICON: f32 = 20.0;
const TOP: f32 = 96.0;
const START_SCALE: f32 = 0.94;
const BACKDROP_DIM: f32 = 0.22;
/// Rows shown at most.
pub const MAX_ROWS: usize = 10;
/// History entries kept for a typed query even when many tabs match.
const HISTORY_RESERVE: usize = 3;
/// The selected row's highlight (accent at this opacity).
const SELECTED_OPACITY: f32 = 0.16;

/// What a result opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultKind {
    /// An open tab (`private`: in the private group).
    Tab {
        view: ViewId,
        private: bool,
    },
    /// A tab closed a moment ago.
    Closed,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub kind: ResultKind,
    pub title: String,
    pub url: String,
    /// The tab group's name for an open tab in one.
    pub group: Option<String>,
}

impl SearchResult {
    fn badge(&self) -> &'static str {
        match self.kind {
            ResultKind::Tab { .. } => tr("Open Tab"),
            ResultKind::Closed => tr("Recently Closed"),
            ResultKind::History => tr("History"),
        }
    }
}

/// Whether every word of `query` (lowercase) occurs in one of the `haystacks` (lowercase).
fn matches(query: &str, haystacks: &[&str]) -> bool {
    query.split_whitespace().all(|word| haystacks.iter().any(|h| h.contains(word)))
}

/// The results for `query`: open tabs of `shown` (and `other`, listed too for the sidebar's
/// other group) whose title, address or group name match, then recently closed tabs, then
/// history (not in private browsing). Without a query: the open tabs, then what was closed.
pub fn search(
    query: &str,
    tabs: &[(&Tabs, bool)],
    closed: &[(String, String)],
    store: Option<&Store>,
    history_limit: usize,
    now: i64,
) -> Vec<SearchResult> {
    let query = query.trim().to_lowercase();
    let mut results: Vec<SearchResult> = Vec::new();
    let typed = !query.is_empty();
    let reserve = if typed && store.is_some() { history_limit.min(HISTORY_RESERVE) } else { 0 };
    let open_cap = MAX_ROWS.saturating_sub(reserve);
    for &(group, private) in tabs {
        for tab in group.all() {
            let label = tab.label();
            let group_name = tab.group.and_then(|id| group.group(id)).map(crate::chrome::group_title);
            let (l, u) = (label.to_lowercase(), tab.url.to_lowercase());
            let g = group_name.as_deref().unwrap_or("").to_lowercase();
            if typed && !matches(&query, &[&l, &u, &g]) {
                continue;
            }
            if results.len() < open_cap {
                results.push(SearchResult {
                    kind: ResultKind::Tab { view: tab.view, private },
                    title: label,
                    url: tab.url.clone(),
                    group: group_name,
                });
            }
        }
    }
    for (url, title) in closed {
        let (l, u) = (title.to_lowercase(), url.to_lowercase());
        if (typed && !matches(&query, &[&l, &u])) || results.len() >= open_cap {
            continue;
        }
        results.push(SearchResult {
            kind: ResultKind::Closed,
            title: if title.is_empty() { display_host(url) } else { title.clone() },
            url: url.clone(),
            group: None,
        });
    }
    if typed
        && let Some(store) = store
        && history_limit > 0
    {
        let room = MAX_ROWS.saturating_sub(results.len()).min(history_limit);
        if let Ok(found) = store.search_history(&query, room + results.len(), now) {
            for h in found {
                if results.len() >= MAX_ROWS || results.iter().any(|r| r.url == h.url) {
                    continue;
                }
                results.push(SearchResult {
                    kind: ResultKind::History,
                    title: if h.title.is_empty() { display_host(&h.url) } else { h.title },
                    url: h.url,
                    group: None,
                });
            }
        }
    }
    results
}

pub struct TabSearch {
    root: NodeId,
    card: NodeId,
    field: NodeId,
    list: NodeId,
    empty: NodeId,
    results: Vec<SearchResult>,
    rows: Vec<NodeId>,
    selected: usize,
    open: bool,
    appear: Animated<f32>,
}

impl TabSearch {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(BACKDROP),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::START,
                padding: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(TOP),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        let card = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                max_size: Size { width: percent(0.94), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(4.0) },
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(PADDING),
                    bottom: length(PADDING),
                },
                ..Default::default()
            },
        );
        let field_box = ui.add(
            card,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
            Style {
                size: Size { width: percent(1.0), height: length(FIELD_HEIGHT) },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                gap: Size { width: length(10.0), height: length(0.0) },
                ..Default::default()
            },
        );
        ui.add(field_box, Widget::icon(Icon::Search, 18.0).with_paint(Paint::Secondary), Style::default());
        let field = ui.add_text_field(
            field_box,
            tr("Search Tabs"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let list = ui.add(
            card,
            Widget::container(),
            Style { flex_direction: FlexDirection::Column, ..Default::default() },
        );
        let empty = ui.add_label(
            card,
            tr("No matching tabs or history."),
            TextStyle::Footnote,
            Style {
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
                    top: length(10.0),
                    bottom: length(10.0),
                },
                ..Default::default()
            },
        );
        if let Some(w) = ui.widget_mut(empty) {
            w.set_paint(Paint::Secondary);
        }
        ui.set_visible(empty, false);
        ui.set_visible(root, false);
        Self {
            root,
            card,
            field,
            list,
            empty,
            results: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            open: false,
            appear: Animated::new(0.0, Spring::SNAPPY),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn query(&self, ui: &Ui) -> String {
        ui.field_text(self.field).unwrap_or_default().to_owned()
    }

    /// Whether `id` is the backdrop's (a click outside the card closes it).
    pub fn is_backdrop(id: ActionId) -> bool {
        id == BACKDROP
    }

    /// The result row `id` is, if it is one.
    pub fn row_of(id: ActionId) -> Option<usize> {
        (ROW_BASE..ROW_BASE + MAX_ROWS as u32).contains(&id.0).then(|| (id.0 - ROW_BASE) as usize)
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.card).contains(p)
    }

    pub fn open(&mut self, ui: &mut Ui, config: &Config) -> Vec<UiEvent> {
        self.open = true;
        self.appear.spring = if config.glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(
                config.motion.snappy.response * config.motion.speed,
                config.glass.damping(config.motion.snappy.damping),
            )
        };
        self.appear.snap(0.0);
        self.appear.target = 1.0;
        ui.set_field_text(self.field, "");
        ui.set_visible(self.root, true);
        ui.focus(self.field)
    }

    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
        self.appear.target = 0.0;
        let _ = ui.blur();
    }

    pub fn selected(&self) -> Option<&SearchResult> {
        self.results.get(self.selected)
    }

    pub fn result(&self, row: usize) -> Option<&SearchResult> {
        self.results.get(row)
    }

    pub fn move_selection(&mut self, ui: &mut Ui, delta: isize) {
        let n = self.results.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.paint_selection(ui);
    }

    fn paint_selection(&self, ui: &mut Ui) {
        for (i, &row) in self.rows.iter().enumerate() {
            if let Some(w) = ui.widget_mut(row) {
                w.background = if i == self.selected {
                    Background::Fill(Paint::Accent, SELECTED_OPACITY)
                } else {
                    Background::None
                };
            }
        }
    }

    /// Shows `results` (the first is selected).
    pub fn set_results(&mut self, ui: &mut Ui, results: Vec<SearchResult>, favicons: &Favicons) {
        ui.clear_children(self.list);
        self.rows.clear();
        self.selected = 0;
        for (i, result) in results.iter().enumerate() {
            let row = ui.add(
                self.list,
                Widget::container()
                    .with_action(ActionId(ROW_BASE + i as u32))
                    .with_radius(CAPSULE)
                    .without_feedback(),
                Style {
                    size: Size { width: percent(1.0), height: length(ROW_HEIGHT) },
                    padding: Rect {
                        left: length(14.0),
                        right: length(14.0),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(12.0), height: length(0.0) },
                    ..Default::default()
                },
            );
            let fallback = match result.kind {
                ResultKind::Tab { .. } => Icon::Globe,
                ResultKind::Closed | ResultKind::History => Icon::History,
            };
            favicon_or(ui, row, &result.url, ICON, favicons, fallback);
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
            let shrink = Style {
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            };
            ui.add_label(text, &result.title, TextStyle::Body, shrink.clone());
            let sub = match &result.group {
                Some(group) => format!("{group} \u{b7} {}", display_host(&result.url)),
                None => display_host(&result.url),
            };
            let sub = ui.add_label(text, &sub, TextStyle::Footnote, shrink);
            if let Some(w) = ui.widget_mut(sub) {
                w.set_paint(Paint::Secondary);
            }
            let badge = ui.add_label(row, result.badge(), TextStyle::Footnote, Style::default());
            if let Some(w) = ui.widget_mut(badge) {
                w.set_paint(Paint::Secondary);
            }
            self.rows.push(row);
        }
        ui.set_visible(self.empty, results.is_empty());
        self.results = results;
        self.paint_selection(ui);
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if !self.open && k < 0.01 {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_opacity(self.root, k.min(1.0));
        let frame = ui.frame(self.card);
        if frame.size.x > 0.0 {
            let scale = START_SCALE + (1.0 - START_SCALE) * k;
            let size = frame.size * scale;
            let origin = Vec2::new(frame.origin.x + (frame.size.x - size.x) / 2.0, frame.origin.y);
            ui.set_visual_frame(self.card, Some(Frame { origin, size: size.max(Vec2::ONE) }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::{GroupColor, Tab};

    fn tabs() -> Tabs {
        let mut t = Tabs::default();
        for (i, (url, title)) in [
            ("https://rust-lang.org/", "Rust Programming Language"),
            ("https://example.com/a", "Example"),
            ("https://news.example.org/", "Daily News"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut tab = Tab::new(ViewId(i as u32), url);
            tab.title = title.into();
            t.insert(tab, true);
        }
        t
    }

    #[test]
    fn matches_titles_addresses_and_group_names() {
        let mut t = tabs();
        let g = t.create_group("Reading", GroupColor::Green);
        t.set_tab_group(2, Some(g));
        let find = |q: &str| search(q, &[(&t, false)], &[], None, 8, 0);
        assert_eq!(find("").len(), 3);
        assert_eq!(find("rust").len(), 1);
        assert_eq!(find("EXAMPLE").len(), 2);
        // Every word must match somewhere.
        assert_eq!(find("example news").len(), 1);
        // The group's name finds its tabs, and the result names the group.
        let r = find("reading");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].group.as_deref(), Some("Reading"));
        assert!(find("zzz").is_empty());
    }

    #[test]
    fn closed_tabs_follow_the_open_ones() {
        let t = tabs();
        let closed = vec![("https://old.example/".to_owned(), "Old Page".to_owned())];
        let r = search("old", &[(&t, false)], &closed, None, 8, 0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, ResultKind::Closed);
        let all = search("", &[(&t, false)], &closed, None, 8, 0);
        assert_eq!(all.len(), 4);
        assert_eq!(all[3].kind, ResultKind::Closed);
    }

    #[test]
    fn rows_are_capped() {
        let mut t = Tabs::default();
        for i in 0..30 {
            t.insert(Tab::new(ViewId(i), &format!("https://t{i}.example")), true);
        }
        assert_eq!(search("", &[(&t, false)], &[], None, 8, 0).len(), MAX_ROWS);
    }

    #[test]
    fn row_ids_round_trip() {
        assert_eq!(TabSearch::row_of(ActionId(ROW_BASE + 3)), Some(3));
        assert_eq!(TabSearch::row_of(BACKDROP), None);
        assert!(TabSearch::is_backdrop(BACKDROP));
    }
}
