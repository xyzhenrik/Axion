//! History and Downloads as glass sheets: the same big sheet as Settings and Passwords (it flows
//! out of the sidebar row that opened it and drains back into it, over the dimmed window), with
//! a title, an optional search field, the list of [`PanelItem`] rows grouped under day headings
//! that fold, and the buttons of the rows (a download's pause, show in folder …).
//!
//! The list is **windowed**: the rows are only data (`items`); widgets exist for the lines in
//! view plus a margin (`lists.overscan_rows`), however long the list is. Scrolling moves one
//! container and adds or drops a row at the edges; the cost of a frame does not depend on the
//! length of the history. A line is a day heading or a row; folded days contribute their
//! heading only (`history.collapse_older_days`).
//!
//! Rows can be selected (checkbox on hover or in selection mode, Shift+click for a range,
//! Ctrl+click to toggle, the day heading's checkbox, Ctrl+A) and deleted together. The sheet
//! only shows what the browser hands it (`set_items`) and says what was clicked ([`Click`]);
//! the browser does the work, as it does for the sidebar lists. Each sheet owns a block of
//! action ids (`base` + [`SPAN`]), see [`crate::action_ids`]: the ids of a row are those of its
//! *slot* (line number modulo [`SLOTS`]), which is unique among the lines that exist.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::{tr, tr_fmt};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::chrome::{PANEL_BUTTONS, PanelItem, panel_row_icon};
use crate::drip::Phase;
use crate::favicons::Favicons;
use crate::morph::{Origin, SHEET_BACKDROP_DIM, SHEET_MARGIN, SHEET_RADIUS, SheetMorph};
use crate::url::display_host;

/// Lines that can exist as widgets at once (one block of action ids each). The visible lines
/// plus the margin stay far below this; see [`visible_lines`].
pub const SLOTS: usize = 64;
/// Action ids per slot: the row itself, its ×, its checkbox, and its trailing buttons.
const ROW_KINDS: u32 = 3 + PANEL_BUTTONS as u32;
const KIND_ROW: u32 = 0;
const KIND_DELETE: u32 = 1;
const KIND_CHECK: u32 = 2;
const KIND_BUTTON: u32 = 3;
const ROWS_AT: u32 = 100;
/// Ids a sheet uses from its base.
pub const SPAN: u32 = ROWS_AT + ROW_KINDS * SLOTS as u32;

const CLOSE_BACKDROP: u32 = 0;
const CLOSE_BUTTON: u32 = 1;
const CLEAR: u32 = 2;
const SELECT: u32 = 3;
const DELETE_SELECTED: u32 = 4;
const CANCEL_SELECTION: u32 = 5;

const SHEET_MAX: Vec2 = Vec2::new(760.0, 640.0);
const ROW_HEIGHT: f32 = 54.0;
const HEADER_HEIGHT: f32 = 36.0;
/// Space between two lines.
const LINE_GAP: f32 = 2.0;
const PROGRESS_ROOM: f32 = 12.0;
const PROGRESS_HEIGHT: f32 = 4.0;
const ICON: f32 = 22.0;
const CHECK: f32 = 22.0;
const HEADER_BUTTON: f32 = 34.0;
const SEARCH_HEIGHT: f32 = 36.0;
const BAR_HEIGHT: f32 = 40.0;
/// Rotation of a folded day's chevron (degrees; it points down when open).
const FOLDED_CHEVRON: f32 = -90.0;

/// Which list the sheet shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetKind {
    History,
    Downloads,
}

impl SheetKind {
    pub fn title(self) -> &'static str {
        match self {
            SheetKind::History => tr("History"),
            SheetKind::Downloads => tr("Downloads"),
        }
    }

    /// The first action id of the sheet.
    pub fn base(self) -> u32 {
        match self {
            SheetKind::History => crate::action_ids::HISTORY_SHEET,
            SheetKind::Downloads => crate::action_ids::DOWNLOADS_SHEET,
        }
    }
}

/// What a click in the sheet asks for. Rows are named by their index in the item list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Close,
    /// The header's clear button.
    Clear,
    /// The header's Select button: selection mode on or off.
    Select,
    DeleteSelected,
    CancelSelection,
    Open(usize),
    Delete(usize),
    /// The `k`th trailing button of a row.
    Button(usize, usize),
    /// A row's checkbox.
    Check(usize),
    /// A day heading: fold or open it.
    Group(usize),
    /// A day heading's checkbox.
    GroupCheck(usize),
}

/// One line of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Header(usize),
    Row(usize),
}

/// A day: its heading and the rows under it (`items[start..start + len]`).
struct Group {
    label: String,
    start: usize,
    len: usize,
}

/// The state of a heading's checkbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    None,
    Some,
    All,
}

// ---- pure logic (tested below) ----

/// The selected rows, by their index in the item list.
#[derive(Default, Debug)]
pub struct Selection {
    set: HashSet<usize>,
    /// Where a Shift+click range starts: the last row clicked.
    anchor: Option<usize>,
}

impl Selection {
    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    pub fn contains(&self, i: usize) -> bool {
        self.set.contains(&i)
    }

    pub fn clear(&mut self) {
        self.set.clear();
        self.anchor = None;
    }

    /// Ctrl+click and a plain click on a checkbox: flips one row and makes it the anchor.
    pub fn toggle(&mut self, i: usize) {
        if !self.set.remove(&i) {
            self.set.insert(i);
        }
        self.anchor = Some(i);
    }

    /// Shift+click: selects `between` (the shown rows from the anchor to the clicked one,
    /// both ends included). Without an anchor it is a plain toggle.
    pub fn extend(&mut self, i: usize, between: impl IntoIterator<Item = usize>) {
        if self.anchor.is_none() {
            self.toggle(i);
            return;
        }
        self.set.extend(between);
    }

    /// Ctrl+A: these rows, and nothing else.
    pub fn select_all(&mut self, rows: impl IntoIterator<Item = usize>) {
        self.set = rows.into_iter().collect();
    }

    /// A day's checkbox: when every one of `rows` is selected, unselects them, else selects
    /// them all.
    pub fn toggle_day(&mut self, rows: impl Iterator<Item = usize> + Clone) {
        if rows.clone().all(|i| self.set.contains(&i)) {
            for i in rows {
                self.set.remove(&i);
            }
        } else {
            self.set.extend(rows);
        }
    }

    pub fn day_state(&self, rows: Range<usize>) -> Tri {
        let selected = rows.clone().filter(|i| self.set.contains(i)).count();
        match selected {
            0 => Tri::None,
            n if n == rows.len() => Tri::All,
            _ => Tri::Some,
        }
    }

    /// The selected rows, ascending.
    pub fn sorted(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = self.set.iter().copied().collect();
        rows.sort_unstable();
        rows
    }

    /// Row numbers after the rows `removed` (ascending) left the list: the selection keeps what
    /// is still there, renumbered.
    fn after_removal(&mut self, removed: &[usize]) {
        let shift = |i: usize| i - removed.partition_point(|&r| r < i);
        self.set = self.set.iter().filter(|i| removed.binary_search(i).is_err()).map(|&i| shift(i)).collect();
        self.anchor = self.anchor.filter(|a| removed.binary_search(a).is_err()).map(shift);
    }
}

/// The lines to build for a list scrolled to `scroll` showing `height` px: `offsets[i]` is the
/// top of line `i` (one more entry than lines: the total height). The lines in view plus
/// `overscan` on both sides, at most `max` of them.
pub fn visible_lines(offsets: &[f32], scroll: f32, height: f32, overscan: usize, max: usize) -> Range<usize> {
    let lines = offsets.len().saturating_sub(1);
    if lines == 0 {
        return 0..0;
    }
    // The line containing the top edge, and the first one that starts below the bottom edge.
    let first = offsets[..lines].partition_point(|&o| o <= scroll).saturating_sub(1);
    let end = offsets[..lines].partition_point(|&o| o < scroll + height).max(first + 1);
    let start = first.saturating_sub(overscan);
    let end = (end + overscan).min(lines);
    start..end.min(start + max)
}

/// The tops of all lines (and the total height at the end).
fn line_offsets(lines: &[Line], items: &[PanelItem]) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut y = 0.0;
    for line in lines {
        offsets.push(y);
        y += match line {
            Line::Header(_) => HEADER_HEIGHT,
            Line::Row(i) => ROW_HEIGHT + if items[*i].progress.is_some() { PROGRESS_ROOM } else { 0.0 },
        } + LINE_GAP;
    }
    offsets.push(y);
    offsets
}

