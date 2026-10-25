//! The tab overview from the design ("Tab overview", "Transition — tab zooms into its card"):
//! a grid of tab thumbnails over the blurred, dimmed window, with a search field and a glass
//! toolbar. Opening zooms the current page down into its card on a spring; closing zooms the
//! chosen card back up to full size.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config};
use axion_engine::ViewId;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, Shadow, TextStyle, Ui, UiEvent, Widget};
use glam::Vec2;

use crate::favicons::{Favicons, favicon_node};
use crate::tabs::Tabs;
use crate::url::display_host;

const SELECT_BASE: u32 = 3000;
const CLOSE_BASE: u32 = 4000;
const DONE: u32 = 20;
const NEW_TAB: u32 = 21;
const PRIVATE: u32 = 22;
const NORMAL: u32 = 23;
const SEARCH: u32 = 25;

/// Layout from the design board, in logical pixels.
const TOP: f32 = 14.0;
const HEADER_HEIGHT: f32 = 44.0;
const HEADER_SIDE: f32 = 16.0;
const GRID_TOP: f32 = 84.0;
const SEGMENT_HEIGHT: f32 = 34.0;
const CIRCLE_GAP: f32 = 18.0;
const SIDE_MIN: f32 = 48.0;
const COLUMN_GAP: f32 = 40.0;
const ROW_GAP: f32 = 36.0;
const CARD_MIN_WIDTH: f32 = 300.0;
const CARD_MAX_WIDTH: f32 = 420.0;
const MAX_COLUMNS: usize = 4;
/// Thumbnail height / width (233 / 373 in the design).
const THUMB_ASPECT: f32 = 0.625;
const THUMB_RADIUS: f32 = 12.0;
const LABEL_HEIGHT: f32 = 32.0;
const LABEL_GAP: f32 = 12.0;
const TOOLBAR_BOTTOM: f32 = 32.0;
/// Width of the top bar's right group (three circles), mirrored on the left so the segmented
/// control stays centred.
const SIDE_GROUP_WIDTH: f32 = 3.0 * HEADER_HEIGHT + 2.0 * CIRCLE_GAP;
/// Overview zoom and fade (Safari's is quick): response in seconds and damping.
const ZOOM_SPRING: (f32, f32) = (0.32, 0.86);
const FADE_SPRING: (f32, f32) = (0.22, 1.0);
/// Animation counts as finished below these distances.
const SETTLED_PX: f32 = 0.5;
const SETTLED_FADE: f32 = 0.002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverviewAction {
    Select(usize),
    Close(usize),
    Done,
    NewTab,
    /// The segmented control moved: private or normal tabs.
    ShowPrivate,
    ShowNormal,
    ToggleSearch,
}

impl OverviewAction {
    fn id(self) -> ActionId {
        ActionId(match self {
            OverviewAction::Select(i) => SELECT_BASE + i as u32,
            OverviewAction::Close(i) => CLOSE_BASE + i as u32,
            OverviewAction::Done => DONE,
            OverviewAction::NewTab => NEW_TAB,
            OverviewAction::ShowPrivate => PRIVATE,
            OverviewAction::ShowNormal => NORMAL,
            OverviewAction::ToggleSearch => SEARCH,
        })
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        Some(match id.0 {
            DONE => OverviewAction::Done,
            NEW_TAB => OverviewAction::NewTab,
            PRIVATE => OverviewAction::ShowPrivate,
            NORMAL => OverviewAction::ShowNormal,
            SEARCH => OverviewAction::ToggleSearch,
            n if (CLOSE_BASE..CLOSE_BASE + 1000).contains(&n) => {
                OverviewAction::Close((n - CLOSE_BASE) as usize)
            }
            n if (SELECT_BASE..CLOSE_BASE).contains(&n) => OverviewAction::Select((n - SELECT_BASE) as usize),
            _ => return None,
        })
    }
}

/// One card: the thumbnail image, and which tab it shows.
struct Card {
    tab: usize,
    view: ViewId,
    image: NodeId,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Closed,
    Opening,
    Open,
    Closing,
}

