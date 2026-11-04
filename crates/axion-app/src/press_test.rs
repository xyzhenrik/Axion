//! Pointer tests for the lists that float over the page (address suggestions, command palette,
//! tab search, menus): a real press, move and release at a row's pixels through `Ui::handle`
//! must end in `UiEvent::Clicked` with that row's action, however the browser reacts to the
//! events in between (the address field losing focus, an answer replacing the rows) and whatever
//! window-frame region (move / resize) lies under it.
//!
//! The browser's own reaction to the press is mirrored by the `between` closures: a lost focus
//! closes the suggestion card (`Browser::handle_ui_events`, `FocusChanged { node: None }`).

use axion_compositor::Rect as Frame;
use axion_config::{Config, Keymap};
use axion_ui::taffy::NodeId;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Ui, UiEvent, UiInput, Widget};
use glam::Vec2;

use crate::chrome::Chrome;
use crate::favicons::Favicons;
use crate::menu::{Menu, MenuAction, MenuItem, PAGE_MENU_BASE, Placement};
use crate::palette::CommandPalette;
use crate::popover::Popover;
use crate::suggestions::{Kind, Suggestion, SuggestionList};
use crate::tab_search::{ResultKind, SearchResult, TabSearch};
use crate::window_frame::{resize_enabled, resize_hit};

const WINDOW: Vec2 = Vec2::new(1400.0, 900.0);
const FRAME: f32 = 1.0 / 60.0;

/// A chrome in a 1400 x 900 window with its layout settled.
struct Rig {
    config: Config,
    ui: Ui,
    chrome: Chrome,
    favicons: Favicons,
}

impl Rig {
    fn new() -> Self {
        let config = Config::default();
        let mut ui = Ui::new(axion_ui::Theme::new(&config, false), 1.0);
        let chrome = Chrome::build(&mut ui, &config);
        ui.set_viewport(WINDOW, 1.0);
        let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
        let favicons = Favicons::new(device, queue, None);
        let mut rig = Self { config, ui, chrome, favicons };
        rig.settle(|_| {});
        rig
    }

    /// Runs the animations to rest, `each` once per frame.
    fn settle(&mut self, mut each: impl FnMut(&mut Ui)) {
        for _ in 0..240 {
            self.chrome.animate_all(&mut self.ui, FRAME);
            each(&mut self.ui);
            self.chrome.apply_layout(&mut self.ui, &self.config, WINDOW);
            self.ui.layout();
        }
    }

    fn page_top(&self) -> f32 {
        self.chrome.page_rect(&self.config, WINDOW).origin.y
    }
}

/// A left click at `p` as the browser feeds it: move, press, (the browser reacts to the press's
/// events in `between`), move again, release. Returns every event of the click.
fn click(ui: &mut Ui, p: Vec2, between: impl FnOnce(&mut Ui, &[UiEvent])) -> Vec<UiEvent> {
    ui.handle(UiInput::PointerMove(p));
    let (_, mut events) = ui.handle(UiInput::PointerDown);
    between(ui, &events);
    ui.handle(UiInput::PointerMove(p));
    events.extend(ui.handle(UiInput::PointerUp).1);
    events
}

fn clicked(events: &[UiEvent]) -> Vec<ActionId> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Clicked { action, .. } => Some(*action),
            _ => None,
        })
        .collect()
}

fn focus_lost(events: &[UiEvent]) -> bool {
    events.iter().any(|e| matches!(e, UiEvent::FocusChanged { node: None }))
}

/// The visible node that carries `action`.
fn node_with_action(ui: &Ui, action: ActionId) -> Option<NodeId> {
    fn find(ui: &Ui, node: NodeId, action: ActionId) -> Option<NodeId> {
        if ui.is_visible(node) && ui.widget(node).and_then(|w| w.action) == Some(action) {
            return Some(node);
        }
        ui.children(node).into_iter().find_map(|c| find(ui, c, action))
    }
    find(ui, ui.root(), action)
}

fn centre_of(ui: &Ui, action: ActionId) -> Vec2 {
    let node = node_with_action(ui, action).unwrap_or_else(|| panic!("no node with action {}", action.0));
    ui.frame(node).center()
}

fn sug(kind: Kind, title: &str, url: &str) -> Suggestion {
    Suggestion { kind, title: title.into(), url: url.into() }
}

fn three() -> Vec<Suggestion> {
    vec![
        sug(Kind::Search, "Search for sda", "https://search.example/?q=sda"),
        sug(Kind::History, "SDA one", "https://one.example/"),
        sug(Kind::Bookmark, "SDA two", "https://two.example/"),
    ]
}