/// The days of `items`: a day starts at each item that carries a heading.
fn day_groups(items: &[PanelItem]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match (&item.section, groups.last_mut()) {
            (Some(label), _) => groups.push(Group { label: label.clone(), start: i, len: 1 }),
            (None, Some(group)) => group.len += 1,
            // Rows before the first heading (a list without headings) form one day without a name.
            (None, None) => groups.push(Group { label: String::new(), start: i, len: 1 }),
        }
    }
    groups
}

// ---- widgets ----

/// The widgets of one line in the window.
struct Built {
    line: usize,
    node: NodeId,
    /// The checkbox (a circle; filled with a tick when selected).
    check: Option<NodeId>,
    check_glyph: Option<NodeId>,
    /// What the checkbox covers: the row's icon.
    icon_slot: Option<NodeId>,
    /// A heading's chevron.
    chevron: Option<NodeId>,
    /// Whether the row's site icon was found when built (else it is rebuilt when one arrives).
    icon_found: bool,
}

pub struct ListSheet {
    kind: SheetKind,
    base: u32,
    root: NodeId,
    sheet: NodeId,
    morph: SheetMorph,
    origin: Option<(Origin, Origin)>,
    frame: Frame,
    clear: NodeId,
    clear_label: NodeId,
    select: NodeId,
    search_box: NodeId,
    search: NodeId,
    viewport: NodeId,
    /// The built lines, in a column whose top margin follows the scroll.
    list: NodeId,
    message: NodeId,
    bar: NodeId,
    bar_label: NodeId,
    bar_delete: NodeId,
    bar_delete_label: NodeId,
    scroll: f32,
    open: bool,
    /// The query the list was last filtered with.
    query: String,
    /// What the search field says now and since when (the query is sent after it rests).
    typed: Option<(String, Instant)>,
    debounce: Duration,
    overscan: usize,
    hover_checkboxes: bool,
    collapse_older: bool,
    /// The rows' data.
    items: Vec<PanelItem>,
    groups: Vec<Group>,
    /// Days the user opened (true) or folded (false), by heading, while the sheet stays open.
    user_open: HashMap<String, bool>,
    lines: Vec<Line>,
    offsets: Vec<f32>,
    /// The lines that exist as widgets, ascending.
    built: Vec<Built>,
    selection: Selection,
    /// Selection mode: checkboxes show on every row.
    selecting: bool,
    /// The line under the pointer.
    hover_line: Option<usize>,
    empty_text: String,
    loading: bool,
}

fn padding(h: f32, v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(h), right: length(h), top: length(v), bottom: length(v) }
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn column(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        gap: Size { width: length(0.0), height: length(gap) },
        ..Default::default()
    }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

/// A round button of pure glass (no fill behind it).
fn icon_button(ui: &mut Ui, parent: NodeId, action: ActionId, icon: Icon, size: f32) -> NodeId {
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thin)),
        Style {
            size: Size { width: length(size), height: length(size) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(0.0)
        },
    );
    ui.add(button, Widget::icon(icon, size * 0.5).with_paint(Paint::Accent), Style::default());
    button
}

/// A capsule of glass with a label; `tint` makes it a tinted (destructive) button.
fn capsule_button(
    ui: &mut Ui,
    parent: NodeId,
    action: ActionId,
    text: &str,
    tint: Option<Color>,
) -> (NodeId, NodeId) {
    let background = match tint {
        Some(color) => Background::TintedGlass(GlassLevel::Thin, Paint::Fixed(color)),
        None => Background::Glass(GlassLevel::Thin),
    };
    let button = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            size: Size { width: auto(), height: length(HEADER_BUTTON) },
            padding: padding(16.0, 0.0),
            flex_shrink: 0.0,
            ..row(0.0)
        },
    );
    let label = ui.add_label(button, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(match tint {
            Some(color) => Paint::OnColor(color),
            None => Paint::Accent,
        });
    }
    (button, label)
}

