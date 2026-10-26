//! Find on Page: a glass bar that drips out of the address field (see [`Popover`]) with the
//! search field, the match count ("3 of 17"), a case toggle, previous / next and close.
//!
//! It searches as you type: every change of the field starts a new search in the engine
//! (Chromium's own incremental find, which highlights all matches and scrolls to the first
//! one), and the count arrives back as `EngineEvent::FindResult`. Like Safari it belongs to
//! the page it was opened on: switching tabs or loading another page closes it.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_engine::{Engine, FindRequest, ViewId};
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};

use crate::popover::Popover;

const ACTION_BASE: u32 = 6400;
const PREVIOUS: ActionId = ActionId(ACTION_BASE);
const NEXT: ActionId = ActionId(ACTION_BASE + 1);
const MATCH_CASE: ActionId = ActionId(ACTION_BASE + 2);
const CLOSE: ActionId = ActionId(ACTION_BASE + 3);
const WIDTH: f32 = 470.0;
const HEIGHT: f32 = 48.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 10.0;
const GAP: f32 = 6.0;
const BUTTON: f32 = 34.0;
const COUNT_WIDTH: f32 = 82.0;
const GAP_BELOW_ANCHOR: f32 = 26.0;
/// The bar floats this far inside the page's top edge (below the tab strip and bookmarks).
const PAGE_INSET: f32 = 10.0;
/// Kept free at the window's edges.
const WINDOW_MARGIN: f32 = 8.0;
/// The case toggle's fill while on (accent at this opacity) and off (label colour).
const TOGGLE_ON: f32 = 0.18;
const BUTTON_FILL: f32 = 0.06;

/// A click in the find bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindAction {
    Previous,
    Next,
    ToggleMatchCase,
    Close,
}

pub struct FindBar {
    popover: Popover,
    field: NodeId,
    count: NodeId,
    case_button: NodeId,
    /// The page being searched (while open).
    view: Option<ViewId>,
    /// What the engine was last asked to find.
    query: String,
    match_case: bool,
    /// Where the bar was last sent (see `follow`).
    target: Option<Frame>,
    /// The first match of the current search was selected.
    selected_first: bool,
}

