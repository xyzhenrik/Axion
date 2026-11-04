//! History and Downloads as glass sheets: the same big sheet as Settings and Passwords (it flows
//! out of the sidebar row that opened it and drains back into it, over the dimmed window), with
//! a title, an optional search field, the list of [`PanelItem`] rows grouped under day headings,
//! and the buttons of the rows (a download's pause, show in folder …).
//!
//! The sheet only shows what the browser hands it (`set_items`) and says what was clicked
//! ([`Click`]); the browser does the work, as it does for the sidebar lists. Each sheet owns a
//! block of action ids (`base` + [`SPAN`]), see [`crate::action_ids`].

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::chrome::{PANEL_BUTTONS, PanelItem, panel_row_icon};
use crate::drip::Phase;
use crate::favicons::Favicons;
use crate::morph::{Origin, SHEET_BACKDROP_DIM, SHEET_MARGIN, SHEET_RADIUS, SheetMorph};
use crate::url::display_host;

/// The most rows a sheet lists (one block of action ids each).
pub const MAX_ROWS: usize = 500;
/// Action ids per row: the row itself, its ×, and its trailing buttons.
const ROW_KINDS: u32 = 2 + PANEL_BUTTONS as u32;
const ROWS_AT: u32 = 100;
/// Ids a sheet uses from its base.
pub const SPAN: u32 = ROWS_AT + ROW_KINDS * MAX_ROWS as u32;

const CLOSE_BACKDROP: u32 = 0;
const CLOSE_BUTTON: u32 = 1;
const CLEAR: u32 = 2;

const SHEET_MAX: Vec2 = Vec2::new(760.0, 640.0);
const ROW_HEIGHT: f32 = 54.0;
const PROGRESS_ROOM: f32 = 12.0;
const PROGRESS_HEIGHT: f32 = 4.0;
const ICON: f32 = 22.0;
const HEADER_BUTTON: f32 = 34.0;
const SEARCH_HEIGHT: f32 = 36.0;

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

/// What a click in the sheet asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Close,
    /// The header's clear button.
    Clear,
    Open(usize),
    Delete(usize),
    /// The `k`th trailing button of a row.
    Button(usize, usize),
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
    search_box: NodeId,
    search: NodeId,
    viewport: NodeId,
    list: NodeId,
    scroll: f32,
    open: bool,
    /// The query the list was last filtered with.
    query: String,
    /// The shown rows: their nodes, and whether their site icon was found when built.
    rows: Vec<Row>,
    /// The rows' data (kept for a rebuild when an icon arrives).
    items: Vec<PanelItem>,
    empty_text: String,
}