impl ListSheet {
    /// Builds the (hidden) sheet on the root of `ui`.
    pub fn build(ui: &mut Ui, kind: SheetKind) -> Self {
        let base = kind.base();
        let destructive = ui.theme().config().appearance.destructive;
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), SHEET_BACKDROP_DIM))
                .without_feedback()
                .with_action(ActionId(base + CLOSE_BACKDROP)),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        let sheet = ui.add(
            root,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_radius(SHEET_RADIUS),
            Style { position: Position::Absolute, padding: padding(22.0, 18.0), ..column(10.0) },
        );
        let header = ui.add(sheet, Widget::container(), Style { padding: padding(6.0, 4.0), ..row(8.0) });
        ui.add_label(header, kind.title(), TextStyle::Title1, Style { flex_grow: 1.0, ..shrinking() });
        // The Select button toggles selection mode; the clear button (hidden when there is
        // nothing to clear) clears the list. Both are capsules of glass with a label.
        let (select, _) = capsule_button(ui, header, ActionId(base + SELECT), tr("Select"), None);
        let (clear, clear_label) = capsule_button(ui, header, ActionId(base + CLEAR), "", None);
        icon_button(ui, header, ActionId(base + CLOSE_BUTTON), Icon::Close, HEADER_BUTTON);

        let search_box = ui.add(
            sheet,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(SEARCH_HEIGHT) },
                flex_shrink: 0.0,
                padding: padding(12.0, 0.0),
                ..row(8.0)
            },
        );
        ui.add(search_box, Widget::icon(Icon::Search, 16.0).with_paint(Paint::Secondary), Style::default());
        let search = ui.add_text_field(
            search_box,
            tr("Search"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let viewport = ui.add(
            sheet,
            Widget::container().with_clip(),
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::START,
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
            Style {
                flex_shrink: 0.0,
                size: Size { width: percent(1.0), height: auto() },
                ..column(LINE_GAP)
            },
        );
        let message = ui.add_label(
            viewport,
            "",
            TextStyle::Body,
            Style { padding: padding(12.0, 12.0), position: Position::Absolute, ..Default::default() },
        );
        if let Some(w) = ui.widget_mut(message) {
            w.set_paint(Paint::Secondary);
        }
        // The selection bar: how many rows are selected, Cancel and Delete (red glass).
        let bar = ui.add(
            sheet,
            Widget::container(),
            Style {
                size: Size { width: auto(), height: length(BAR_HEIGHT) },
                flex_shrink: 0.0,
                padding: padding(6.0, 0.0),
                ..row(8.0)
            },
        );
        let bar_label = ui.add_label(bar, "", TextStyle::Callout, Style { flex_grow: 1.0, ..shrinking() });
        if let Some(w) = ui.widget_mut(bar_label) {
            w.set_paint(Paint::Secondary);
        }
        capsule_button(ui, bar, ActionId(base + CANCEL_SELECTION), tr("Cancel"), None);
        let (bar_delete, bar_delete_label) =
            capsule_button(ui, bar, ActionId(base + DELETE_SELECTED), tr("Delete"), Some(destructive));
        ui.set_visible(bar, false);
        ui.set_visible(root, false);
        let morph = SheetMorph::build(sheet, root, SHEET_RADIUS, SHEET_BACKDROP_DIM);
        let defaults = axion_config::Config::default();
        let this = Self {
            kind,
            base,
            root,
            sheet,
            morph,
            origin: None,
            frame: Frame::default(),
            clear,
            clear_label,
            select,
            search_box,
            search,
            viewport,
            list,
            message,
            bar,
            bar_label,
            bar_delete,
            bar_delete_label,
            scroll: 0.0,
            open: false,
            query: String::new(),
            typed: None,
            debounce: Duration::from_millis(u64::from(defaults.history.search_debounce_ms)),
            overscan: defaults.lists.overscan_rows as usize,
            hover_checkboxes: defaults.lists.hover_checkboxes,
            collapse_older: defaults.history.collapse_older_days,
            items: Vec::new(),
            groups: Vec::new(),
            user_open: HashMap::new(),
            lines: Vec::new(),
            offsets: vec![0.0],
            built: Vec::new(),
            selection: Selection::default(),
            selecting: false,
            hover_line: None,
            empty_text: String::new(),
            loading: false,
        };
        ui.set_visible(this.search_box, kind == SheetKind::History);
        this.set_message(ui);
        this
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether `action` is one of this sheet's.
    pub fn owns_action(&self, action: ActionId) -> bool {
        (self.base..self.base + SPAN).contains(&action.0)
    }

    pub fn search_field(&self) -> NodeId {
        self.search
    }

    fn id(&self, line: usize, kind: u32) -> ActionId {
        ActionId(self.base + ROWS_AT + (line % SLOTS) as u32 * ROW_KINDS + kind)
    }

    /// The decoded meaning of a click on `action`; the backdrop only closes from outside the sheet.
    pub fn click(&self, ui: &Ui, action: ActionId, pointer: Option<Vec2>) -> Option<Click> {
        let n = action.0.checked_sub(self.base)?;
        match n {
            CLOSE_BACKDROP => (!pointer
                .is_some_and(|p| ui.is_visible(self.sheet) && ui.frame(self.sheet).contains(p)))
            .then_some(Click::Close),
            CLOSE_BUTTON => Some(Click::Close),
            CLEAR => Some(Click::Clear),
            SELECT => Some(Click::Select),
            DELETE_SELECTED => Some(Click::DeleteSelected),
            CANCEL_SELECTION => Some(Click::CancelSelection),
            n if (ROWS_AT..SPAN).contains(&n) => {
                let n = n - ROWS_AT;
                let (slot, k) = ((n / ROW_KINDS) as usize, n % ROW_KINDS);
                let line = self.built.iter().find(|b| b.line % SLOTS == slot)?.line;
                Some(match (self.lines.get(line)?, k) {
                    (Line::Header(g), KIND_ROW) => Click::Group(*g),
                    (Line::Header(g), KIND_CHECK) => Click::GroupCheck(*g),
                    (Line::Header(_), _) => return None,
                    (Line::Row(i), KIND_ROW) => Click::Open(*i),
                    (Line::Row(i), KIND_DELETE) => Click::Delete(*i),
                    (Line::Row(i), KIND_CHECK) => Click::Check(*i),
                    (Line::Row(i), k) => Click::Button(*i, (k - KIND_BUTTON) as usize),
                })
            }
            _ => None,
        }
    }

    /// The row under `p` (logical px), for a right click.
    pub fn row_at(&self, ui: &Ui, p: Vec2) -> Option<usize> {
        if !self.open || !ui.is_visible(self.sheet) || !ui.frame(self.viewport).contains(p) {
            return None;
        }
        let action = ui.hit_action(p).and_then(|n| ui.widget(n)).and_then(|w| w.action)?;
        match self.click(ui, action, None)? {
            Click::Open(i) | Click::Delete(i) | Click::Button(i, _) | Click::Check(i) => Some(i),
            _ => None,
        }
    }

    /// The address of row `i` (a history entry's page, a download's source).
    pub fn item_url(&self, i: usize) -> Option<&str> {
        self.items.get(i).map(|item| item.url.as_str())
    }

    /// How many rows the list has.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// How many widgets the list has built (a measure, and a test's proof of windowing).
    pub fn built_count(&self) -> usize {
        self.built.len()
    }

    /// Opens out of `origin` (or `fallback`); `window` is the window's logical size.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, window: Vec2, origin: Origin, fallback: Origin) {
        self.origin = Some((origin, fallback));
        self.open = true;
        self.debounce = Duration::from_millis(u64::from(config.history.search_debounce_ms));
        self.overscan = config.lists.overscan_rows as usize;
        self.hover_checkboxes = config.lists.hover_checkboxes;
        self.collapse_older = config.history.collapse_older_days;
        self.user_open.clear();
        self.selection.clear();
        self.selecting = false;
        self.typed = None;
        self.loading = true;
        self.scroll = 0.0;
        self.paint_destructive(ui, config.appearance.destructive);
        self.show_bar(ui);
        ui.set_visible(self.root, true);
        ui.set_visible(self.sheet, true);
        self.resize(ui, window);
        self.morph.open(ui, config, origin, fallback, self.frame, true);
    }

    /// Closes: drains back into where it came from (or `fallback`).
    pub fn close(&mut self, ui: &mut Ui, config: &Config, fallback: Origin) {
        if !self.open {
            return;
        }
        self.open = false;
        ui.set_field_text(self.search, "");
        self.query.clear();
        self.typed = None;
        let draining = ui.is_visible(self.root) && self.morph.is_shown();
        self.morph.close(ui, config, fallback);
        if !draining {
            ui.set_visible(self.root, false);
            self.release(ui);
        }
    }

    /// The sheet is gone: its rows' memory goes with it.
    fn release(&mut self, ui: &mut Ui) {
        self.drop_built(ui);
        self.items = Vec::new();
        self.groups = Vec::new();
        self.lines = Vec::new();
        self.offsets = vec![0.0];
        self.selection.clear();
        self.selecting = false;
    }

    /// Lays the sheet out for `window`.
    pub fn resize(&mut self, ui: &mut Ui, window: Vec2) {
        let size = (window - Vec2::splat(2.0 * SHEET_MARGIN)).min(SHEET_MAX).max(Vec2::new(420.0, 320.0));
        let origin = ((window - size) / 2.0).max(Vec2::ZERO);
        let mut style = ui.style(self.sheet);
        style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
        style.size = Size { width: length(size.x), height: length(size.y) };
        ui.set_style(self.sheet, style);
        self.frame = Frame { origin, size };
        self.morph.retarget(self.frame);
    }

    /// Advances the flow, the pointer's hover and the window of built lines; returns the search
    /// text when it changed and rested for the debounce time.
    pub fn animate(
        &mut self,
        ui: &mut Ui,
        dt: f32,
        pointer: Option<Vec2>,
        favicons: &Favicons,
    ) -> Option<String> {
        let phase = self.morph.animate(ui, dt);
        if !self.open && phase == Phase::Closed && ui.is_visible(self.root) {
            ui.set_visible(self.root, false);
            self.release(ui);
        }
        if !self.open || !ui.is_visible(self.sheet) {
            return None;
        }
        self.track_hover(ui, pointer);
        self.sync_window(ui, favicons);
        if self.kind != SheetKind::History {
            return None;
        }
        let text = ui.field_text(self.search).unwrap_or_default().trim().to_owned();
        if text == self.query {
            self.typed = None;
            return None;
        }
        let since = match &self.typed {
            Some((t, since)) if *t == text => *since,
            _ => Instant::now(),
        };
        self.typed = Some((text.clone(), since));
        if since.elapsed() >= self.debounce {
            self.typed = None;
            self.query = text.clone();
            return Some(text);
        }
        None
    }

    /// Whether a typed query waits for its debounce.
    #[cfg(test)]
    fn search_pending(&self) -> bool {
        self.typed.is_some()
    }

    /// The search text the list was filtered with.
    pub fn query(&self) -> &str {
        &self.query
    }

    // ---- scrolling and the window of built lines ----

    fn viewport_height(&self, ui: &Ui) -> f32 {
        let h = ui.frame(self.viewport).size.y;
        if h > 1.0 { h } else { SHEET_MAX.y - 180.0 }
    }

    fn max_scroll(&self, ui: &Ui) -> f32 {
        let total = self.offsets.last().copied().unwrap_or(0.0);
        (total - LINE_GAP - self.viewport_height(ui)).max(0.0)
    }

    /// Scrolls the list by `dy` logical px (wheel: positive scrolls up).
    pub fn scroll_by(&mut self, ui: &mut Ui, favicons: &Favicons, dy: f32) {
        self.scroll = (self.scroll - dy).clamp(0.0, self.max_scroll(ui));
        self.sync_window(ui, favicons);
    }

    /// Scrolls to the absolute position `scroll` (test aids and tests).
    pub fn scroll_to(&mut self, ui: &mut Ui, favicons: &Favicons, scroll: f32) {
        self.scroll = scroll.clamp(0.0, self.max_scroll(ui));
        self.sync_window(ui, favicons);
    }

    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// The first line that exists as a widget (0 without any).
    fn window_top(&self) -> f32 {
        self.built.first().map_or(0.0, |b| self.offsets[b.line])
    }

    fn apply_scroll(&self, ui: &mut Ui) {
        let mut style = ui.style(self.list);
        style.margin = Rect {
            left: length(0.0),
            right: length(0.0),
            top: length(self.window_top() - self.scroll),
            bottom: length(0.0),
        };
        ui.set_style(self.list, style);
    }

    /// Adds the lines that came into view and drops those that left it.
    fn sync_window(&mut self, ui: &mut Ui, favicons: &Favicons) {
        let want =
            visible_lines(&self.offsets, self.scroll, self.viewport_height(ui), self.overscan, SLOTS - 1);
        let have = match (self.built.first(), self.built.last()) {
            (Some(a), Some(b)) => a.line..b.line + 1,
            _ => 0..0,
        };
        if want == have || (want.is_empty() && self.built.is_empty()) {
            self.apply_scroll(ui);
            return;
        }
        let mut kept = Vec::with_capacity(want.len());
        for built in std::mem::take(&mut self.built) {
            if want.contains(&built.line) {
                kept.push(built);
            } else {
                ui.remove(built.node);
            }
        }
        let mut have_lines: HashSet<usize> = kept.iter().map(|b| b.line).collect();
        for line in want {
            if have_lines.insert(line) {
                kept.push(self.build_line(ui, line, favicons));
            }
        }
        kept.sort_by_key(|b| b.line);
        let ids: Vec<NodeId> = kept.iter().map(|b| b.node).collect();
        ui.set_children(self.list, &ids);
        self.built = kept;
        self.style_all(ui);
        self.apply_scroll(ui);
    }

    fn drop_built(&mut self, ui: &mut Ui) {
        for built in self.built.drain(..) {
            ui.remove(built.node);
        }
        self.hover_line = None;
    }

    /// Builds every line of the window afresh (the lines changed).
    fn rebuild_window(&mut self, ui: &mut Ui, favicons: &Favicons) {
        self.drop_built(ui);
        self.sync_window(ui, favicons);
    }

    // ---- items, days and lines ----

    /// Shows `items`; `clear` is the clear button's label (none: no button). `keep_scroll` after
    /// a change in the list (a row removed). The selection stays on the rows that are still there.
    pub fn set_items(
        &mut self,
        ui: &mut Ui,
        items: Vec<PanelItem>,
        clear: Option<&str>,
        empty_text: &str,
        favicons: &Favicons,
        keep_scroll: bool,
    ) {
        ui.set_visible(self.clear, clear.is_some());
        ui.set_text(self.clear_label, clear.unwrap_or_default());
        self.empty_text = empty_text.to_owned();
        self.loading = false;
        self.carry_selection(&items);
        self.items = items;
        self.groups = day_groups(&self.items);
        self.relayout();
        if !keep_scroll {
            self.scroll = 0.0;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(ui));
        self.rebuild_window(ui, favicons);
        self.set_message(ui);
        self.show_bar(ui);
    }

    /// History rows are told apart by address, downloads by position.
    fn carry_selection(&mut self, new_items: &[PanelItem]) {
        if self.selection.is_empty() {
            return;
        }
        if self.kind == SheetKind::History {
            let urls: HashSet<&str> = self
                .selection
                .set
                .iter()
                .filter_map(|&i| self.items.get(i))
                .map(|i| i.url.as_str())
                .collect();
            let kept: Vec<usize> = new_items
                .iter()
                .enumerate()
                .filter(|(_, item)| urls.contains(item.url.as_str()))
                .map(|(i, _)| i)
                .collect();
            self.selection.anchor = None;
            self.selection.set = kept.into_iter().collect();
        } else {
            let len = new_items.len();
            self.selection.set.retain(|&i| i < len && new_items[i].deletable);
        }
    }

    /// Whether the day `g` shows its rows.
    fn group_open(&self, g: usize) -> bool {
        let group = &self.groups[g];
        if let Some(&open) = self.user_open.get(&group.label) {
            return open;
        }
        let folds = self.kind == SheetKind::History && self.collapse_older && self.query.is_empty();
        !folds || group.label == tr("Today")
    }

    /// Lines and their geometry from the days and which of them are open.
    fn relayout(&mut self) {
        let mut lines = Vec::with_capacity(self.items.len() + self.groups.len());
        for (g, group) in self.groups.iter().enumerate() {
            if !group.label.is_empty() {
                lines.push(Line::Header(g));
            }
            if group.label.is_empty() || self.group_open(g) {
                lines.extend((group.start..group.start + group.len).map(Line::Row));
            }
        }
        self.offsets = line_offsets(&lines, &self.items);
        self.lines = lines;
    }

    /// Folds or opens the day `g`.
    pub fn toggle_group(&mut self, ui: &mut Ui, favicons: &Favicons, g: usize) {
        let Some(group) = self.groups.get(g) else { return };
        let open = !self.group_open(g);
        self.user_open.insert(group.label.clone(), open);
        self.relayout();
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(ui));
        self.rebuild_window(ui, favicons);
    }

    fn set_message(&self, ui: &mut Ui) {
        let text = if !self.items.is_empty() {
            ""
        } else if self.loading {
            tr("Loading…")
        } else {
            self.empty_text.as_str()
        };
        ui.set_text(self.message, text);
        ui.set_visible(self.message, !text.is_empty());
    }

    /// Relabels the clear button (its confirming second click).
    pub fn set_clear_label(&mut self, ui: &mut Ui, label: &str) {
        ui.set_text(self.clear_label, label);
    }

    /// Rebuilds the rows whose site icon was missing and has arrived since. Call while open.
    pub fn sync_icons(&mut self, ui: &mut Ui, favicons: &Favicons) {
        let arrived = self.built.iter().any(|b| {
            !b.icon_found
                && match self.lines.get(b.line) {
                    Some(Line::Row(i)) => {
                        self.items[*i].icon_wants_site() && favicons.get(&self.items[*i].url).is_some()
                    }
                    _ => false,
                }
        });
        if arrived {
            self.rebuild_window(ui, favicons);
        }
    }

    // ---- building a line ----

    fn build_line(&self, ui: &mut Ui, line: usize, favicons: &Favicons) -> Built {
        match self.lines[line] {
            Line::Header(g) => self.build_header(ui, line, g),
            Line::Row(i) => self.build_row(ui, line, i, favicons),
        }
    }

    /// A checkbox circle at the end of `parent`'s children (see [`Self::style_line`]).
    fn add_check(&self, ui: &mut Ui, parent: NodeId, line: usize, absolute: bool) -> (NodeId, NodeId) {
        let mut style = Style {
            size: Size { width: length(CHECK), height: length(CHECK) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(0.0)
        };
        if absolute {
            style.position = Position::Absolute;
            style.inset = Rect { left: length(0.0), right: auto(), top: length(0.0), bottom: auto() };
        }
        let check = ui.add(
            parent,
            Widget::container()
                .with_action(self.id(line, KIND_CHECK))
                .with_radius(CAPSULE)
                .without_feedback(),
            style,
        );
        let glyph =
            ui.add(check, Widget::icon(Icon::Check, 14.0).with_paint(Paint::OnAccent), Style::default());
        (check, glyph)
    }

    fn build_header(&self, ui: &mut Ui, line: usize, g: usize) -> Built {
        let node = ui.add(
            self.list,
            Widget::container().with_action(self.id(line, KIND_ROW)).with_radius(10.0),
            Style {
                size: Size { width: auto(), height: length(HEADER_HEIGHT) },
                flex_shrink: 0.0,
                padding: padding(10.0, 0.0),
                ..row(6.0)
            },
        );
        let chevron = ui.add(
            node,
            Widget::icon(Icon::ChevronDown, 14.0).with_paint(Paint::Secondary),
            Style::default(),
        );
        let label = ui.add_label(
            node,
            &self.groups[g].label,
            TextStyle::BodyEmphasis,
            Style { flex_grow: 1.0, ..shrinking() },
        );
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
        let (check, glyph) = self.add_check(ui, node, line, false);
        Built {
            line,
            node,
            check: Some(check),
            check_glyph: Some(glyph),
            icon_slot: None,
            chevron: Some(chevron),
            icon_found: true,
        }
    }

    fn build_row(&self, ui: &mut Ui, line: usize, i: usize, favicons: &Favicons) -> Built {
        let item = &self.items[i];
        let height = ROW_HEIGHT + if item.progress.is_some() { PROGRESS_ROOM } else { 0.0 };
        let node = ui.add(
            self.list,
            Widget::container().with_action(self.id(line, KIND_ROW)).with_radius(14.0),
            Style {
                size: Size { width: auto(), height: length(height) },
                flex_shrink: 0.0,
                padding: padding(10.0, 0.0),
                ..row(12.0)
            },
        );
        // The icon sits in a slot the checkbox covers while it shows.
        let slot = ui.add(
            node,
            Widget::container(),
            Style {
                size: Size { width: length(CHECK), height: length(CHECK) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        let icon_found = panel_row_icon(ui, slot, item, ICON - 2.0, favicons);
        let check = item.deletable.then(|| self.add_check(ui, slot, line, true));
        let text = ui.add(
            node,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..column(0.0)
            },
        );
        let title = if item.title.trim().is_empty() { display_host(&item.url) } else { item.title.clone() };
        ui.add_label(text, &title, TextStyle::Body, shrinking());
        if let Some(progress) = item.progress {
            let track = ui.add(
                text,
                Widget::container()
                    .with_background(Background::Fill(Paint::Label, 0.12))
                    .with_radius(CAPSULE),
                Style {
                    size: Size { width: percent(1.0), height: length(PROGRESS_HEIGHT) },
                    margin: Rect {
                        left: length(0.0),
                        right: length(0.0),
                        top: length(3.0),
                        bottom: length(3.0),
                    },
                    flex_shrink: 0.0,
                    ..row(0.0)
                },
            );
            ui.add(
                track,
                Widget::container()
                    .with_background(Background::Fill(Paint::Accent, 1.0))
                    .with_radius(CAPSULE),
                Style {
                    size: Size { width: percent(progress.clamp(0.0, 1.0)), height: percent(1.0) },
                    ..Default::default()
                },
            );
        }
        let detail = ui.add_label(text, &item.detail, TextStyle::Footnote, shrinking());
        if let Some(w) = ui.widget_mut(detail) {
            w.set_paint(Paint::Secondary);
        }
        for (k, &icon) in item.buttons.iter().take(PANEL_BUTTONS).enumerate() {
            icon_button(ui, node, self.id(line, KIND_BUTTON + k as u32), icon, 30.0);
        }
        if item.deletable {
            icon_button(ui, node, self.id(line, KIND_DELETE), Icon::Close, 30.0);
        }
        Built {
            line,
            node,
            check: check.map(|c| c.0),
            check_glyph: check.map(|c| c.1),
            icon_slot: Some(slot),
            chevron: None,
            icon_found,
        }
    }

    // ---- hover, selection state on screen ----

    fn track_hover(&mut self, ui: &mut Ui, pointer: Option<Vec2>) {
        let hover = pointer
            .filter(|p| ui.frame(self.viewport).contains(*p))
            .and_then(|p| ui.hit_action(p))
            .and_then(|n| ui.widget(n))
            .and_then(|w| w.action)
            .and_then(|a| a.0.checked_sub(self.base + ROWS_AT))
            .and_then(|n| {
                let slot = (n / ROW_KINDS) as usize;
                self.built.iter().find(|b| b.line % SLOTS == slot).map(|b| b.line)
            });
        if hover != self.hover_line {
            let before = std::mem::replace(&mut self.hover_line, hover);
            for line in before.into_iter().chain(hover) {
                self.style_line_at(ui, line);
            }
        }
    }

    fn style_all(&mut self, ui: &mut Ui) {
        for k in 0..self.built.len() {
            self.style_line(ui, k);
        }
    }

    fn style_line_at(&mut self, ui: &mut Ui, line: usize) {
        if let Some(k) = self.built.iter().position(|b| b.line == line) {
            self.style_line(ui, k);
        }
    }

    /// The selection and hover look of the built line `k`: the row's tint, the checkbox (shown
    /// on hover, in selection mode, or when selected) and its fill.
    fn style_line(&self, ui: &mut Ui, k: usize) {
        let built = &self.built[k];
        let (state, selectable) = match self.lines.get(built.line) {
            Some(Line::Row(i)) => {
                (if self.selection.contains(*i) { Tri::All } else { Tri::None }, self.items[*i].deletable)
            }
            Some(Line::Header(g)) => {
                let group = &self.groups[*g];
                let selectable = self.items[group.start..group.start + group.len].iter().any(|i| i.deletable);
                (self.selection.day_state(group.start..group.start + group.len), selectable)
            }
            None => return,
        };
        let hovered = self.hover_line == Some(built.line);
        let shown = selectable
            && (self.selecting
                || state != Tri::None
                || hovered && (self.hover_checkboxes || matches!(self.lines[built.line], Line::Header(_))));
        if let Some(w) = ui.widget_mut(built.node) {
            w.background = match (self.lines[built.line], state) {
                (Line::Row(_), Tri::All) => Background::Fill(Paint::Accent, 0.16),
                _ => Background::None,
            };
        }
        if let (Some(check), Some(glyph)) = (built.check, built.check_glyph) {
            if let Some(w) = ui.widget_mut(check) {
                w.background = match state {
                    Tri::None => Background::Fill(Paint::Label, 0.18),
                    _ => Background::Fill(Paint::Accent, 1.0),
                };
            }
            ui.set_icon(glyph, if state == Tri::Some { Icon::Minus } else { Icon::Check });
            ui.set_opacity(glyph, if state == Tri::None { 0.0 } else { 1.0 });
            ui.set_opacity(check, if shown { 1.0 } else { 0.0 });
            ui.set_passthrough(check, !shown);
        }
        if let Some(slot) = built.icon_slot {
            // The checkbox covers the icon while it shows.
            if let Some(&icon) = ui.children(slot).first() {
                ui.set_opacity(icon, if shown { 0.0 } else { 1.0 });
            }
        }
        if let Some(chevron) = built.chevron
            && let Some(Line::Header(g)) = self.lines.get(built.line)
        {
            let open = self.group_open(*g);
            if let Some(w) = ui.widget_mut(chevron) {
                w.rotation = if open { 0.0 } else { FOLDED_CHEVRON };
            }
        }
    }

    // ---- selection ----

    pub fn is_selecting(&self) -> bool {
        self.selecting
    }

    pub fn selected_count(&self) -> usize {
        self.selection.len()
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.selection.contains(i)
    }

    /// The selected rows, ascending.
    pub fn selected_items(&self) -> Vec<usize> {
        self.selection.sorted()
    }

    /// The selection bar and the Select button follow the state.
    fn show_bar(&self, ui: &mut Ui) {
        ui.set_visible(self.bar, self.selecting);
        if let Some(w) = ui.widget_mut(self.select) {
            w.selected = self.selecting;
        }
        let n = self.selection.len();
        ui.set_text(
            self.bar_label,
            &if n == 0 { tr("Select items").to_owned() } else { tr_fmt!("{count} selected", count = n) },
        );
        if let Some(w) = ui.widget_mut(self.bar_delete) {
            w.enabled = n > 0;
        }
        ui.set_opacity(self.bar_delete, if n > 0 { 1.0 } else { 0.5 });
        let _ = self.bar_delete_label;
    }

    fn selection_changed(&mut self, ui: &mut Ui) {
        self.show_bar(ui);
        self.style_all(ui);
    }

    fn paint_destructive(&self, ui: &mut Ui, color: Color) {
        if let Some(w) = ui.widget_mut(self.bar_delete) {
            w.background = Background::TintedGlass(GlassLevel::Thin, Paint::Fixed(color));
        }
        if let Some(w) = ui.widget_mut(self.bar_delete_label) {
            w.set_paint(Paint::OnColor(color));
        }
    }

    /// Selection mode on (checkboxes everywhere) or off (and nothing selected).
    pub fn toggle_selecting(&mut self, ui: &mut Ui) {
        if self.selecting {
            self.end_selection(ui);
        } else {
            self.selecting = true;
            self.selection_changed(ui);
            // Typing in the search field must not swallow Delete and Ctrl+A.
            let _ = ui.blur();
            self.relayout_for_bar(ui);
        }
    }

    /// Leaves selection mode and unselects everything.
    pub fn end_selection(&mut self, ui: &mut Ui) {
        self.selecting = false;
        self.selection.clear();
        self.selection_changed(ui);
        self.relayout_for_bar(ui);
    }

    /// The bar changes the viewport's height; the window follows in the next frame's sync.
    fn relayout_for_bar(&mut self, ui: &mut Ui) {
        ui.layout();
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(ui));
    }

    /// The rows from the anchor to `i` among those shown (rows of folded days are not).
    fn shown_between(&self, i: usize) -> Vec<usize> {
        let Some(anchor) = self.selection.anchor else { return vec![i] };
        let (lo, hi) = (anchor.min(i), anchor.max(i));
        let mut rows = Vec::new();
        for (g, group) in self.groups.iter().enumerate() {
            let (start, end) = (group.start.max(lo), (group.start + group.len).min(hi + 1));
            if start < end && (group.label.is_empty() || self.group_open(g)) {
                rows.extend((start..end).filter(|&r| self.items[r].deletable));
            }
        }
        rows
    }

    /// A click on row `i` in selection mode, or with Ctrl (toggle) or Shift (range) held.
    pub fn click_row(&mut self, ui: &mut Ui, i: usize, shift: bool) {
        if !self.items.get(i).is_some_and(|item| item.deletable) {
            return;
        }
        if shift {
            let between = self.shown_between(i);
            self.selection.extend(i, between);
        } else {
            self.selection.toggle(i);
        }
        self.selecting = true;
        self.selection_changed(ui);
        let _ = ui.blur();
    }

    /// A click on the checkbox of the day `g`.
    pub fn click_group_check(&mut self, ui: &mut Ui, g: usize) {
        let Some(group) = self.groups.get(g) else { return };
        // Only the rows that can be removed take part.
        let deletable: Vec<usize> =
            (group.start..group.start + group.len).filter(|&i| self.items[i].deletable).collect();
        if deletable.is_empty() {
            return;
        }
        self.selection.toggle_day(deletable.into_iter());
        self.selecting = true;
        self.selection_changed(ui);
        let _ = ui.blur();
    }

    /// Ctrl+A: every row of the list (the filtered one while searching).
    pub fn select_all(&mut self, ui: &mut Ui) {
        self.selection.select_all((0..self.items.len()).filter(|&i| self.items[i].deletable));
        self.selecting = true;
        self.selection_changed(ui);
        let _ = ui.blur();
    }

    /// The rows left the list (deleted): the list closes the gaps without a reload, what was
    /// selected besides stays selected (the mode ends with the last selected row).
    pub fn remove_items(&mut self, ui: &mut Ui, favicons: &Favicons, removed: &[usize]) {
        let mut removed: Vec<usize> = removed.iter().copied().filter(|&i| i < self.items.len()).collect();
        removed.sort_unstable();
        removed.dedup();
        if removed.is_empty() {
            return;
        }
        // A removed first row of a day hands its heading to the next row of that day.
        for &i in &removed {
            if let Some(label) = self.items[i].section.take()
                && let Some(next) = self.items.get_mut(i + 1)
                && next.section.is_none()
                && !removed.contains(&(i + 1))
            {
                next.section = Some(label);
            }
        }
        // Drop the rows (a single pass over the list).
        let mut index = 0;
        self.items.retain(|_| {
            let keep = removed.binary_search(&index).is_err();
            index += 1;
            keep
        });
        self.selection.after_removal(&removed);
        self.groups = day_groups(&self.items);
        self.selecting = self.selecting && !self.selection.is_empty();
        self.relayout();
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(ui));
        self.rebuild_window(ui, favicons);
        self.set_message(ui);
        self.show_bar(ui);
        self.relayout_for_bar(ui);
    }
}

