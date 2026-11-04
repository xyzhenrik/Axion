//! A dropdown list: a glass popover that drips out of a picker button (the interface language,
//! the language pages are translated into) and holds a scrollable list with the current choice
//! ticked and a search field on top. Up / Down move the highlight, Enter chooses it, Escape
//! closes the list, typing filters it; the list opens scrolled to the current choice.
//!
//! It is built on [`Popover`] in a plane of its own above Settings, never joined to the sheet
//! under it (`unjoined`: the sheet is another glass plane, a bridge between the two would only
//! show as a smear). Rows carry the action ids their owner gives them, so a click arrives as an
//! ordinary `UiEvent::Clicked` for the owner's own action; [`Dropdown::owns`] lets the browser
//! close the list when one of its rows was clicked.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

use crate::menu::{Placement, target_frame};
use crate::popover::Popover;

const RADIUS: f32 = 22.0;
const PADDING: f32 = 8.0;
const ROW_HEIGHT: f32 = 36.0;
const ROW_RADIUS: f32 = 12.0;
const FIELD_HEIGHT: f32 = 36.0;
const FIELD_GAP: f32 = 6.0;
const MIN_WIDTH: f32 = 260.0;
const TICK: f32 = 16.0;
/// The keyboard highlight (accent at this opacity).
const HIGHLIGHT_OPACITY: f32 = 0.14;

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropdownItem {
    pub label: String,
    /// The click id of the row (its owner's action for this choice).
    pub action: ActionId,
    /// The current choice: ticked, and the list opens scrolled to it.
    pub selected: bool,
}

pub struct Dropdown {
    popover: Popover,
    field: NodeId,
    viewport: NodeId,
    list: NodeId,
    items: Vec<DropdownItem>,
    /// The rows that match the query, as indices into `items`.
    shown: Vec<usize>,
    /// The highlighted row, as an index into `shown`.
    highlighted: usize,
    scroll: f32,
    /// The query the list is filtered with.
    query: String,
    visible_rows: usize,
    anchor: Frame,
    window: Vec2,
}

/// The indices of the `labels` that contain every word of `query` (case-insensitive).
pub fn filter(labels: &[&str], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    labels
        .iter()
        .enumerate()
        .filter(|(_, label)| {
            let label = label.to_lowercase();
            words.iter().all(|w| label.contains(w))
        })
        .map(|(i, _)| i)
        .collect()
}

/// The scroll that shows row `row` (rows of `ROW_HEIGHT` in a viewport of `view` px, `rows` in
/// all) with the least movement from `scroll`.
pub fn scroll_to_show(scroll: f32, row: usize, rows: usize, view: f32) -> f32 {
    let (top, bottom) = (row as f32 * ROW_HEIGHT, (row + 1) as f32 * ROW_HEIGHT);
    let max = (rows as f32 * ROW_HEIGHT - view).max(0.0);
    let scroll = if top < scroll {
        top
    } else if bottom > scroll + view {
        bottom - view
    } else {
        scroll
    };
    scroll.clamp(0.0, max)
}

/// The scroll that puts row `row` in the middle of the viewport (as far as the list allows).
pub fn scroll_to_centre(row: usize, rows: usize, view: f32) -> f32 {
    let max = (rows as f32 * ROW_HEIGHT - view).max(0.0);
    ((row as f32 + 0.5) * ROW_HEIGHT - view / 2.0).clamp(0.0, max)
}

/// The highlight after moving `delta` rows among `rows` (it stops at the ends).
pub fn step(highlighted: usize, delta: isize, rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    (highlighted as isize + delta).clamp(0, rows as isize - 1) as usize
}