pub struct Overview {
    phase: Phase,
    root: NodeId,
    search: NodeId,
    search_box: NodeId,
    segments: NodeId,
    /// The segmented control inside `segments`: 0 private, 1 normal tabs.
    segmented: NodeId,
    /// (segment, its label)
    private_segment: (NodeId, NodeId),
    normal_segment: (NodeId, NodeId),
    grid: NodeId,
    ghost: NodeId,
    cards: Vec<Card>,
    fade: Animated<f32>,
    ghost_origin: Animated<Vec2>,
    ghost_size: Animated<Vec2>,
    /// Which tab the ghost shows (the active tab when opening, the chosen one when closing).
    ghost_view: Option<ViewId>,
    scroll: f32,
    content_height: f32,
    window: Vec2,
}

/// One of the overview's own springs, scaled by the global motion speed.
fn quick((response, damping): (f32, f32), speed: f32) -> Spring {
    Spring::new(response * speed, damping)
}

fn fixed(color: Color) -> Paint {
    Paint::Fixed(color)
}

fn row_style(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

/// A glass circle button of the top bar; `primary` is Safari's blue "done" checkmark.
fn glass_circle(ui: &mut Ui, parent: NodeId, action: OverviewAction, icon: Icon, primary: bool) -> NodeId {
    let background = if primary {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE).with_background(background),
        Style {
            size: Size { width: length(HEADER_HEIGHT), height: length(HEADER_HEIGHT) },
            justify_content: JustifyContent::CENTER,
            ..row_style(0.0)
        },
    );
    let glyph = Widget::icon(icon, 20.0);
    let glyph = if primary {
        glyph.with_paint(fixed(Color::rgb(0xFFFFFF))).with_icon_weight(IconWeight::Bold)
    } else {
        glyph
    };
    ui.add(node, glyph, Style::default());
    node
}

/// One segment of the [Private | N Tabs] control; returns (segment, label).
fn segment(ui: &mut Ui, parent: NodeId, text: &str) -> (NodeId, NodeId) {
    let node = ui.add(
        parent,
        Widget::container(),
        Style {
            size: Size { width: auto(), height: length(SEGMENT_HEIGHT) },
            padding: Rect { left: length(16.0), right: length(16.0), top: length(0.0), bottom: length(0.0) },
            ..row_style(0.0)
        },
    );
    let label = ui.add_label(node, text, TextStyle::Callout, Style::default());
    (node, label)
}

fn insets(left: f32, right: f32, top: f32, bottom: f32) -> Rect<LengthPercentageAuto> {
    Rect { left: length(left), right: length(right), top: length(top), bottom: length(bottom) }
}

impl Overview {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.root();
        // Full-window thick glass blurs the browser behind; a dark veil gives the design's look.
        // An opaque backdrop (Safari): nothing of the window — sidebar, bars, page — shows
        // through; the page only appears as the card that zooms in and out.
        let backdrop = ui.add(
            root,
            Widget::container().with_background(Background::Fill(Paint::Ground, 1.0)),
            Style { position: Position::Absolute, inset: insets(0.0, 0.0, 0.0, 0.0), ..Default::default() },
        );