impl PanelItem {
    /// Whether the row shows its site's icon when there is one.
    pub fn icon_wants_site(&self) -> bool {
        self.icon.is_none() || self.site_icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axion_ui::{UiEvent, UiInput};
    use std::path::Path;

    fn built(kind: SheetKind) -> (Ui, ListSheet) {
        let config = Config::default();
        let mut ui = Ui::new(axion_ui::Theme::new(&config, false), 1.0);
        let sheet = ListSheet::build(&mut ui, kind);
        (ui, sheet)
    }

    fn items() -> Vec<PanelItem> {
        vec![
            PanelItem {
                title: "a.zip".into(),
                url: "https://example.org/a.zip".into(),
                detail: "1 MB".into(),
                section: Some("Today".into()),
                icon: Some(Icon::File),
                site_icon: true,
                buttons: vec![Icon::Folder],
                deletable: true,
                ..PanelItem::default()
            },
            PanelItem { title: "b".into(), url: "https://example.org/b".into(), ..PanelItem::default() },
        ]
    }

    /// `days` days of `per_day` history rows each, newest first, headings on each day's first row.
    fn history(days: usize, per_day: usize) -> Vec<PanelItem> {
        let mut out = Vec::new();
        for d in 0..days {
            for r in 0..per_day {
                out.push(PanelItem {
                    title: format!("Page {d}/{r}"),
                    url: format!("https://site{}.example/{d}/{r}", r % 7),
                    detail: format!("site{}.example", r % 7),
                    section: (r == 0).then(|| if d == 0 { "Today".to_owned() } else { format!("Day {d}") }),
                    deletable: true,
                    ..PanelItem::default()
                });
            }
        }
        out
    }

    fn find_action(ui: &Ui, node: NodeId, action: ActionId) -> Option<NodeId> {
        if ui.widget(node).and_then(|w| w.action) == Some(action) {
            return Some(node);
        }
        ui.children(node).into_iter().find_map(|c| find_action(ui, c, action))
    }

    fn favicons() -> Favicons {
        let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
        Favicons::new(device, queue, None)
    }

    fn opened(kind: SheetKind, items: Vec<PanelItem>, config: &Config) -> (Ui, ListSheet, Favicons) {
        let (mut ui, mut sheet) = built(kind);
        let favicons = favicons();
        let origin = Origin { plane: ui.root(), point: Vec2::new(100.0, 100.0), control: Frame::default() };
        sheet.open(&mut ui, config, Vec2::new(1200.0, 800.0), origin, origin);
        sheet.set_items(&mut ui, items, Some("Clear"), "Nothing here", &favicons, false);
        settle(&mut ui, &mut sheet, &favicons);
        (ui, sheet, favicons)
    }

    fn settle(ui: &mut Ui, sheet: &mut ListSheet, favicons: &Favicons) {
        for _ in 0..240 {
            sheet.animate(ui, 1.0 / 60.0, None, favicons);
            ui.layout();
        }
    }

    fn click_node(ui: &mut Ui, node: NodeId) -> Vec<ActionId> {
        let centre = ui.frame(node).center();
        let _ = ui.handle(UiInput::PointerMove(centre));
        let _ = ui.handle(UiInput::PointerDown);
        let (_, up) = ui.handle(UiInput::PointerUp);
        up.iter()
            .filter_map(|e| match e {
                UiEvent::Clicked { action, .. } => Some(*action),
                _ => None,
            })
            .collect()
    }

    // ---- logic ----

    /// What one wheel step costs the sheet alone (no chrome around it); `--ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn scroll_step_cost() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(90, 555), &config);
        let started = Instant::now();
        for _ in 0..200 {
            sheet.scroll_by(&mut ui, &favicons, -37.0);
            ui.layout();
        }
        let per_step = started.elapsed().as_secs_f32() * 1000.0 / 200.0;
        let widgets = ui.children(sheet.list).len();
        eprintln!(
            "{per_step:.2} ms per scroll step, {} lines built, {widgets} children",
            sheet.built_count()
        );
    }