impl Dropdown {
    /// Builds the (closed) dropdown as a plane of its own on the root of `ui`: build it after
    /// the sheets it opens over, before the planes that must stay above it.
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.root();
        let popover = Popover::build(
            ui,
            root,
            RADIUS,
            Style {
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(PADDING),
                    bottom: length(PADDING),
                },
                gap: Size { width: length(0.0), height: length(FIELD_GAP) },
                ..Default::default()
            },
        )
        .unjoined();
        let card = popover.root();
        let field_box = ui.add(
            card,
            Widget::container().with_radius(CAPSULE).with_background(Background::Fill(Paint::Label, 0.07)),
            Style {
                size: Size { width: percent(1.0), height: length(FIELD_HEIGHT) },
                flex_shrink: 0.0,
                padding: Rect {
                    left: length(12.0),
                    right: length(12.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                gap: Size { width: length(8.0), height: length(0.0) },
                ..Default::default()
            },
        );
        ui.add(field_box, Widget::icon(Icon::Search, 16.0).with_paint(Paint::Secondary), Style::default());
        let field = ui.add_text_field(
            field_box,
            tr("Search"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let viewport = ui.add(
            card,
            Widget::container().with_clip(),
            Style {
                size: Size { width: percent(1.0), height: length(ROW_HEIGHT) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
        );
        let list = ui.add(
            viewport,
            Widget::container(),
            Style { flex_direction: FlexDirection::Column, flex_shrink: 0.0, ..Default::default() },
        );
        Self {
            popover,
            field,
            viewport,
            list,
            items: Vec::new(),
            shown: Vec::new(),
            highlighted: 0,
            scroll: 0.0,
            query: String::new(),
            visible_rows: 8,
            anchor: Frame::default(),
            window: Vec2::ZERO,
        }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }

    /// Whether `action` is the id of a row of the open list.
    pub fn owns(&self, action: ActionId) -> bool {
        self.is_open() && self.items.iter().any(|i| i.action == action)
    }

    /// Opens the list below `anchor` (the picker button), scrolled to the current choice.
    /// Returns the focus events of the search field.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window: Vec2,
        items: Vec<DropdownItem>,
    ) -> Vec<UiEvent> {
        self.items = items;
        self.anchor = anchor;
        self.window = window;
        self.visible_rows = config.ui.dropdown_visible_rows.max(1) as usize;
        self.query.clear();
        ui.set_field_text(self.field, "");
        self.shown = (0..self.items.len()).collect();
        self.highlighted = self.items.iter().position(|i| i.selected).unwrap_or(0);
        self.rebuild(ui);
        self.scroll = scroll_to_centre(self.highlighted, self.shown.len(), self.view_height());
        self.apply_scroll(ui);
        let target = self.target();
        self.popover.open(ui, config, anchor, target);
        ui.focus(self.field)
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        if self.popover.is_open() {
            self.popover.close(ui, config);
            let _ = ui.blur();
        }
    }

    /// The viewport's height: the rows that match (at most the configured number), or one row
    /// for "No matches".
    fn view_height(&self) -> f32 {
        self.shown.len().clamp(1, self.visible_rows) as f32 * ROW_HEIGHT
    }

    fn target(&self) -> Frame {
        let width = self.anchor.size.x.max(MIN_WIDTH);
        let height = 2.0 * PADDING + FIELD_HEIGHT + FIELD_GAP + self.view_height();
        target_frame(Placement::BelowStart, self.anchor, self.window, width, height, 8.0)
    }

    /// Builds the rows of the matching items.
    fn rebuild(&mut self, ui: &mut Ui) {
        ui.clear_children(self.list);
        if self.shown.is_empty() {
            let label = ui.add_label(
                self.list,
                tr("No matches"),
                TextStyle::Body,
                Style {
                    size: Size { width: percent(1.0), height: length(ROW_HEIGHT) },
                    padding: Rect {
                        left: length(12.0),
                        right: length(12.0),
                        top: length(8.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
        }
        for (k, &i) in self.shown.iter().enumerate() {
            let item = &self.items[i];
            let row = ui.add(
                self.list,
                Widget::container().with_action(item.action).with_radius(ROW_RADIUS),
                Style {
                    size: Size { width: percent(1.0), height: length(ROW_HEIGHT) },
                    flex_shrink: 0.0,
                    padding: Rect {
                        left: length(12.0),
                        right: length(12.0),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(8.0), height: length(0.0) },
                    ..Default::default()
                },
            );
            let label = ui.add_label(
                row,
                &item.label,
                TextStyle::Body,
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
            ui.set_label_lines(label, 1);
            if item.selected {
                ui.add(row, Widget::icon(Icon::Check, TICK).with_paint(Paint::Accent), Style::default());
            }
            let _ = k;
        }
        let mut style = ui.style(self.viewport);
        style.size = Size { width: percent(1.0), height: length(self.view_height()) };
        ui.set_style(self.viewport, style);
        self.paint_highlight(ui);
    }

    fn paint_highlight(&self, ui: &mut Ui) {
        let rows = ui.children(self.list);
        // "No matches" is the only child of an empty list.
        if self.shown.is_empty() {
            return;
        }
        for (k, row) in rows.into_iter().enumerate() {
            if let Some(w) = ui.widget_mut(row) {
                w.background = if k == self.highlighted {
                    Background::Fill(Paint::Accent, HIGHLIGHT_OPACITY)
                } else {
                    Background::None
                };
            }
        }
    }

    fn apply_scroll(&self, ui: &mut Ui) {
        let mut style = ui.style(self.list);
        style.margin =
            Rect { left: length(0.0), right: length(0.0), top: length(-self.scroll), bottom: length(0.0) };
        ui.set_style(self.list, style);
    }

    fn max_scroll(&self) -> f32 {
        (self.shown.len() as f32 * ROW_HEIGHT - self.view_height()).max(0.0)
    }

    /// The wheel: `dy` > 0 scrolls up.
    pub fn scroll_by(&mut self, ui: &mut Ui, dy: f32) {
        self.scroll = (self.scroll - dy).clamp(0.0, self.max_scroll());
        self.apply_scroll(ui);
    }

    /// Up / Down: moves the highlight (and scrolls it into view).
    pub fn move_highlight(&mut self, ui: &mut Ui, delta: isize) {
        self.highlighted = step(self.highlighted, delta, self.shown.len());
        self.scroll = scroll_to_show(self.scroll, self.highlighted, self.shown.len(), self.view_height());
        self.apply_scroll(ui);
        self.paint_highlight(ui);
    }

    /// The highlighted row's action (Enter).
    pub fn chosen(&self) -> Option<ActionId> {
        self.shown.get(self.highlighted).map(|&i| self.items[i].action)
    }

    /// Follows the search field, the popover's drip and the window; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if self.popover.is_open() {
            let typed = ui.field_text(self.field).unwrap_or_default().to_owned();
            if typed != self.query {
                self.query = typed;
                self.refilter(ui);
            }
        }
        self.popover.animate(ui, dt);
    }

    fn refilter(&mut self, ui: &mut Ui) {
        let labels: Vec<&str> = self.items.iter().map(|i| i.label.as_str()).collect();
        self.shown = filter(&labels, &self.query);
        // The first match is the one Enter takes.
        self.highlighted = 0;
        self.scroll = 0.0;
        self.rebuild(ui);
        self.apply_scroll(ui);
        let target = self.target();
        self.popover.retarget(ui, target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_every_word_in_any_case() {
        let labels = ["English", "Deutsch (German)", "Español", "Português (Brasil)"];
        assert_eq!(filter(&labels, ""), [0, 1, 2, 3]);
        assert_eq!(filter(&labels, "eng"), [0]);
        assert_eq!(filter(&labels, "GER deu"), [1]);
        assert_eq!(filter(&labels, "pt"), Vec::<usize>::new());
        assert_eq!(filter(&labels, "ñ"), [2]);
    }

    #[test]
    fn the_list_scrolls_the_highlight_into_view() {
        let view = 5.0 * ROW_HEIGHT;
        // Already visible: no movement.
        assert_eq!(scroll_to_show(0.0, 3, 58, view), 0.0);
        // Below the view: the row ends at the bottom edge.
        assert_eq!(scroll_to_show(0.0, 7, 58, view), 8.0 * ROW_HEIGHT - view);
        // Above the view: the row starts at the top edge.
        assert_eq!(scroll_to_show(10.0 * ROW_HEIGHT, 4, 58, view), 4.0 * ROW_HEIGHT);
        // Never past the ends.
        assert_eq!(scroll_to_show(0.0, 57, 58, view), 53.0 * ROW_HEIGHT);
    }

    #[test]
    fn opening_centres_the_current_choice() {
        let view = 5.0 * ROW_HEIGHT;
        assert_eq!(scroll_to_centre(0, 58, view), 0.0);
        assert_eq!(scroll_to_centre(30, 58, view), 28.0 * ROW_HEIGHT);
        assert_eq!(scroll_to_centre(57, 58, view), 53.0 * ROW_HEIGHT);
        // A short list does not scroll at all.
        assert_eq!(scroll_to_centre(2, 3, 3.0 * ROW_HEIGHT), 0.0);
    }

    #[test]
    fn the_highlight_stops_at_the_ends() {
        assert_eq!(step(0, -1, 5), 0);
        assert_eq!(step(4, 1, 5), 4);
        assert_eq!(step(2, 1, 5), 3);
        assert_eq!(step(0, 1, 0), 0);
    }

    fn built() -> (Ui, Dropdown) {
        let config = Config::default();
        let mut ui = Ui::new(axion_ui::Theme::new(&config, false), 1.0);
        let dropdown = Dropdown::build(&mut ui);
        (ui, dropdown)
    }

    fn items(n: usize, selected: usize) -> Vec<DropdownItem> {
        (0..n)
            .map(|i| DropdownItem {
                label: format!("Language {i}"),
                action: ActionId(1100 + i as u32),
                selected: i == selected,
            })
            .collect()
    }

    #[test]
    fn opens_on_the_current_choice_and_chooses_with_the_keyboard() {
        let config = Config::default();
        let (mut ui, mut dropdown) = built();
        let anchor = Frame::new(400.0, 200.0, 200.0, 36.0);
        let _ = dropdown.open(&mut ui, &config, anchor, Vec2::new(1200.0, 800.0), items(58, 30));
        assert!(dropdown.is_open());
        assert_eq!(dropdown.chosen(), Some(ActionId(1130)));
        assert!(dropdown.scroll > 0.0, "scrolled to the current choice");
        assert!(dropdown.owns(ActionId(1100)) && !dropdown.owns(ActionId(5)));
        dropdown.move_highlight(&mut ui, 1);
        assert_eq!(dropdown.chosen(), Some(ActionId(1131)));
        dropdown.move_highlight(&mut ui, -3);
        assert_eq!(dropdown.chosen(), Some(ActionId(1128)));
        // Only the configured number of rows is visible; all 58 exist.
        assert_eq!(ui.children(dropdown.list).len(), 58);
        assert_eq!(dropdown.view_height(), config.ui.dropdown_visible_rows as f32 * ROW_HEIGHT);
    }

    #[test]
    fn typing_filters_and_the_first_match_is_chosen() {
        let config = Config::default();
        let (mut ui, mut dropdown) = built();
        let anchor = Frame::new(400.0, 200.0, 200.0, 36.0);
        let _ = dropdown.open(&mut ui, &config, anchor, Vec2::new(1200.0, 800.0), items(58, 0));
        ui.set_field_text(dropdown.field, "language 4");
        dropdown.animate(&mut ui, 0.016);
        // Every label with a 4: 4, 14, 24, 34, 40 to 49, 54.
        assert_eq!(dropdown.shown.len(), 15);
        assert_eq!(dropdown.chosen(), Some(ActionId(1104)));
        ui.set_field_text(dropdown.field, "zzz");
        dropdown.animate(&mut ui, 0.016);
        assert_eq!(dropdown.chosen(), None);
        dropdown.close(&mut ui, &config);
        assert!(!dropdown.is_open());
    }
}