/// What `Browser::handle_ui_events` does with the events of a press on the suggestion card.
fn browser_reacts(list: &mut SuggestionList, ui: &mut Ui, config: &Config, events: &[UiEvent]) {
    if focus_lost(events) {
        list.hide(ui, config);
    }
}

/// An address field being edited with `items` shown in the card below it, everything settled.
fn suggestions_rig(items: Vec<Suggestion>) -> (Rig, SuggestionList) {
    let mut rig = Rig::new();
    let mut list = SuggestionList::build(&mut rig.ui, rig.chrome.bar_node());
    rig.ui.set_field_text(rig.chrome.address_field, "sda");
    rig.ui.focus(rig.chrome.address_field);
    let anchor = rig.chrome.address_frame(&rig.ui);
    list.update(&mut rig.ui, &rig.config, "sda", anchor, &rig.favicons, || items);
    assert!(list.is_visible());
    rig.settle(|ui| list.animate(ui, FRAME));
    (rig, list)
}

/// The bug: a press on a row blurred the address field, the browser closed the card on the lost
/// focus and cleared its rows, and the release found no row to click. Pressing a row keeps the
/// focus, and the click reaches the row for every row of the card.
#[test]
fn a_suggestion_row_is_clicked_by_a_real_press_and_release() {
    for row in 0..3 {
        let (mut rig, mut list) = suggestions_rig(three());
        let field = rig.chrome.address_field;
        let at = list.row_center(&rig.ui, row).expect("row");
        let config = rig.config.clone();
        let mut pressed_events = Vec::new();
        let events = click(&mut rig.ui, at, |ui, events| {
            pressed_events = events.to_vec();
            browser_reacts(&mut list, ui, &config, events);
        });
        assert!(!focus_lost(&pressed_events), "row {row}: the press leaves the focus where it is");
        assert_eq!(rig.ui.focused(), Some(field), "row {row}");
        assert_eq!(clicked(&events), [ActionId(5000 + row as u32)], "row {row}");
        assert_eq!(
            SuggestionList::row_action(ActionId(5000 + row as u32)).and_then(|i| list.get(i)),
            three().get(row),
            "the click finds the row's suggestion"
        );
    }
}

/// An answer of the worker replaces the rows between press and release: the click still lands,
/// on the row that was pressed, and the new rows appear once the pointer is let go.
#[test]
fn a_worker_answer_between_press_and_release_does_not_cancel_the_click() {
    let (mut rig, mut list) = suggestions_rig(three());
    let at = list.row_center(&rig.ui, 1).expect("row");
    let anchor = rig.chrome.address_frame(&rig.ui);
    let config = rig.config.clone();
    let answer = vec![
        sug(Kind::Search, "Search for sda", "https://search.example/?q=sda"),
        sug(Kind::History, "Other", "https://other.example/"),
    ];
    let events = click(&mut rig.ui, at, |ui, events| {
        browser_reacts(&mut list, ui, &config, events);
        list.set_items(ui, &config, "sda", answer.clone(), anchor, &rig.favicons);
        ui.layout();
    });
    assert_eq!(clicked(&events), [ActionId(5001)]);
    assert_eq!(
        list.get(1).map(|s| s.url.as_str()),
        Some("https://one.example/"),
        "the pressed row's meaning held"
    );
    // Let go: the answer is applied.
    list.flush_pending(&mut rig.ui, &config, anchor, &rig.favicons);
    assert_eq!(list.get(1).map(|s| s.url.as_str()), Some("https://other.example/"));
    assert!(list.get(2).is_none());
}

/// A card that stays open while the field is not focused (the focus left some other way) still
/// takes clicks on its rows.
#[test]
fn a_blurred_field_does_not_stop_a_press_on_an_open_card() {
    let (mut rig, list) = suggestions_rig(three());
    rig.ui.blur();
    let at = list.row_center(&rig.ui, 2).expect("row");
    let events = click(&mut rig.ui, at, |_, _| {});
    assert_eq!(clicked(&events), [ActionId(5002)]);
}