    #[test]
    fn click_ids_round_trip() {
        let (mut ui, mut sheet) = built(SheetKind::Downloads);
        let favicons = favicons();
        // Line 0 is the "Today" heading, line 1 the first row, line 2 the second.
        sheet.set_items(&mut ui, items(), None, "", &favicons, false);
        let at = |n: u32| ActionId(sheet.base + n);
        assert_eq!(sheet.click(&ui, at(CLOSE_BUTTON), None), Some(Click::Close));
        assert_eq!(sheet.click(&ui, at(CLEAR), None), Some(Click::Clear));
        assert_eq!(sheet.click(&ui, at(SELECT), None), Some(Click::Select));
        assert_eq!(sheet.click(&ui, at(DELETE_SELECTED), None), Some(Click::DeleteSelected));
        assert_eq!(sheet.click(&ui, at(CANCEL_SELECTION), None), Some(Click::CancelSelection));
        let slot = |line: u32, kind: u32| at(ROWS_AT + line * ROW_KINDS + kind);
        assert_eq!(sheet.click(&ui, slot(0, KIND_ROW), None), Some(Click::Group(0)));
        assert_eq!(sheet.click(&ui, slot(0, KIND_CHECK), None), Some(Click::GroupCheck(0)));
        assert_eq!(sheet.click(&ui, slot(1, KIND_ROW), None), Some(Click::Open(0)));
        assert_eq!(sheet.click(&ui, slot(1, KIND_DELETE), None), Some(Click::Delete(0)));
        assert_eq!(sheet.click(&ui, slot(1, KIND_CHECK), None), Some(Click::Check(0)));
        assert_eq!(sheet.click(&ui, slot(1, KIND_BUTTON), None), Some(Click::Button(0, 0)));
        assert_eq!(sheet.click(&ui, slot(2, KIND_ROW), None), Some(Click::Open(1)));
        // A slot without a line says nothing.
        assert_eq!(sheet.click(&ui, slot(9, KIND_ROW), None), None);
        assert_eq!(sheet.click(&ui, ActionId(sheet.base + SPAN), None), None);
        assert!(!sheet.owns_action(ActionId(sheet.base + SPAN)));
    }