impl FindBar {
    /// `parent` is the toolbar, so the bar's glass flows out of the address field's.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            HEIGHT / 2.0,
            Style {
                size: Size { width: length(WIDTH), height: length(HEIGHT) },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(GAP), height: length(0.0) },
                padding: Rect {
                    left: length(PADDING + INSET),
                    right: length(PADDING),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        )
        .detached();
        let plate = popover.root();
        ui.add(
            plate,
            Widget::icon(Icon::Search, 17.0).with_paint(Paint::Secondary),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        let field = ui.add_text_field(
            plate,
            tr("Find on Page"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let count = ui.add_label(
            plate,
            "",
            TextStyle::Footnote,
            Style {
                flex_shrink: 0.0,
                size: Size { width: length(COUNT_WIDTH), height: auto() },
                ..Default::default()
            },
        );
        if let Some(w) = ui.widget_mut(count) {
            w.set_paint(Paint::Secondary);
        }
        ui.set_label_align(count, axion_ui::Align::Center);
        let case_button = button(ui, plate, MATCH_CASE, BUTTON_FILL);
        ui.add_label(case_button, "Aa", TextStyle::Callout, Style::default());
        let previous = button(ui, plate, PREVIOUS, BUTTON_FILL);
        ui.add(previous, Widget::icon(Icon::ChevronUp, 17.0).with_paint(Paint::Accent), Style::default());
        let next = button(ui, plate, NEXT, BUTTON_FILL);
        ui.add(next, Widget::icon(Icon::ChevronDown, 17.0).with_paint(Paint::Accent), Style::default());
        let close = button(ui, plate, CLOSE, BUTTON_FILL);
        ui.add(close, Widget::icon(Icon::Close, 15.0).with_paint(Paint::Secondary), Style::default());
        Self {
            popover,
            field,
            count,
            case_button,
            view: None,
            query: String::new(),
            match_case: false,
            target: None,
            selected_first: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    /// The page being searched, while the bar is open.
    pub fn view(&self) -> Option<ViewId> {
        self.view.filter(|_| self.is_open())
    }

    pub fn field(&self) -> NodeId {
        self.field
    }

    pub fn action(id: ActionId) -> Option<FindAction> {
        Some(match id {
            PREVIOUS => FindAction::Previous,
            NEXT => FindAction::Next,
            MATCH_CASE => FindAction::ToggleMatchCase,
            CLOSE => FindAction::Close,
            _ => return None,
        })
    }

    /// Drips out of `anchor` (the address field) to search `view`. The field keeps the last
    /// search, which runs again on this page.
    /// `page_top`: where the page starts (logical px); the bar floats just inside it, under
    /// the tab strip and bookmarks bar.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        page_top: f32,
        view: ViewId,
    ) {
        if self.view != Some(view) || !self.is_open() {
            // Searched again (below) on the page it now belongs to.
            self.query.clear();
            ui.set_text(self.count, "");
        }
        self.view = Some(view);
        self.match_case = config.find.match_case;
        self.show_match_case(ui);
        let target = target(anchor, window_width, page_top);
        self.target = Some(target);
        self.popover.open(ui, config, anchor, target);
    }

    /// Glides along when the address field moved (the window or the bar changed); call every
    /// frame.
    pub fn follow(&mut self, ui: &mut Ui, anchor: Frame, window_width: f32, page_top: f32) {
        let target = target(anchor, window_width, page_top);
        if self.is_open() && self.target != Some(target) {
            self.target = Some(target);
            self.popover.retarget(ui, target);
        }
    }

    /// Flows back into the address field and removes the highlights from the page.
    pub fn close(&mut self, ui: &mut Ui, config: &Config, engine: &mut impl Engine) {
        if let Some(view) = self.view.take() {
            engine.stop_finding(view);
        }
        self.query.clear();
        self.popover.close(ui, config);
    }

    /// Advances the drip and searches when the field changed; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32, engine: &mut impl Engine) {
        self.popover.animate(ui, dt);
        let Some(view) = self.view() else { return };
        let text = ui.field_text(self.field).unwrap_or_default();
        if text == self.query {
            return;
        }
        self.query = text.to_owned();
        self.selected_first = false;
        if self.query.is_empty() {
            engine.stop_finding(view);
            ui.set_text(self.count, "");
            return;
        }
        let request =
            FindRequest { text: self.query.clone(), forward: true, match_case: self.match_case, next: false };
        engine.find(view, &request);
    }

    /// Moves to the next (or previous) match.
    pub fn step(&mut self, engine: &mut impl Engine, forward: bool) {
        let Some(view) = self.view() else { return };
        if self.query.is_empty() {
            return;
        }
        let request =
            FindRequest { text: self.query.clone(), forward, match_case: self.match_case, next: true };
        engine.find(view, &request);
    }

    /// Flips case-sensitive matching (remembered in `config.find.match_case`) and searches
    /// again.
    pub fn toggle_match_case(&mut self, ui: &mut Ui, config: &mut Config) {
        self.match_case = !self.match_case;
        config.find.match_case = self.match_case;
        self.show_match_case(ui);
        // A new search with the new rule, on the next frame.
        self.query.clear();
    }

    /// The engine counted matches for the page.
    pub fn set_result(
        &mut self,
        ui: &mut Ui,
        engine: &mut impl Engine,
        view: ViewId,
        (active, count, final_update): (u32, u32, bool),
    ) {
        if self.view() != Some(view) || self.query.is_empty() {
            return;
        }
        // Chromium highlights the matches of a new search without selecting one: select (and
        // scroll to) the first, as typing does in every browser.
        if final_update && active == 0 && count > 0 && !self.selected_first {
            self.selected_first = true;
            self.step(engine, true);
        }
        let text = match count {
            0 if final_update => tr("Not found").to_owned(),
            0 => return,
            _ => axion_i18n::tr_fmt!("{index} of {count}", index = active.max(1), count = count),
        };
        ui.set_text(self.count, &text);
    }

    fn show_match_case(&self, ui: &mut Ui) {
        if let Some(w) = ui.widget_mut(self.case_button) {
            w.background = if self.match_case {
                Background::Fill(Paint::Accent, TOGGLE_ON)
            } else {
                Background::Fill(Paint::Label, BUTTON_FILL)
            };
        }
    }
}

/// Below the address field, right-aligned with it, inside the window.
fn target(anchor: Frame, window_width: f32, page_top: f32) -> Frame {
    let right = anchor.origin.x + anchor.size.x;
    let left =
        (right - WIDTH).clamp(WINDOW_MARGIN, (window_width - WIDTH - WINDOW_MARGIN).max(WINDOW_MARGIN));
    let top = (anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR).max(page_top + PAGE_INSET);
    Frame::new(left, top, WIDTH, HEIGHT)
}

fn button(ui: &mut Ui, parent: NodeId, action: ActionId, fill: f32) -> NodeId {
    ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_radius(CAPSULE)
            .with_background(Background::Fill(Paint::Label, fill)),
        Style {
            size: Size { width: length(BUTTON), height: length(BUTTON) },
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_stays_inside_the_window_below_the_field() {
        let field = Frame::new(400.0, 16.0, 600.0, 36.0);
        let t = target(field, 1440.0, 0.0);
        assert_eq!(t.origin.x + t.size.x, 1000.0);
        assert_eq!(t.origin.y, 16.0 + 36.0 + GAP_BELOW_ANCHOR);
        // Under the tab strip and bookmarks bar when they push the page down.
        assert_eq!(target(Frame::new(400.0, 16.0, 600.0, 36.0), 1440.0, 150.0).origin.y, 160.0);
        // A narrow window: pushed in from the left edge.
        let t = target(Frame::new(10.0, 16.0, 200.0, 36.0), 600.0, 0.0);
        assert_eq!(t.origin.x, WINDOW_MARGIN);
    }

    #[test]
    fn actions_round_trip() {
        for action in [FindAction::Previous, FindAction::Next, FindAction::ToggleMatchCase, FindAction::Close]
        {
            let id = match action {
                FindAction::Previous => PREVIOUS,
                FindAction::Next => NEXT,
                FindAction::ToggleMatchCase => MATCH_CASE,
                FindAction::Close => CLOSE,
            };
            assert_eq!(FindBar::action(id), Some(action));
        }
        assert_eq!(FindBar::action(ActionId(ACTION_BASE + 4)), None);
    }
}
