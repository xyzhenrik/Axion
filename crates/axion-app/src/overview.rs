//! The tab overview from the design ("Tab overview", "Transition — tab zooms into its card"):
//! a grid of tab thumbnails over the blurred, dimmed window, with a search field and a glass
//! toolbar. Opening zooms the current page down into its card on a spring; closing zooms the
//! chosen card back up to full size.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config, SpringSpec};
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

/// Layout from the design board, in logical pixels.
const TOP: f32 = 28.0;
const HEADER_HEIGHT: f32 = 52.0;
const GRID_TOP: f32 = 128.0;
const SIDE_MIN: f32 = 48.0;
const COLUMN_GAP: f32 = 40.0;
const ROW_GAP: f32 = 36.0;
const CARD_MIN_WIDTH: f32 = 300.0;
const CARD_MAX_WIDTH: f32 = 420.0;
const MAX_COLUMNS: usize = 4;
/// Thumbnail height / width (233 / 373 in the design).
const THUMB_ASPECT: f32 = 0.625;
const THUMB_RADIUS: f32 = 20.0;
const LABEL_HEIGHT: f32 = 32.0;
const LABEL_GAP: f32 = 12.0;
const TOOLBAR_BOTTOM: f32 = 32.0;
/// The design's dark glass over the blurred window.
const DIM: Color = Color::rgb(0x080A12);
const DIM_OPACITY: f32 = 0.45;
const DARK_PILL: Color = Color::rgb(0x1E2028);
const DARK_PILL_OPACITY: f32 = 0.62;
const ON_DARK: Color = Color::rgb(0xF5F6F8);
/// Accent ring around the current tab's card.
const RING: f32 = 3.0;
/// Animation counts as finished below these distances.
const SETTLED_PX: f32 = 0.5;
const SETTLED_FADE: f32 = 0.002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverviewAction {
    Select(usize),
    Close(usize),
    Done,
    NewTab,
}