    #[test]
    fn selection_toggles_and_extends_over_a_range() {
        let mut s = Selection::default();
        s.toggle(3);
        s.toggle(5);
        s.toggle(3);
        assert_eq!(s.sorted(), [5]);
        // Shift+click from the last click (5) to 8, then back up to 2: both ends included.
        s.extend(8, 5..=8);
        assert_eq!(s.sorted(), [5, 6, 7, 8]);
        s.extend(2, 2..=5);
        assert_eq!(s.sorted(), [2, 3, 4, 5, 6, 7, 8]);
        // Without an anchor Shift+click is a plain toggle.
        let mut s = Selection::default();
        s.extend(4, 0..=4);
        assert_eq!(s.sorted(), [4]);
    }

    #[test]
    fn a_day_checkbox_selects_and_unselects_the_whole_day() {
        let mut s = Selection::default();
        s.toggle(11);
        assert_eq!(s.day_state(10..14), Tri::Some);
        s.toggle_day(10..14);
        assert_eq!(s.day_state(10..14), Tri::All);
        assert_eq!(s.len(), 4);
        s.toggle_day(10..14);
        assert_eq!(s.day_state(10..14), Tri::None);
        assert!(s.is_empty());
    }

    #[test]
    fn the_selection_is_renumbered_when_rows_leave() {
        let mut s = Selection::default();
        s.select_all([1, 4, 6, 9]);
        s.anchor = Some(6);
        s.after_removal(&[2, 4, 5]);
        assert_eq!(s.sorted(), [1, 3, 6]);
        assert_eq!(s.anchor, Some(3));
    }