        let grid = ui.add(
            backdrop,
            Widget::container(),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                gap: Size { width: length(COLUMN_GAP), height: length(ROW_GAP) },
                ..Default::default()
            },
        );

        // Top bar (Safari): sidebar · [Private | N Tabs] · new tab, search, done.
        let header = ui.add(
            backdrop,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect {
                    left: length(HEADER_SIDE),
                    right: length(HEADER_SIDE),
                    top: length(TOP),
                    bottom: auto(),
                },
                size: Size { width: auto(), height: length(HEADER_HEIGHT) },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                justify_content: JustifyContent::SPACE_BETWEEN,
                ..Default::default()
            },
        );
        // Circles further apart than the glass merge distance stay separate drops.
        ui.add(
            header,
            Widget::container(),
            Style { size: Size { width: length(SIDE_GROUP_WIDTH), height: auto() }, ..Default::default() },
        );
        let centre = ui.add(
            header,
            Widget::container(),
            Style { flex_grow: 1.0, justify_content: JustifyContent::CENTER, ..row_style(0.0) },
        );
        let segments = ui.add(
            centre,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(SEGMENT_HEIGHT + 6.0) },
                padding: Rect {
                    left: length(3.0),
                    right: length(3.0),
                    top: length(3.0),
                    bottom: length(3.0),
                },
                ..row_style(2.0)
            },
        );
        // Tap a segment, or hold the pill and slide it across (like a Liquid Glass switch).
        let segmented = ui.add(segments, Widget::segmented(1), row_style(2.0));
        let private_segment = segment(ui, segmented, "Private");
        let normal_segment = segment(ui, segmented, "Tabs");
        let search_box = ui.add(
            centre,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                size: Size { width: length(360.0), height: length(SEGMENT_HEIGHT + 6.0) },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..row_style(8.0)
            },
        );
        ui.add(search_box, Widget::icon(Icon::Search, 17.0).with_paint(Paint::Secondary), Style::default());
        let search = ui.add_text_field(
            search_box,
            "Search Tabs",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        ui.set_visible(search_box, false);
        let right = ui.add(header, Widget::container(), row_style(CIRCLE_GAP));
        glass_circle(ui, right, OverviewAction::NewTab, Icon::Plus, false);
        glass_circle(ui, right, OverviewAction::ToggleSearch, Icon::Search, false);
        glass_circle(ui, right, OverviewAction::Done, Icon::Check, true);

        // The zooming page, above everything and outside the fading backdrop.
        let ghost = ui.add(
            root,
            Widget::image().with_radius(THUMB_RADIUS),
            Style { position: Position::Absolute, ..Default::default() },
        );

        ui.set_visible(backdrop, false);
        ui.set_visible(ghost, false);
        Self {
            phase: Phase::Closed,
            root: backdrop,
            search,
            search_box,
            segments,
            segmented,
            private_segment,
            normal_segment,
            grid,
            ghost,
            cards: Vec::new(),
            fade: Animated::new(0.0, Spring::SMOOTH),
            ghost_origin: Animated::new(Vec2::ZERO, Spring::SMOOTH),
            ghost_size: Animated::new(Vec2::ONE, Spring::SMOOTH),
            ghost_view: None,
            scroll: 0.0,
            content_height: 0.0,
            window: Vec2::ONE,
        }
    }

    pub fn is_open(&self) -> bool {
        self.phase != Phase::Closed
    }

    pub fn search_field(&self) -> NodeId {
        self.search
    }

    /// Shows which group of tabs is listed: the selected segment is the lighter pill.
    pub fn set_mode(&mut self, ui: &mut Ui, private: bool, normal_count: usize) {
        ui.set_text(
            self.normal_segment.1,
            &format!("{normal_count} {}", if normal_count == 1 { "Tab" } else { "Tabs" }),
        );
        ui.set_value(self.segmented, if private { 0.0 } else { 1.0 });
        for ((_, label), selected) in [(self.private_segment, private), (self.normal_segment, !private)] {
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(if selected { Paint::Label } else { Paint::Secondary });
            }
        }
    }

    /// The segmented control's node and what a new value of it means.
    pub fn segment_action(&self, node: NodeId, value: f32) -> Option<OverviewAction> {
        (node == self.segmented).then_some(if value < 0.5 {
            OverviewAction::ShowPrivate
        } else {
            OverviewAction::ShowNormal
        })
    }

    /// Whether the search field is showing.
    pub fn is_searching(&self, ui: &Ui) -> bool {
        self.is_open() && ui.is_visible(self.search_box)
    }

    /// Whether `p` (logical px) is on the search field.
    pub fn search_contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.search_box).contains(p)
    }

    /// Hides the overview at once, without zooming a page out of it (e.g. into locked private
    /// browsing, whose pages must not be seen).
    pub fn close_instantly(&mut self, ui: &mut Ui) -> Vec<UiEvent> {
        self.phase = Phase::Closed;
        self.fade.snap(0.0);
        self.ghost_view = None;
        ui.set_visible(self.ghost, false);
        ui.set_visible(self.root, false);
        ui.blur()
    }

    /// Swaps the segmented control for the search field (and back). Returns the events of
    /// focusing it.
    pub fn toggle_search(&mut self, ui: &mut Ui) -> Vec<UiEvent> {
        let searching = !ui.is_visible(self.search_box);
        ui.set_visible(self.search_box, searching);
        ui.set_visible(self.segments, !searching);
        if searching {
            ui.focus(self.search)
        } else {
            ui.set_field_text(self.search, "");
            ui.blur()
        }
    }

    fn columns(&self) -> (usize, f32, f32) {
        let available = (self.window.x - 2.0 * SIDE_MIN).max(CARD_MIN_WIDTH);
        let mut columns = ((available + COLUMN_GAP) / (CARD_MIN_WIDTH + COLUMN_GAP)).floor() as usize;
        columns = columns.clamp(1, MAX_COLUMNS);
        let width = ((available - COLUMN_GAP * (columns - 1) as f32) / columns as f32).min(CARD_MAX_WIDTH);
        let grid_width = width * columns as f32 + COLUMN_GAP * (columns - 1) as f32;
        let side = (self.window.x - grid_width) / 2.0;
        (columns, width, side)
    }

    /// Rebuilds the cards for the tabs matching the search.
    pub fn rebuild(&mut self, ui: &mut Ui, tabs: &Tabs, favicons: &Favicons) {
        ui.clear_children(self.grid);
        self.cards.clear();
        let query = ui.field_text(self.search).unwrap_or_default().trim().to_lowercase();
        let (columns, width, side) = self.columns();
        let thumb_height = (width * THUMB_ASPECT).round();

        let mut grid_style = ui.style(self.grid);
        grid_style.inset = Rect {
            left: length(side),
            right: length(side),
            top: length(GRID_TOP - self.scroll),
            bottom: auto(),
        };
        ui.set_style(self.grid, grid_style);

        let mut count: usize = 0;
        for (i, tab) in tabs.all().iter().enumerate() {
            let label = tab.label();
            if !query.is_empty()
                && !label.to_lowercase().contains(&query)
                && !tab.url.to_lowercase().contains(&query)
            {
                continue;
            }
            count += 1;
            let active = i == tabs.active_index();
            let card = ui.add(
                self.grid,
                Widget::container(),
                Style {
                    size: Size { width: length(width), height: auto() },
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(0.0), height: length(LABEL_GAP) },
                    ..Default::default()
                },
            );
            // A hairline around every card (Safari marks no current card).
            let _ = active;
            let ring = Background::Fill(Paint::Hairline, 1.0);
            let ring_width = 0.5;
            let frame = ui.add(
                card,
                Widget::container()
                    .with_background(ring)
                    .with_radius(THUMB_RADIUS + ring_width)
                    .with_shadow(Shadow { offset_y: 8.0, blur: 24.0, opacity: 0.18 })
                    .with_action(OverviewAction::Select(i).id()),
                Style {
                    size: Size {
                        width: length(width + 2.0 * ring_width),
                        height: length(thumb_height + 2.0 * ring_width),
                    },
                    padding: Rect {
                        left: length(ring_width),
                        right: length(ring_width),
                        top: length(ring_width),
                        bottom: length(ring_width),
                    },
                    margin: Rect {
                        left: length(-ring_width),
                        right: length(-ring_width),
                        top: length(-ring_width),
                        bottom: length(-ring_width),
                    },
                    ..Default::default()
                },
            );
            let thumb_size = Size { width: length(width), height: length(thumb_height) };
            let placeholder = ui.add(
                frame,
                Widget::container()
                    .with_background(Background::Fill(Paint::Ground, 1.0))
                    .with_radius(THUMB_RADIUS),
                Style { size: thumb_size, flex_shrink: 0.0, ..Default::default() },
            );
            // The thumbnail; the close button sits in its top-right corner as a child (taffy
            // resolves absolute children inside wrapping grids against the wrong box).
            let image = ui.add(
                placeholder,
                Widget::image().with_radius(THUMB_RADIUS),
                Style {
                    size: thumb_size,
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::START,
                    align_items: AlignItems::END,
                    padding: Rect {
                        left: length(8.0),
                        right: length(8.0),
                        top: length(8.0),
                        bottom: length(8.0),
                    },
                    ..Default::default()
                },
            );
            let close = ui.add(
                image,
                Widget::container()
                    .with_background(Background::Glass(GlassLevel::Thin))
                    .with_radius(CAPSULE)
                    .with_action(OverviewAction::Close(i).id()),
                Style {
                    size: Size { width: length(24.0), height: length(24.0) },
                    flex_shrink: 0.0,
                    align_items: AlignItems::CENTER,
                    justify_content: JustifyContent::CENTER,
                    ..Default::default()
                },
            );
            ui.add(
                close,
                Widget::icon(Icon::Close, 12.0).with_icon_weight(IconWeight::Bold),
                Style::default(),
            );

            let pill = ui.add(
                card,
                Widget::container(),
                Style {
                    size: Size { width: auto(), height: length(LABEL_HEIGHT) },
                    max_size: Size { width: length(width), height: auto() },
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::CENTER,
                    gap: Size { width: length(8.0), height: length(0.0) },
                    padding: Rect {
                        left: length(8.0),
                        right: length(14.0),
                        top: length(0.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            favicon_node(ui, pill, &tab.url, 16.0, favicons);
            let title = if label.is_empty() { display_host(&tab.url) } else { label };
            let text = ui.add_label(
                pill,
                &title,
                TextStyle::Footnote,
                Style {
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
            let _ = text;
            self.cards.push(Card { tab: i, view: tab.view, image });
        }
        // The zooming page belongs to the other group now (switched to/from Private while
        // opening): end the zoom instead of flying a card that isn't in the grid.
        if self.ghost_view.is_some_and(|v| !self.cards.iter().any(|c| c.view == v)) {
            self.ghost_origin.snap(self.ghost_origin.target);
            self.ghost_size.snap(self.ghost_size.target);
            self.ghost_view = None;
        }
        let rows = count.div_ceil(columns.max(1));
        self.content_height =
            rows as f32 * (thumb_height + LABEL_GAP + LABEL_HEIGHT) + rows.saturating_sub(1) as f32 * ROW_GAP;
    }

    /// Opens the overview, zooming the current page (`page`, logical) into its card.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        tabs: &Tabs,
        favicons: &Favicons,
        config: &Config,
        window: Vec2,
        page: Frame,
    ) -> Vec<UiEvent> {
        self.window = window;
        self.scroll = 0.0;
        ui.set_field_text(self.search, "");
        self.rebuild(ui, tabs, favicons);
        ui.set_visible(self.root, true);
        ui.layout();
        let speed = config.motion.speed;
        let reduce = config.glass.reduce_motion;
        self.fade.spring = quick(FADE_SPRING, speed);
        self.fade.snap(0.0);
        self.fade.target = 1.0;
        self.ghost_view = tabs.active().map(|t| t.view);
        let target = self.card_frame(ui, tabs.active_index()).unwrap_or(page);
        self.start_ghost(page, target, quick(ZOOM_SPRING, speed), reduce);
        self.phase = Phase::Opening;
        // Search starts hidden behind its button (Safari).
        ui.set_visible(self.search_box, false);
        ui.set_visible(self.segments, true);
        ui.set_field_text(self.search, "");
        ui.blur()
    }

    /// Closes the overview, zooming tab `index`'s card (now the active tab) back up to `page`.
    pub fn close(
        &mut self,
        ui: &mut Ui,
        tabs: &Tabs,
        config: &Config,
        index: usize,
        page: Frame,
    ) -> Vec<UiEvent> {
        if !self.is_open() || self.phase == Phase::Closing {
            return Vec::new();
        }
        let speed = config.motion.speed;
        let from = self.card_frame(ui, index).unwrap_or(page);
        self.ghost_view = tabs.all().get(index).map(|t| t.view);
        self.start_ghost(from, page, quick(ZOOM_SPRING, speed), config.glass.reduce_motion);
        self.fade.spring = quick(FADE_SPRING, speed);
        self.fade.target = 0.0;
        self.phase = Phase::Closing;
        ui.blur()
    }

    fn start_ghost(&mut self, from: Frame, to: Frame, spring: Spring, reduce_motion: bool) {
        self.ghost_origin.spring = spring;
        self.ghost_size.spring = spring;
        if reduce_motion {
            // Reduce Motion: no zoom, just the cross-fade.
            self.ghost_origin.snap(to.origin);
            self.ghost_size.snap(to.size);
        } else {
            self.ghost_origin.snap(from.origin);
            self.ghost_size.snap(from.size);
        }
        self.ghost_origin.target = to.origin;
        self.ghost_size.target = to.size;
    }

    fn card_frame(&self, ui: &Ui, tab: usize) -> Option<Frame> {
        let card = self.cards.iter().find(|c| c.tab == tab)?;
        Some(ui.frame(card.image))
    }

    pub fn scroll_by(&mut self, ui: &mut Ui, dy: f32) {
        let visible = self.window.y - GRID_TOP - TOOLBAR_BOTTOM - 56.0 - 24.0;
        let max = (self.content_height - visible).max(0.0);
        let scroll = (self.scroll - dy).clamp(0.0, max);
        if scroll != self.scroll {
            self.scroll = scroll;
            let mut style = ui.style(self.grid);
            style.inset.top = length(GRID_TOP - scroll);
            ui.set_style(self.grid, style);
        }
    }

    pub fn resize(&mut self, ui: &mut Ui, tabs: &Tabs, favicons: &Favicons, window: Vec2) {
        if self.is_open() && window != self.window {
            self.window = window;
            self.rebuild(ui, tabs, favicons);
        }
    }

    /// The first tab matching the search, for Enter in the search field.
    pub fn first_match(&self) -> Option<usize> {
        self.cards.first().map(|c| c.tab)
    }

    /// Advances the animations and refreshes thumbnails. `frame_of` gives a view's latest
    /// frame. Returns `true` when the overview just finished closing.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        dt: f32,
        frame_of: impl Fn(ViewId) -> Option<(wgpu::TextureView, (u32, u32))>,
    ) -> bool {
        if !self.is_open() {
            return false;
        }
        self.fade.step(dt);
        self.ghost_origin.step(dt);
        self.ghost_size.step(dt);
        let fade = self.fade.value.clamp(0.0, 1.0);
        ui.set_opacity(self.root, fade);

        let zooming = !(self.ghost_origin.is_settled(SETTLED_PX) && self.ghost_size.is_settled(SETTLED_PX));
        let ghost_view = self.ghost_view;
        for card in &self.cards {
            ui.set_image(card.image, frame_of(card.view));
            // The card being zoomed is shown by the ghost instead.
            let hidden = zooming && Some(card.view) == ghost_view;
            ui.set_opacity(card.image, if hidden { 0.0 } else { 1.0 });
        }
        ui.set_visible(self.ghost, zooming);
        if zooming {
            ui.set_image(self.ghost, ghost_view.and_then(&frame_of));
            let origin = self.ghost_origin.value;
            let size = self.ghost_size.value.max(Vec2::ONE);
            ui.set_style(
                self.ghost,
                Style {
                    position: Position::Absolute,
                    inset: Rect {
                        left: length(origin.x),
                        right: auto(),
                        top: length(origin.y),
                        bottom: auto(),
                    },
                    size: Size { width: length(size.x), height: length(size.y) },
                    ..Default::default()
                },
            );
        }

        match self.phase {
            Phase::Opening if !zooming && self.fade.is_settled(SETTLED_FADE) => self.phase = Phase::Open,
            Phase::Closing if !zooming && self.fade.value <= SETTLED_FADE => {
                self.phase = Phase::Closed;
                ui.set_visible(self.root, false);
                return true;
            }
            _ => {}
        }
        false
    }
}