/// The window's move and resize regions never take a press on the suggestion card: not on a row
/// and not on the card's padding, and the card stays clear of the resize border.
#[test]
fn window_frame_regions_never_claim_the_suggestion_card() {
    let (rig, list) = suggestions_rig(three());
    let card = list.card(&rig.ui);
    let enabled = resize_enabled(&rig.config, false, false);
    assert!(enabled);
    let border = rig.config.window.resize_border_px;
    let mut checked = 0;
    for ix in 0..=20 {
        for iy in 0..=10 {
            let p = card.origin + Vec2::new(card.size.x * ix as f32 / 20.0, card.size.y * iy as f32 / 10.0);
            // The corners are rounded: only points the UI covers count.
            if !rig.ui.covers(p) || p.x >= card.origin.x + card.size.x || p.y >= card.origin.y + card.size.y {
                continue;
            }
            checked += 1;
            assert!(!rig.chrome.is_drag_region(&rig.ui, &rig.config, p), "{p:?} moves the window");
            assert!(resize_hit(p, WINDOW, border, enabled).is_none(), "{p:?} resizes the window");
        }
    }
    assert!(checked > 100, "{checked} points checked");
}

/// A popover built inside the bar it flows out of (the page menu, Shields, the palette, the
/// suggestions) that reaches up into the bar's rows is not free toolbar space.
#[test]
fn a_popover_over_the_bar_is_not_a_drag_region() {
    let mut rig = Rig::new();
    let mut popover = Popover::build(
        &mut rig.ui,
        rig.chrome.bar_node(),
        22.0,
        Style { size: Size { width: length(300.0), height: length(220.0) }, ..Default::default() },
    );
    let anchor = rig.chrome.address_frame(&rig.ui);
    let target = Frame::new(500.0, 4.0, 300.0, 220.0);
    popover.open(&mut rig.ui, &rig.config, anchor, target);
    rig.settle(|ui| {
        popover.animate(ui, FRAME);
    });
    let inside = Vec2::new(650.0, 40.0);
    assert!(rig.page_top() > inside.y, "the point lies in the bar's rows");
    assert!(rig.ui.covers(inside));
    assert!(
        !rig.chrome.is_drag_region(&rig.ui, &rig.config, inside),
        "the popover's padding moves the window"
    );
}

/// The command palette's rows click while the palette's own field has the focus.
#[test]
fn a_palette_row_is_clicked() {
    let mut rig = Rig::new();
    let mut palette = CommandPalette::build(&mut rig.ui, rig.chrome.bar_node());
    let anchor = rig.chrome.address_frame(&rig.ui);
    let page_top = rig.page_top();
    palette.open(&mut rig.ui, &rig.config, &Keymap::default(), anchor, WINDOW.x, page_top);
    rig.ui.focus(palette.field());
    rig.settle(|ui| palette.animate(ui, FRAME));
    for row in [0, 2] {
        let id = ActionId(6500 + row);
        let at = centre_of(&rig.ui, id);
        let events = click(&mut rig.ui, at, |_, _| {});
        assert_eq!(clicked(&events), [id], "palette row {row}");
        assert_eq!(CommandPalette::row_action(id), Some(row as usize));
    }
}

/// The tab search's results click with the search field focused, and the search field keeps
/// working as the backdrop-click test (a press on the backdrop is a different action).
#[test]
fn a_tab_search_row_is_clicked() {
    let mut rig = Rig::new();
    let mut search = TabSearch::build(&mut rig.ui);
    let results = (0..4)
        .map(|i| SearchResult {
            kind: ResultKind::History,
            title: format!("Page {i}"),
            url: format!("https://page{i}.example/"),
            group: None,
        })
        .collect();
    search.open(&mut rig.ui, &rig.config);
    search.set_results(&mut rig.ui, results, &rig.favicons);
    rig.settle(|ui| search.animate(ui, FRAME));
    assert!(search.is_open());
    for row in [0usize, 3] {
        let at = rig
            .ui
            .children(rig.ui.root())
            .into_iter()
            .find_map(|n| find_row(&rig.ui, n, row))
            .unwrap_or_else(|| panic!("no row {row}"));
        let events = click(&mut rig.ui, at, |_, _| {});
        let rows: Vec<Option<usize>> = clicked(&events).into_iter().map(TabSearch::row_of).collect();
        assert_eq!(rows, [Some(row)], "tab search row {row}");
    }
}

fn find_row(ui: &Ui, node: NodeId, row: usize) -> Option<Vec2> {
    if ui.is_visible(node) && ui.widget(node).and_then(|w| w.action).and_then(TabSearch::row_of) == Some(row)
    {
        return Some(ui.frame(node).center());
    }
    ui.children(node).into_iter().find_map(|c| find_row(ui, c, row))
}