    #[test]
    fn the_window_covers_the_view_plus_the_margin() {
        // 1000 lines of 56 px.
        let offsets: Vec<f32> = (0..=1000).map(|i| i as f32 * 56.0).collect();
        // At the top: 520 px show ten lines (the tenth partly), plus six more below.
        assert_eq!(visible_lines(&offsets, 0.0, 520.0, 6, 63), 0..16);
        // Scrolled by exactly three lines and a bit.
        assert_eq!(visible_lines(&offsets, 3.0 * 56.0 + 10.0, 520.0, 6, 63), 0..19);
        // Deep in the list the window slides along; it never exceeds the slots.
        let r = visible_lines(&offsets, 500.0 * 56.0, 520.0, 6, 63);
        assert_eq!(r, 494..516);
        assert!(visible_lines(&offsets, 0.0, 100_000.0, 6, 63).len() <= 63);
        // The end of the list, and an empty one.
        assert_eq!(visible_lines(&offsets, 990.0 * 56.0, 520.0, 6, 63).end, 1000);
        assert_eq!(visible_lines(&[0.0], 0.0, 520.0, 6, 63), 0..0);
    }

    #[test]
    fn line_offsets_add_up_headings_rows_and_progress_rows() {
        let mut list = history(1, 2);
        list[1].progress = Some(0.5);
        let groups = day_groups(&list);
        assert_eq!(groups.len(), 1);
        let lines = [Line::Header(0), Line::Row(0), Line::Row(1)];
        let offsets = line_offsets(&lines, &list);
        assert_eq!(offsets[1], HEADER_HEIGHT + LINE_GAP);
        assert_eq!(offsets[2], offsets[1] + ROW_HEIGHT + LINE_GAP);
        assert_eq!(offsets[3], offsets[2] + ROW_HEIGHT + PROGRESS_ROOM + LINE_GAP);
    }

    // ---- the sheet ----