struct Row {
    icon_found: bool,
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

impl ListSheet {
    /// Builds the (hidden) sheet on the root of `ui`.
    pub fn build(ui: &mut Ui, kind: SheetKind) -> Self {
        let base = kind.base();
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
        // The clear button is a capsule of glass with a label (hidden when there is nothing to clear).
        let clear = ui.add(
            header,
            Widget::container()
                .with_action(ActionId(base + CLEAR))
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin)),
            Style {
                size: Size { width: auto(), height: length(HEADER_BUTTON) },
                padding: padding(16.0, 0.0),
                flex_shrink: 0.0,
                ..row(0.0)
            },
        );
        let clear_label = ui.add_label(clear, "", TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(clear_label) {
            w.set_paint(Paint::Accent);
        }
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
            Style { flex_shrink: 0.0, size: Size { width: percent(1.0), height: auto() }, ..column(2.0) },
        );
        ui.set_visible(root, false);
        let morph = SheetMorph::build(sheet, root, SHEET_RADIUS, SHEET_BACKDROP_DIM);
        Self {
            kind,
            base,
            root,
            sheet,
            morph,
            origin: None,
            frame: Frame::default(),
            clear,
            clear_label,
            search_box,
            search,
            viewport,
            list,
            scroll: 0.0,
            open: false,
            query: String::new(),
            rows: Vec::new(),
            items: Vec::new(),
            empty_text: String::new(),
        }
        .with_search(kind == SheetKind::History, ui)
    }

    fn with_search(self, shown: bool, ui: &mut Ui) -> Self {
        ui.set_visible(self.search_box, shown);
        self
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

    /// The decoded meaning of a click on `action`; the backdrop only closes from outside the sheet.
    pub fn click(&self, ui: &Ui, action: ActionId, pointer: Option<Vec2>) -> Option<Click> {
        let n = action.0.checked_sub(self.base)?;
        match n {
            CLOSE_BACKDROP => (!pointer
                .is_some_and(|p| ui.is_visible(self.sheet) && ui.frame(self.sheet).contains(p)))
            .then_some(Click::Close),
            CLOSE_BUTTON => Some(Click::Close),
            CLEAR => Some(Click::Clear),
            n if (ROWS_AT..SPAN).contains(&n) => {
                let n = n - ROWS_AT;
                let (i, k) = ((n / ROW_KINDS) as usize, n % ROW_KINDS);
                Some(match k {
                    0 => Click::Open(i),
                    1 => Click::Delete(i),
                    k => Click::Button(i, k as usize - 2),
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
            Click::Open(i) | Click::Delete(i) | Click::Button(i, _) => Some(i),
            _ => None,
        }
    }

    /// The address of row `i` (a history entry's page, a download's source).
    pub fn item_url(&self, i: usize) -> Option<&str> {
        self.items.get(i).map(|item| item.url.as_str())
    }

    /// Opens out of `origin` (or `fallback`); `window` is the window's logical size.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, window: Vec2, origin: Origin, fallback: Origin) {
        self.origin = Some((origin, fallback));
        self.open = true;
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
        let draining = ui.is_visible(self.root) && self.morph.is_shown();
        self.morph.close(ui, config, fallback);
        if !draining {
            ui.set_visible(self.root, false);
        }
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

    /// Advances the flow; returns the search text when it changed since the last call.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) -> Option<String> {
        let phase = self.morph.animate(ui, dt);
        if !self.open && phase == Phase::Closed && ui.is_visible(self.root) {
            ui.set_visible(self.root, false);
        }
        if self.open && self.kind == SheetKind::History && ui.is_visible(self.sheet) {
            let text = ui.field_text(self.search).unwrap_or_default().trim().to_owned();
            if text != self.query {
                self.query = text.clone();
                return Some(text);
            }
        }
        None
    }

    /// The search text the list was filtered with.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Scrolls the list by `dy` logical px (wheel: positive scrolls up).
    pub fn scroll_by(&mut self, ui: &mut Ui, dy: f32) {
        let max = (ui.frame(self.list).size.y - ui.frame(self.viewport).size.y).max(0.0);
        self.set_scroll(ui, (self.scroll - dy).clamp(0.0, max));
    }

    fn set_scroll(&mut self, ui: &mut Ui, scroll: f32) {
        self.scroll = scroll;
        let mut style = ui.style(self.list);
        style.margin =
            Rect { left: length(0.0), right: length(0.0), top: length(-scroll), bottom: length(0.0) };
        ui.set_style(self.list, style);
    }

    /// Shows `items`; `clear` is the clear button's label (none: no button). `keep_scroll` after
    /// a change in the list (a row removed).
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
        self.items = items;
        self.build_rows(ui, favicons);
        if !keep_scroll {
            self.set_scroll(ui, 0.0);
        }
    }

    /// Relabels the clear button (its confirming second click).
    pub fn set_clear_label(&mut self, ui: &mut Ui, label: &str) {
        ui.set_text(self.clear_label, label);
    }

    /// Rebuilds the rows whose site icon was missing and has arrived since. Call while open.
    pub fn sync_icons(&mut self, ui: &mut Ui, favicons: &Favicons) {
        let arrived = self.rows.iter().zip(&self.items).any(|(row, item)| {
            !row.icon_found && item.icon_wants_site() && favicons.get(&item.url).is_some()
        });
        if arrived {
            self.build_rows(ui, favicons);
        }
    }

    fn build_rows(&mut self, ui: &mut Ui, favicons: &Favicons) {
        ui.clear_children(self.list);
        self.rows.clear();
        if self.items.is_empty() {
            let empty = ui.add_label(
                self.list,
                &self.empty_text.clone(),
                TextStyle::Body,
                Style { padding: padding(12.0, 12.0), ..Default::default() },
            );
            if let Some(w) = ui.widget_mut(empty) {
                w.set_paint(Paint::Secondary);
            }
            return;
        }
        for (i, item) in self.items.iter().take(MAX_ROWS).enumerate() {
            if let Some(section) = &item.section {
                let heading = ui.add_label(
                    self.list,
                    section,
                    TextStyle::BodyEmphasis,
                    Style {
                        padding: Rect {
                            left: length(10.0),
                            right: length(10.0),
                            top: length(if i == 0 { 4.0 } else { 14.0 }),
                            bottom: length(4.0),
                        },
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                );
                if let Some(w) = ui.widget_mut(heading) {
                    w.set_paint(Paint::Secondary);
                }
            }
            let height = ROW_HEIGHT + if item.progress.is_some() { PROGRESS_ROOM } else { 0.0 };
            let id = |kind: u32| ActionId(self.base + ROWS_AT + i as u32 * ROW_KINDS + kind);
            let node = ui.add(
                self.list,
                Widget::container().with_action(id(0)).with_radius(14.0),
                Style {
                    size: Size { width: auto(), height: length(height) },
                    flex_shrink: 0.0,
                    padding: padding(10.0, 0.0),
                    ..row(12.0)
                },
            );
            let icon_found = panel_row_icon(ui, node, item, ICON - 2.0, favicons);
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
            let title =
                if item.title.trim().is_empty() { display_host(&item.url) } else { item.title.clone() };
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
                icon_button(ui, node, id(2 + k as u32), icon, 30.0);
            }
            if item.deletable {
                icon_button(ui, node, id(1), Icon::Close, 30.0);
            }
            self.rows.push(Row { icon_found });
        }
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

    #[test]
    fn click_ids_round_trip() {
        let (ui, sheet) = built(SheetKind::Downloads);
        let base = sheet.base;
        let at = |n: u32| ActionId(base + n);
        assert_eq!(sheet.click(&ui, at(CLOSE_BUTTON), None), Some(Click::Close));
        assert_eq!(sheet.click(&ui, at(CLEAR), None), Some(Click::Clear));
        assert_eq!(sheet.click(&ui, at(ROWS_AT), None), Some(Click::Open(0)));
        assert_eq!(sheet.click(&ui, at(ROWS_AT + 1), None), Some(Click::Delete(0)));
        assert_eq!(sheet.click(&ui, at(ROWS_AT + 2), None), Some(Click::Button(0, 0)));
        assert_eq!(sheet.click(&ui, at(ROWS_AT + ROW_KINDS * 7 + 3), None), Some(Click::Button(7, 1)));
        assert_eq!(sheet.click(&ui, ActionId(base + SPAN), None), None);
        assert!(!sheet.owns_action(ActionId(base + SPAN)));
    }

    /// The Downloads sheet's "Show in Folder" button, clicked through the real pointer path (move,
    /// press, release over the laid-out button): exactly one click with the id the sheet decodes
    /// as that row's first button, and no other owner of the id range claims it.
    #[test]
    fn a_row_button_is_clicked_through_pointer_dispatch() {
        let (mut ui, mut sheet) = built(SheetKind::Downloads);
        let config = Config::default();
        sheet.set_items(&mut ui, items(), Some("Clear List"), "Nothing here", &favicons(), false);
        let window = Vec2::new(1200.0, 800.0);
        let origin = Origin { plane: ui.root(), point: Vec2::new(100.0, 100.0), control: Frame::default() };
        sheet.open(&mut ui, &config, window, origin, origin);
        for _ in 0..240 {
            sheet.animate(&mut ui, 1.0 / 60.0);
            ui.layout();
        }
        // Find the first row's first button by its action id.
        let button = ActionId(sheet.base + ROWS_AT + 2);
        let node = find_action(&ui, sheet.list, button).expect("the row has its button");
        let frame = ui.frame(node);
        assert!(frame.size.x > 1.0, "the button is laid out");
        let centre = frame.origin + frame.size / 2.0;
        let _ = ui.handle(UiInput::PointerMove(centre));
        let (consumed, down) = ui.handle(UiInput::PointerDown);
        assert!(consumed && down.is_empty());
        let (_, up) = ui.handle(UiInput::PointerUp);
        let clicked: Vec<ActionId> = up
            .iter()
            .filter_map(|e| match e {
                UiEvent::Clicked { action, .. } => Some(*action),
                _ => None,
            })
            .collect();
        assert_eq!(clicked, [button]);
        assert_eq!(sheet.click(&ui, clicked[0], Some(centre)), Some(Click::Button(0, 0)));
        assert_eq!(crate::action_ids::owners(clicked[0]), ["downloads sheet"]);
    }

    #[test]
    fn the_backdrop_closes_only_from_outside_the_sheet() {
        let (mut ui, mut sheet) = built(SheetKind::History);
        let config = Config::default();
        let window = Vec2::new(1200.0, 800.0);
        let origin = Origin { plane: ui.root(), point: Vec2::new(100.0, 100.0), control: Frame::default() };
        sheet.open(&mut ui, &config, window, origin, origin);
        for _ in 0..240 {
            sheet.animate(&mut ui, 1.0 / 60.0);
            ui.layout();
        }
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
        assert!(!sheet.rows[0].icon_found, "no icon known: the file's glyph");
        let rgba: Vec<u8> = (0..16 * 16).flat_map(|_| [0x20, 0x80, 0xF0, 0xFF]).collect();
        favicons.insert("https://example.org/", "https://example.org/favicon.png", 16, 16, &rgba, false);
        // The icon arrives while the sheet is open: its rows are rebuilt with it.
        sheet.sync_icons(&mut ui, &favicons);
        assert!(sheet.rows[0].icon_found, "the site's icon replaces the glyph");
        // With `downloads.show_site_icons` off the item says so and keeps the glyph.
        let mut plain = items();
        plain[0].site_icon = false;
        sheet.set_items(&mut ui, plain, None, "", &favicons, false);
        assert!(!sheet.rows[0].icon_found);
    }

    #[test]
    fn rows_without_a_site_icon_keep_the_file_icon_and_rebuild_when_it_arrives() {
        let (mut ui, mut sheet) = built(SheetKind::Downloads);
        let favicons = favicons();
        sheet.set_items(&mut ui, items(), None, "", &favicons, false);
        assert!(sheet.rows.iter().all(|r| !r.icon_found));
        // Nothing arrived: no rebuild (the rows' nodes stay).
        let before = ui.children(sheet.list);
        sheet.sync_icons(&mut ui, &favicons);
        assert_eq!(ui.children(sheet.list), before);
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

        let (mut ui, mut sheet) = built(SheetKind::Downloads);
        sheet.set_items(&mut ui, items, Some("Clear List"), "", &favicons(), false);
        let origin = Origin { plane: ui.root(), point: Vec2::new(100.0, 100.0), control: Frame::default() };
        sheet.open(&mut ui, &config, Vec2::new(1200.0, 800.0), origin, origin);
        for _ in 0..240 {
            sheet.animate(&mut ui, 1.0 / 60.0);
            ui.layout();
        }
        let button = ActionId(sheet.base + ROWS_AT + 2);
        let node = find_action(&ui, sheet.list, button).expect("the row's button");
        let centre = ui.frame(node).center();
        let _ = ui.handle(UiInput::PointerMove(centre));
        let _ = ui.handle(UiInput::PointerDown);
        let (_, up) = ui.handle(UiInput::PointerUp);
        let action = up
            .iter()
            .find_map(|e| match e {
                UiEvent::Clicked { action, .. } => Some(*action),
                _ => None,
            })
            .expect("a click");
        let Some(Click::Button(row, k)) = sheet.click(&ui, action, Some(centre)) else {
            panic!("not a button")
        };
        downloads::LAUNCHED.with_borrow_mut(Vec::clear);
        downloads.row_button_with(List::Panel, row, k, &mut |_, _| panic!("nothing runs in the engine"));
        let launched = downloads::LAUNCHED.with_borrow(Clone::clone);
        assert_eq!(launched, [Launch::Show(file.clone())]);
    }
}