impl OverviewAction {
    fn id(self) -> ActionId {
        ActionId(match self {
            OverviewAction::Select(i) => SELECT_BASE + i as u32,
            OverviewAction::Close(i) => CLOSE_BASE + i as u32,
            OverviewAction::Done => DONE,
            OverviewAction::NewTab => NEW_TAB,
        })
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        Some(match id.0 {
            DONE => OverviewAction::Done,
            NEW_TAB => OverviewAction::NewTab,
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

fn spring(spec: SpringSpec, speed: f32) -> Spring {
    Spring::new(spec.response * speed.max(0.05), spec.damping)
}

fn fixed(color: Color) -> Paint {
    Paint::Fixed(color)
}

fn insets(left: f32, right: f32, top: f32, bottom: f32) -> Rect<LengthPercentageAuto> {
    Rect { left: length(left), right: length(right), top: length(top), bottom: length(bottom) }
}

impl Overview {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.root();
        // Full-window thick glass blurs the browser behind; a dark veil gives the design's look.
        let backdrop = ui.add(
            root,
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)),
            Style { position: Position::Absolute, inset: insets(0.0, 0.0, 0.0, 0.0), ..Default::default() },
        );
        ui.add(
            backdrop,
            Widget::container().with_background(Background::Fill(fixed(DIM), DIM_OPACITY)),
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

        let header = ui.add(
            backdrop,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(TOP), bottom: auto() },
                justify_content: JustifyContent::CENTER,
                ..Default::default()
            },
        );
        let search_box = ui.add(
            header,
            Widget::container()
                .with_background(Background::Fill(fixed(DARK_PILL), DARK_PILL_OPACITY))
                .with_radius(CAPSULE),
            Style {
                size: Size { width: length(420.0), height: length(HEADER_HEIGHT) },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(10.0), height: length(0.0) },
                padding: Rect {
                    left: length(18.0),
                    right: length(18.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        ui.add(search_box, Widget::icon(Icon::Search, 19.0).with_paint(fixed(ON_DARK)), Style::default());
        let search = ui.add_text_field(
            search_box,
            "Search tabs",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        if let Some(w) = ui.widget_mut(search) {
            w.set_paint(fixed(ON_DARK));
        }

        let toolbar = ui.add(
            backdrop,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: auto(),
                    bottom: length(TOOLBAR_BOTTOM),
                },
                justify_content: JustifyContent::CENTER,
                ..Default::default()
            },
        );
        let bar = ui.add(
            toolbar,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(56.0) },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(6.0), height: length(0.0) },
                padding: Rect {
                    left: length(7.0),
                    right: length(7.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        let new_tab = ui.add(
            bar,
            Widget::container().with_action(OverviewAction::NewTab.id()).with_radius(CAPSULE),
            Style {
                size: Size { width: length(44.0), height: length(44.0) },
                align_items: AlignItems::CENTER,
                justify_content: JustifyContent::CENTER,
                ..Default::default()
            },
        );
        ui.add(new_tab, Widget::icon(Icon::Plus, 22.0), Style::default());
        let done = ui.add(
            bar,
            Widget::container()
                .with_action(OverviewAction::Done.id())
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Accent, 1.0))
                .with_shadow(Shadow { offset_y: 6.0, blur: 18.0, opacity: 0.25 }),
            Style {
                size: Size { width: auto(), height: length(44.0) },
                padding: Rect {
                    left: length(22.0),
                    right: length(22.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let done_label = ui.add_label(
            done,
            "Done",
            TextStyle::BodyEmphasis,
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        if let Some(w) = ui.widget_mut(done_label) {
            w.set_paint(fixed(ON_DARK));
        }

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
            // The accent ring of the current tab is a slightly larger fill behind the thumbnail.
            let ring = if active {
                Background::Fill(Paint::Accent, 1.0)
            } else {
                Background::Fill(fixed(Color::rgb(0xFFFFFF)), 0.18)
            };
            let ring_width = if active { RING } else { 0.5 };
            let frame = ui.add(
                card,
                Widget::container()
                    .with_background(ring)
                    .with_radius(THUMB_RADIUS + ring_width)
                    .with_shadow(Shadow { offset_y: 18.0, blur: 50.0, opacity: 0.45 })
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
            // The thumbnail; the close button sits in its bottom-left corner as a child (taffy
            // resolves absolute children inside wrapping grids against the wrong box).
            let image = ui.add(
                placeholder,
                Widget::image().with_radius(THUMB_RADIUS),
                Style {
                    size: thumb_size,
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::END,
                    align_items: AlignItems::START,
                    padding: Rect {
                        left: length(10.0),
                        right: length(10.0),
                        top: length(10.0),
                        bottom: length(10.0),
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
                    size: Size { width: length(30.0), height: length(30.0) },
                    flex_shrink: 0.0,
                    align_items: AlignItems::CENTER,
                    justify_content: JustifyContent::CENTER,
                    ..Default::default()
                },
            );
            ui.add(
                close,
                Widget::icon(Icon::Close, 14.0).with_icon_weight(IconWeight::Bold),
                Style::default(),
            );

            let pill = ui.add(
                card,
                Widget::container()
                    .with_background(Background::Fill(fixed(DARK_PILL), DARK_PILL_OPACITY))
                    .with_radius(CAPSULE),
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
            favicon_node(ui, pill, &tab.url, 18.0, favicons);
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
            if let Some(w) = ui.widget_mut(text) {
                w.set_paint(fixed(ON_DARK));
            }
            self.cards.push(Card { tab: i, view: tab.view, image });
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
        let motion = &config.motion;
        let reduce = config.glass.reduce_motion;
        self.fade.spring = spring(motion.smooth, motion.speed);
        self.fade.snap(0.0);
        self.fade.target = 1.0;
        self.ghost_view = tabs.active().map(|t| t.view);
        let target = self.card_frame(ui, tabs.active_index()).unwrap_or(page);
        self.start_ghost(page, target, spring(motion.fling, motion.speed), reduce);
        self.phase = Phase::Opening;
        ui.focus(self.search)
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
        let motion = &config.motion;
        let from = self.card_frame(ui, index).unwrap_or(page);
        self.ghost_view = tabs.all().get(index).map(|t| t.view);
        self.start_ghost(from, page, spring(motion.fling, motion.speed), config.glass.reduce_motion);
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