/// The page menu's rows click, also while the address field is being edited.
#[test]
fn a_menu_row_is_clicked_with_or_without_a_focused_field() {
    for editing in [false, true] {
        let mut rig = Rig::new();
        let mut menu = Menu::build(&mut rig.ui, rig.chrome.bar_node(), PAGE_MENU_BASE, Placement::Below);
        let items = vec![
            MenuItem::new(axion_compositor::Icon::Reload, "One", MenuAction::Settings),
            MenuItem::new(axion_compositor::Icon::Close, "Two", MenuAction::Settings),
        ];
        let anchor = Frame::new(1200.0, 20.0, 40.0, 40.0);
        menu.open(&mut rig.ui, &rig.config, anchor, WINDOW, items);
        if editing {
            rig.ui.focus(rig.chrome.address_field);
        }
        rig.settle(|ui| menu.animate(ui, FRAME));
        let id = ActionId(PAGE_MENU_BASE + 1);
        let at = centre_of(&rig.ui, id);
        let events = click(&mut rig.ui, at, |_, _| {});
        assert_eq!(clicked(&events), [id], "editing: {editing}");
    }
}

/// A widget built for a list row keeps the focus; ordinary buttons still take it away.
#[test]
fn only_rows_that_ask_keep_the_focus() {
    let mut rig = Rig::new();
    let field = rig.chrome.address_field;
    rig.ui.focus(field);
    let button = rig.ui.add(
        rig.ui.root(),
        Widget::container().with_action(ActionId(90_999)),
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(700.0), top: length(400.0), right: auto(), bottom: auto() },
            size: Size { width: length(100.0), height: length(40.0) },
            ..Default::default()
        },
    );
    rig.ui.layout();
    let at = rig.ui.frame(button).center();
    let events = click(&mut rig.ui, at, |_, _| {});
    assert!(focus_lost(&events));
    assert_eq!(clicked(&events), [ActionId(90_999)]);
}

/// The welcome tour's language dropdown: a modal plane (the tour) lies under the list, which the
/// browser raises over it. A real press, move and release on the language button and on a row of
/// the list must reach the tour as `Click::LanguagePicker` and as that language's pick, with the
/// search field's focus lost in between (the tour's backdrop and the blur must not swallow it).
#[test]
fn the_tour_language_dropdown_is_clicked_above_the_tour() {
    use crate::dropdown::Dropdown;
    use crate::onboarding::{Click, Onboarding, Pick};
    use axion_config::UiLanguage;

    let config = Config::default();
    let mut ui = Ui::new(axion_ui::Theme::new(&config, false), 1.0);
    ui.set_viewport(WINDOW, 1.0);
    // As in the browser: the list is built first, the tour is a later plane over it.
    let mut dropdown = Dropdown::build(&mut ui);
    let mut tour = Onboarding::build(&mut ui);
    let _ = tour.open(&mut ui, &config, WINDOW, 0);
    let settle = |ui: &mut Ui, tour: &mut Onboarding, dropdown: &mut Dropdown| {
        for _ in 0..240 {
            tour.animate(ui, &config, FRAME);
            dropdown.animate(ui, FRAME);
            ui.layout();
        }
    };
    settle(&mut ui, &mut tour, &mut dropdown);

    // The button is pressed like any tour control.
    let button = tour.language_frame(&ui).expect("the first step has a language button");
    let events = click(&mut ui, button.center(), |_, _| {});
    let picker = clicked(&events);
    assert_eq!(picker.len(), 1);
    assert_eq!(tour.click(picker[0]), Some(Click::LanguagePicker));

    // The browser opens the list over the tour (raised: the tour plane was made after it).
    dropdown.raise(&mut ui);
    let items = tour.language_items(&config);
    assert!(items.len() >= 12 && items.iter().filter(|i| i.selected).count() == 1);
    let _ = dropdown.open(&mut ui, &config, button, WINDOW, items);
    settle(&mut ui, &mut tour, &mut dropdown);
    assert!(dropdown.is_open());

    // The list lies in front of the tour at the row's pixels: scroll so German shows, press it.
    let german = tour
        .language_items(&config)
        .into_iter()
        .find(|i| i.label == "Deutsch")
        .expect("German is offered in its own name");
    let row = node_with_action(&ui, german.action).expect("the row exists");
    // Rows outside the viewport are clipped: step the highlight until the row is on screen.
    while !dropdown.contains(&ui, ui.frame(row).center()) && dropdown.chosen() != Some(german.action) {
        dropdown.move_highlight(&mut ui, 1);
        ui.layout();
    }
    let at = ui.frame(row).center();
    assert!(dropdown.contains(&ui, at), "the row lies inside the list");
    let events = click(&mut ui, at, |_, _| {});
    // Like the browser, the press does not close the list; the focus may go.
    assert!(dropdown.is_open());
    assert_eq!(clicked(&events), [german.action]);
    assert_eq!(tour.click(german.action), Some(Click::Pick(Pick::Language(UiLanguage::German))));
}