    /// With thousands of rows only the visible ones (and a margin) are widgets, and scrolling
    /// to the very end keeps it so.
    #[test]
    fn a_long_history_builds_only_the_visible_rows() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(30, 1_000), &config);
        assert_eq!(sheet.item_count(), 30_000);
        let widgets = |ui: &Ui| {
            fn count(ui: &Ui, n: NodeId) -> usize {
                1 + ui.children(n).into_iter().map(|c| count(ui, c)).sum::<usize>()
            }
            count(ui, ui.root())
        };
        assert!(sheet.built_count() < 30, "{} lines built", sheet.built_count());
        let at_top = widgets(&ui);
        for step in 0..200 {
            sheet.scroll_by(&mut ui, &favicons, -300.0 - step as f32);
            sheet.animate(&mut ui, 1.0 / 60.0, None, &favicons);
            ui.layout();
            assert!(sheet.built_count() < SLOTS);
        }
        sheet.scroll_to(&mut ui, &favicons, 1.0e9);
        ui.layout();
        assert!(sheet.scroll() > 1.0e6, "scrolled deep into the list");
        assert!(sheet.built_count() < 40);
        // The tree stays about as big as at the top, wherever the list is scrolled to.
        assert!(widgets(&ui) < at_top * 2 + 100, "{} widgets at the end, {at_top} at the top", widgets(&ui));
    }

    /// Older days are folded when the sheet opens (today stays open), a search shows them open,
    /// and a day the user opened stays open.
    #[test]
    fn older_days_start_folded_and_keep_what_the_user_opened() {
        let config = Config::default();
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(5, 10), &config);
        // Today's heading and its ten rows, then four more headings.
        assert_eq!(sheet.lines.len(), 1 + 10 + 4);
        assert!(matches!(sheet.lines[11], Line::Header(1)));
        sheet.toggle_group(&mut ui, &favicons, 1);
        assert_eq!(sheet.lines.len(), 1 + 10 + 1 + 10 + 3);
        // A new load (a delete elsewhere, a refresh) keeps Day 1 open.
        let again = history(5, 10);
        sheet.set_items(&mut ui, again, Some("Clear"), "", &favicons, true);
        assert_eq!(sheet.lines.len(), 1 + 10 + 1 + 10 + 3);
        // Folding today works too.
        sheet.toggle_group(&mut ui, &favicons, 0);
        assert_eq!(sheet.lines.len(), 1 + 1 + 10 + 3);
        // With the setting off everything is open.
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (_, sheet, _) = opened(SheetKind::History, history(5, 10), &config);
        assert_eq!(sheet.lines.len(), 5 + 50);
    }

    #[test]
    fn downloads_are_never_folded() {
        let (_, sheet, _) = opened(SheetKind::Downloads, history(3, 4), &Config::default());
        assert_eq!(sheet.lines.len(), 3 + 12);
    }

    #[test]
    fn range_select_skips_rows_of_folded_days() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(3, 4), &config);
        sheet.toggle_group(&mut ui, &favicons, 1); // Day 1 folds: rows 4..8 are hidden
        sheet.click_row(&mut ui, 1, false);
        sheet.click_row(&mut ui, 9, true);
        assert_eq!(sheet.selected_items(), [1, 2, 3, 8, 9]);
        assert!(sheet.is_selecting());
    }

    #[test]
    fn select_all_day_and_delete_through_the_sheet() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(3, 4), &config);
        sheet.click_group_check(&mut ui, 1);
        assert_eq!(sheet.selected_items(), [4, 5, 6, 7]);
        sheet.click_row(&mut ui, 0, false);
        assert_eq!(sheet.selected_count(), 5);
        sheet.select_all(&mut ui);
        assert_eq!(sheet.selected_count(), 12);
        // Delete the middle day: its heading goes, the others stay, the mode ends.
        sheet.selection.select_all(4..8);
        let gone = sheet.selected_items();
        sheet.remove_items(&mut ui, &favicons, &gone);
        assert_eq!(sheet.item_count(), 8);
        assert_eq!(sheet.groups.len(), 2);
        assert!(!sheet.is_selecting() && sheet.selected_count() == 0);
        // Deleting the first row of a day hands the heading to the next one.
        sheet.remove_items(&mut ui, &favicons, &[4]);
        assert_eq!(sheet.groups.len(), 2);
        assert_eq!(sheet.items[4].section.as_deref(), Some("Day 2"));
    }

    #[test]
    fn the_selection_survives_a_reload_by_address() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(2, 4), &config);
        sheet.click_row(&mut ui, 5, false);
        let url = sheet.item_url(5).map(str::to_owned);
        let mut reloaded = history(2, 4);
        reloaded.remove(0);
        sheet.set_items(&mut ui, reloaded, None, "", &favicons, true);
        assert_eq!(sheet.selected_items().len(), 1);
        assert_eq!(sheet.item_url(sheet.selected_items()[0]).map(str::to_owned), url);
    }

    /// The pointer path for selecting and deleting: a click on a row's checkbox selects it, a
    /// Shift+click (as the browser decodes it) extends, and the bar shows the count.
    #[test]
    fn a_checkbox_click_reaches_the_selection() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(1, 5), &config);
        // Select mode first, so the checkboxes take clicks.
        sheet.toggle_selecting(&mut ui);
        settle(&mut ui, &mut sheet, &favicons);
        assert!(ui.is_visible(sheet.bar));
        let check = ActionId(sheet.base + ROWS_AT + 2 * ROW_KINDS + KIND_CHECK);
        let node = find_action(&ui, sheet.list, check).expect("the second row's checkbox");
        let clicked = click_node(&mut ui, node);
        assert_eq!(clicked, [check]);
        let Some(Click::Check(i)) = sheet.click(&ui, clicked[0], None) else { panic!("not a checkbox") };
        sheet.click_row(&mut ui, i, false);
        assert_eq!(sheet.selected_items(), [1]);
    }

    /// A hidden checkbox lets the pointer through: a click on a row outside selection mode
    /// opens it.
    #[test]
    fn a_hidden_checkbox_does_not_take_the_click() {
        let mut config = Config::default();
        config.history.collapse_older_days = false;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(1, 3), &config);
        settle(&mut ui, &mut sheet, &favicons);
        let row = ActionId(sheet.base + ROWS_AT + ROW_KINDS + KIND_ROW);
        let node = find_action(&ui, sheet.list, row).expect("the first row");
        // The row's left end is where the icon, and under it the checkbox, are.
        let f = ui.frame(node);
        let at = Vec2::new(f.origin.x + 10.0 + CHECK / 2.0, f.origin.y + f.size.y / 2.0);
        let _ = ui.handle(UiInput::PointerMove(at));
        let _ = ui.handle(UiInput::PointerDown);
        let (_, up) = ui.handle(UiInput::PointerUp);
        let clicked: Vec<ActionId> = up
            .iter()
            .filter_map(|e| match e {
                UiEvent::Clicked { action, .. } => Some(*action),
                _ => None,
            })
            .collect();
        assert_eq!(clicked, [row]);
    }

    /// The Downloads sheet's "Show in Folder" button, clicked through the real pointer path (move,
    /// press, release over the laid-out button): exactly one click with the id the sheet decodes
    /// as that row's first button, and no other owner of the id range claims it.
    #[test]
    fn a_row_button_is_clicked_through_pointer_dispatch() {
        let (mut ui, sheet, _) = opened(SheetKind::Downloads, items(), &Config::default());
        // Line 0 is "Today"; the first row is line 1.
        let button = ActionId(sheet.base + ROWS_AT + ROW_KINDS + KIND_BUTTON);
        let node = find_action(&ui, sheet.list, button).expect("the row has its button");
        assert!(ui.frame(node).size.x > 1.0, "the button is laid out");
        let clicked = click_node(&mut ui, node);
        assert_eq!(clicked, [button]);
        let centre = ui.frame(node).center();
        assert_eq!(sheet.click(&ui, clicked[0], Some(centre)), Some(Click::Button(0, 0)));
        assert_eq!(crate::action_ids::owners(clicked[0]), ["downloads sheet"]);
    }

    #[test]
    fn the_backdrop_closes_only_from_outside_the_sheet() {
        let (ui, sheet, _) = opened(SheetKind::History, Vec::new(), &Config::default());
        let backdrop = ActionId(sheet.base + CLOSE_BACKDROP);
        let inside = ui.frame(sheet.sheet).center();
        assert_eq!(sheet.click(&ui, backdrop, Some(inside)), None);
        assert_eq!(sheet.click(&ui, backdrop, Some(Vec2::new(2.0, 2.0))), Some(Click::Close));
    }

    /// A download shows its source site's icon when the favicon store has one, else the file icon.
    #[test]
    fn a_download_row_shows_the_site_icon_when_there_is_one() {
        let (mut ui, mut sheet) = built(SheetKind::Downloads);
        let mut favicons = favicons();
        sheet.set_items(&mut ui, items(), None, "", &favicons, false);
        let found =
            |s: &ListSheet| s.built.iter().filter(|b| b.icon_slot.is_some()).map(|b| b.icon_found).next();
        assert_eq!(found(&sheet), Some(false), "no icon known: the file's glyph");
        let rgba: Vec<u8> = (0..16 * 16).flat_map(|_| [0x20, 0x80, 0xF0, 0xFF]).collect();
        favicons.insert("https://example.org/", "https://example.org/favicon.png", 16, 16, &rgba, false);
        // The icon arrives while the sheet is open: its rows are rebuilt with it.
        sheet.sync_icons(&mut ui, &favicons);
        assert_eq!(found(&sheet), Some(true), "the site's icon replaces the glyph");
        // With `downloads.show_site_icons` off the item says so and keeps the glyph.
        let mut plain = items();
        plain[0].site_icon = false;
        sheet.set_items(&mut ui, plain, None, "", &favicons, false);
        assert_eq!(found(&sheet), Some(false));
    }

    #[test]
    fn rows_without_a_site_icon_keep_the_file_icon_and_rebuild_when_it_arrives() {
        let (mut ui, mut sheet) = built(SheetKind::Downloads);
        let favicons = favicons();
        sheet.set_items(&mut ui, items(), None, "", &favicons, false);
        assert!(sheet.built.iter().filter(|b| b.icon_slot.is_some()).all(|b| !b.icon_found));
        // Nothing arrived: no rebuild (the rows' nodes stay).
        let before = ui.children(sheet.list);
        sheet.sync_icons(&mut ui, &favicons);
        assert_eq!(ui.children(sheet.list), before);
    }

    /// Typing rests for the debounce time before the query is handed out.
    #[test]
    fn a_query_waits_for_the_debounce() {
        let mut config = Config::default();
        config.history.search_debounce_ms = 50;
        let (mut ui, mut sheet, favicons) = opened(SheetKind::History, history(1, 3), &config);
        ui.set_field_text(sheet.search_field(), "rust");
        assert_eq!(sheet.animate(&mut ui, 0.016, None, &favicons), None);
        assert!(sheet.search_pending());
        std::thread::sleep(Duration::from_millis(70));
        assert_eq!(sheet.animate(&mut ui, 0.016, None, &favicons).as_deref(), Some("rust"));
        assert_eq!(sheet.query(), "rust");
        assert!(!sheet.search_pending());
    }

    /// "Show in Folder" of a finished download, from the pointer click on its button in the sheet
    /// to the request the desktop gets: every step is the real one (the Ui's press and release,
    /// the sheet's decoding, the downloads list's effect), only the desktop itself is recorded.
    #[test]
    fn the_show_in_folder_button_reaches_the_file_manager() {
        use crate::downloads::{self, Downloads, Launch, List};
        use axion_store::{Store, StoredDownload};

        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("report.pdf");
        std::fs::write(&file, b"%PDF").expect("file");
        let store = Store::open(&dir.path().join("axion.db")).expect("store");
        let record = |name: &str, path: &Path, state: &str| StoredDownload {
            id: 0,
            url: format!("https://example.org/{name}"),
            path: path.to_string_lossy().into_owned(),
            file_name: name.into(),
            mime_type: String::new(),
            size: 4,
            state: state.into(),
            finished: 1_000,
        };
        store.add_download(&record("report.pdf", &file, "complete")).expect("add");
        let config = Config::default();
        let mut downloads = Downloads::new(dir.path().to_path_buf());
        let items = downloads.items(List::Panel, Some(&store), &config, 2_000, usize::MAX);
        assert_eq!(items[0].buttons, [Icon::Folder], "a finished download has its folder button");

        let (mut ui, sheet, _) = opened(SheetKind::Downloads, items, &config);
        let button = ActionId(sheet.base + ROWS_AT + ROW_KINDS + KIND_BUTTON);
        let node = find_action(&ui, sheet.list, button).expect("the row's button");
        let centre = ui.frame(node).center();
        let action = click_node(&mut ui, node).pop().expect("a click");
        let Some(Click::Button(row, k)) = sheet.click(&ui, action, Some(centre)) else {
            panic!("not a button")
        };
        downloads::LAUNCHED.with_borrow_mut(Vec::clear);
        downloads.row_button_with(List::Panel, row, k, &mut |_, _| panic!("nothing runs in the engine"));
        let launched = downloads::LAUNCHED.with_borrow(Clone::clone);
        assert_eq!(launched, [Launch::Show(file.clone())]);
    }
}
