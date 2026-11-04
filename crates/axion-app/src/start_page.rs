//! The native start page: the page behind `axion://home/` drawn by Axion itself (like the
//! Playlist page) so its glass is real Liquid Glass: the search field, the tiles, the gear's
//! panel and the Shields row refract the wallpaper, which the browser draws as an image layer
//! under this page's plane (see `start_wallpaper` and `browser/start.rs`).
//!
//! This module only builds and updates the nodes; what the buttons mean is decided by the
//! browser (`browser/start.rs`), what the page shows by [`StartModel`]. The HTML page stays the
//! fallback (`home.native`); both are fed by `home::HomePage` and take the same `HomeCommand`s.

use std::time::Duration;

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{ClockFormat, Color, Config};
use axion_i18n::{tr, tr_fmt};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Align, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget as UiWidget};
use glam::Vec2;

use crate::favicons::Favicons;
use crate::home::{self, Kind, Site, StartModel};
use crate::menu::{Menu, MenuAction, MenuItem, Placement};
use crate::popover::Popover;

/// Action ids of the page, inside the block `action_ids::RANGES` calls "start page".
const BASE: u32 = 50_000;
const TILE_BASE: u32 = BASE + 100;
const TILE_KINDS: u32 = 8;
/// The most tiles the page lays out (`home.top_sites_count` is far below).
pub const MAX_TILES: usize = 100;
const PICKER_BASE: u32 = TILE_BASE + TILE_KINDS * MAX_TILES as u32 + 100;
/// Backgrounds the picker lists (`settings::BACKGROUND_SLOTS`).
const MAX_PICKER: usize = 32;
const DOT_BASE: u32 = PICKER_BASE + 2 * MAX_PICKER as u32 + 36;
const MAX_DOTS: usize = 64;
/// The engine menu's rows.
pub const MENU_BASE: u32 = DOT_BASE + MAX_DOTS as u32 + 36;

/// What a click on the page means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StartAction {
    Gear,
    Engine,
    /// The format segmented control of the panel.
    Format(ClockFormat),
    Restore,
    MoreSettings,
    ChooseBackground,
    AddPath,
    BackgroundNone,
    Background(usize),
    BackgroundRemove(usize),
    EditorSave,
    EditorCancel,
    AddSite,
    Tile(usize),
    TilePin(usize),
    TileEdit(usize),
    TileRemove(usize),
    Dot(usize),
    /// A switch of the panel (from `value_changed`, no id).
    Widget(home::Widget, bool),
    /// The Dim or Blur slider (from `value_changed`, no id).
    Look {
        dim: Option<f32>,
        blur: Option<f32>,
    },
}

const SIMPLE: [StartAction; 12] = [
    StartAction::Gear,
    StartAction::Engine,
    StartAction::Restore,
    StartAction::MoreSettings,
    StartAction::ChooseBackground,
    StartAction::AddPath,
    StartAction::BackgroundNone,
    StartAction::EditorSave,
    StartAction::EditorCancel,
    StartAction::AddSite,
    StartAction::Format(ClockFormat::Auto),
    StartAction::Format(ClockFormat::H12),
];

impl StartAction {
    pub fn id(self) -> ActionId {
        let tile = |i: usize, k: u32| ActionId(TILE_BASE + i.min(MAX_TILES - 1) as u32 * TILE_KINDS + k);
        match self {
            StartAction::Tile(i) => tile(i, 0),
            StartAction::TilePin(i) => tile(i, 1),
            StartAction::TileEdit(i) => tile(i, 2),
            StartAction::TileRemove(i) => tile(i, 3),
            StartAction::Background(i) => ActionId(PICKER_BASE + 2 * i.min(MAX_PICKER - 1) as u32),
            StartAction::BackgroundRemove(i) => ActionId(PICKER_BASE + 2 * i.min(MAX_PICKER - 1) as u32 + 1),
            StartAction::Dot(i) => ActionId(DOT_BASE + i.min(MAX_DOTS - 1) as u32),
            StartAction::Format(ClockFormat::H24) => ActionId(BASE + 12),
            // No id of their own: the controls report through `value_changed`.
            StartAction::Widget(..) | StartAction::Look { .. } => ActionId(BASE + 13),
            simple => ActionId(BASE + SIMPLE.iter().position(|&a| a == simple).unwrap_or(0) as u32),
        }
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        let in_block = |base: u32, len: u32| id.0.checked_sub(base).filter(|&n| n < len);
        if let Some(n) = in_block(TILE_BASE, TILE_KINDS * MAX_TILES as u32) {
            let i = (n / TILE_KINDS) as usize;
            return Some(match n % TILE_KINDS {
                0 => StartAction::Tile(i),
                1 => StartAction::TilePin(i),
                2 => StartAction::TileEdit(i),
                3 => StartAction::TileRemove(i),
                _ => return None,
            });
        }
        if let Some(n) = in_block(PICKER_BASE, 2 * MAX_PICKER as u32) {
            let i = (n / 2) as usize;
            return Some(if n % 2 == 0 {
                StartAction::Background(i)
            } else {
                StartAction::BackgroundRemove(i)
            });
        }
        if let Some(n) = in_block(DOT_BASE, MAX_DOTS as u32) {
            return Some(StartAction::Dot(n as usize));
        }
        match id.0.checked_sub(BASE)? {
            12 => Some(StartAction::Format(ClockFormat::H24)),
            n @ 0..=11 => SIMPLE.get(n as usize).copied(),
            _ => None,
        }
    }
}

// ---- geometry, in logical pixels (the design's "Start page" board) ----

/// Space left free at the page's sides.
const SIDE_MARGIN: f32 = 24.0;
/// The content starts this far from the top: a share of the page's height, at least `TOP_MIN`.
const TOP_SHARE: f32 = 0.09;
const TOP_MIN: f32 = 64.0;
const GAP: f32 = 28.0;
const SEARCH_WIDTH: f32 = 600.0;
const SEARCH_HEIGHT: f32 = 52.0;
const SITES_WIDTH: f32 = 880.0;
const TILE_WIDTH: f32 = 132.0;
const TILE_HEIGHT: f32 = 116.0;
const TILE_GAP: Vec2 = Vec2::new(14.0, 18.0);
const FACE: f32 = 76.0;
const FACE_RADIUS: f32 = 22.0;
const ICON: f32 = 40.0;
const ICON_RADIUS: f32 = 11.0;
/// A column of the paging math (the HTML page's `118`): tiles plus their gap, estimated.
const COLUMN_ESTIMATE: f32 = 118.0;
const MINI: f32 = 24.0;
const GEAR: f32 = 40.0;
const GEAR_MARGIN: f32 = 18.0;
const STATS_BOTTOM: f32 = 18.0;
/// The Shields row hides when the content comes closer than this.
const STATS_CLEARANCE: f32 = 12.0;
const PRIVATE_WIDTH: f32 = 560.0;
const PANEL_WIDTH: f32 = 320.0;
const PANEL_PAD: f32 = 16.0;
const PANEL_ROW: f32 = 40.0;
const PANEL_RADIUS: f32 = 24.0;
const SWATCH: Vec2 = Vec2::new(62.0, 40.0);
const SWATCH_GAP: f32 = 8.0;
const DIM_MAX: f32 = 0.8;
const BLUR_MAX: f32 = 30.0;
const EDITOR_WIDTH: f32 = 380.0;
const MENU_ANCHOR_GAP: f32 = 6.0;
/// Springs: paging (smooth, no overshoot beyond the config's), tile actions, the editor's fade.
const PAGE_SPRING: (f32, f32) = (0.5, 0.9);
const POP_SPRING: (f32, f32) = (0.3, 0.7);
const SETTLED: f32 = 0.5;
const SETTLED_UNIT: f32 = 0.004;

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

fn square(side: f32) -> Size<Dimension> {
    Size { width: length(side), height: length(side) }
}

fn pad(l: f32, r: f32, t: f32, b: f32) -> Rect<LengthPercentage> {
    Rect { left: length(l), right: length(r), top: length(t), bottom: length(b) }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

/// What a label's colour means (over a wallpaper they turn light).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ink {
    Primary,
    Secondary,
    Accent,
    /// The Shields numbers' own colours.
    Blocked,
    Bandwidth,
}

const BANDWIDTH_COLOR: Color = Color::rgb(0x8C8BFF);
const WHITE: Color = Color::rgb(0xFFFFFF);
const LIGHT_SECONDARY: Color = Color::rgb(0xE8E8EE);
const SHIELD_OVER_IMAGE: Color = Color::rgb(0x7BE0A8);

fn paint_of(ink: Ink, over_image: bool) -> Paint {
    match (ink, over_image) {
        (Ink::Primary, false) => Paint::Label,
        (Ink::Primary, true) => Paint::Fixed(WHITE),
        (Ink::Secondary, false) => Paint::Secondary,
        (Ink::Secondary, true) => Paint::Fixed(LIGHT_SECONDARY),
        (Ink::Accent, false) => Paint::Accent,
        (Ink::Accent, true) => Paint::Fixed(WHITE),
        (Ink::Blocked, false) => Paint::Accent,
        (Ink::Blocked, true) => Paint::Fixed(SHIELD_OVER_IMAGE),
        (Ink::Bandwidth, _) => Paint::Fixed(BANDWIDTH_COLOR),
    }
}

struct TileNodes {
    root: NodeId,
    face: NodeId,
    /// The mini buttons (pin, edit, remove), none for the Add Site tile.
    actions: Option<NodeId>,
    hover: Animated<f32>,
}

/// What the panel's controls are.
#[derive(Clone, Copy, PartialEq)]
enum Control {
    Toggle(home::Widget),
    Dim,
    Blur,
}

struct Panel {
    popover: Popover,
    controls: Vec<(NodeId, Control)>,
    path_field: NodeId,
    note: Option<NodeId>,
    /// What the content was built from (the structure), to rebuild only when it changes.
    signature: String,
    height: f32,
}

struct Editor {
    backdrop: NodeId,
    title: NodeId,
    name: NodeId,
    url: NodeId,
    shown: Animated<f32>,
    open: bool,
    /// The site being edited (its address), empty for a new one.
    old: String,
}

/// Where the keyboard's focus is on the page (Tab walks through them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    None,
    Gear,
    Engine,
    Query,
    /// A tile (the Add Site tile is the one after the last site).
    Tile(usize),
}

pub struct StartPage {
    root: NodeId,
    /// A plane over the page for what floats above its glass: the gear's panel, the engine menu
    /// and the dialog (so their glass refracts the tiles under them).
    overlay: NodeId,
    main: NodeId,
    gear: NodeId,
    clock_block: NodeId,
    clock_rows: [(NodeId, NodeId, NodeId); 3],
    greeting: NodeId,
    private_card: NodeId,
    search: NodeId,
    engine: NodeId,
    engine_icon: NodeId,
    query: NodeId,
    sites: NodeId,
    viewport: NodeId,
    track: NodeId,
    dots: NodeId,
    stats: NodeId,
    stat_values: [NodeId; 3],
    labels: Vec<(NodeId, Ink)>,
    tile_labels: Vec<(NodeId, Ink)>,
    thumb_nodes: Vec<(String, NodeId, bool)>,
    tiles: Vec<TileNodes>,
    add_tile: Option<TileNodes>,
    panel: Panel,
    editor: Editor,
    menu: Menu,
    model: Option<StartModel>,
    rect: Frame,
    open: bool,
    over_image: bool,
    private: bool,
    columns: u32,
    pages: usize,
    page: usize,
    offset: Animated<f32>,
    swipe: f32,
    swipe_locked_until: Option<std::time::Instant>,
    focus: Focus,
    tag: String,
    sig: String,
    flat: bool,
}

impl StartPage {
    pub fn build(ui: &mut Ui, config: &Config) -> Self {
        let root = ui.add(
            ui.root(),
            // Fill at alpha 1 as the page's ground; `set_wallpaper` makes it transparent (the
            // wallpaper layer lies under this plane) but keeps it solid for hit-testing.
            UiWidget::container()
                .with_background(Background::Fill(Paint::Ground, 1.0))
                .with_radius(config.effective_layout().page_corner_radius)
                .with_clip(),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Column,
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        let overlay = ui.add(
            ui.root(),
            UiWidget::container(),
            Style { position: Position::Absolute, ..Default::default() },
        );
        ui.set_visible(overlay, false);
        let mut labels: Vec<(NodeId, Ink)> = Vec::new();
        fn ink_label(
            ui: &mut Ui,
            labels: &mut Vec<(NodeId, Ink)>,
            parent: NodeId,
            text: &str,
            style: TextStyle,
            ink: Ink,
            layout: Style,
        ) -> NodeId {
            let node = ui.add_label(parent, text, style, layout);
            labels.push((node, ink));
            node
        }

        let main = ui.add(
            root,
            UiWidget::container(),
            Style {
                size: Size { width: percent(1.0), height: percent(1.0) },
                align_items: AlignItems::CENTER,
                padding: pad(SIDE_MARGIN, SIDE_MARGIN, TOP_MIN, 48.0),
                ..column(GAP)
            },
        );

        // ---- clock and greeting ----
        let clock_block =
            ui.add(main, UiWidget::container(), Style { align_items: AlignItems::CENTER, ..column(10.0) });
        let sizes = [TextStyle::Clock, TextStyle::ClockMedium, TextStyle::ClockSmall];
        let clock_rows = sizes.map(|size| {
            let r = ui.add(clock_block, UiWidget::container(), row(10.0));
            let time = ink_label(ui, &mut labels, r, "00:00", size, Ink::Primary, Style::default());
            let ampm = ink_label(ui, &mut labels, r, "", TextStyle::Title2, Ink::Secondary, Style::default());
            (r, time, ampm)
        });
        let greeting =
            ink_label(ui, &mut labels, clock_block, "", TextStyle::Title2, Ink::Secondary, Style::default());

        // ---- private card ----
        let private_card = ui.add(
            main,
            UiWidget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(22.0),
            Style {
                size: Size { width: length(PRIVATE_WIDTH), height: auto() },
                max_size: Size { width: percent(1.0), height: auto() },
                padding: pad(20.0, 20.0, 18.0, 18.0),
                ..column(8.0)
            },
        );
        let title_row = ui.add(private_card, UiWidget::container(), row(10.0));
        ui.add(title_row, UiWidget::icon(Icon::EyeOff, 22.0), Style::default());
        ink_label(
            ui,
            &mut labels,
            title_row,
            tr("Private Browsing"),
            TextStyle::Title2,
            Ink::Primary,
            Style::default(),
        );
        let about = ink_label(
            ui,
            &mut labels,
            private_card,
            tr(
                "Axion doesn't remember the pages you visit in private tabs, your searches or what you type into forms. Their cookies and site data are gone when the private tabs close. Downloads and bookmarks you save are kept.",
            ),
            TextStyle::Footnote,
            Ink::Secondary,
            shrinking(),
        );
        ui.set_label_lines(about, 6);
        ui.set_visible(private_card, false);

        // ---- search ----
        let search = ui.add(
            main,
            UiWidget::container()
                .with_background(Background::Glass(GlassLevel::Regular))
                .with_radius(CAPSULE),
            Style {
                size: Size { width: length(SEARCH_WIDTH), height: length(SEARCH_HEIGHT) },
                flex_shrink: 0.0,
                padding: pad(6.0, 18.0, 0.0, 0.0),
                ..row(6.0)
            },
        );
        let engine = ui.add(
            search,
            UiWidget::container()
                .with_action(StartAction::Engine.id())
                .with_radius(CAPSULE)
                .with_tooltip(tr("Search engine"), None),
            Style {
                size: Size { width: auto(), height: length(40.0) },
                padding: pad(6.0, 8.0, 0.0, 0.0),
                flex_shrink: 0.0,
                ..row(4.0)
            },
        );
        let engine_icon = ui.add(
            engine,
            UiWidget::container().with_radius(8.0),
            Style { size: square(26.0), justify_content: JustifyContent::CENTER, ..row(0.0) },
        );
        ui.add(
            engine,
            UiWidget::icon(Icon::ChevronDown, 12.0).with_paint(Paint::Secondary),
            Style::default(),
        );
        let query = ui.add_text_field(
            search,
            tr("Search or enter website"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(24.0) },
                ..Default::default()
            },
        );
        ui.add(
            search,
            UiWidget::icon(Icon::Search, 18.0).with_paint(Paint::Secondary),
            Style { flex_shrink: 0.0, ..Default::default() },
        );

        // ---- top sites ----
        let sites =
            ui.add(main, UiWidget::container(), Style { align_items: AlignItems::CENTER, ..column(14.0) });
        let viewport = ui.add(
            sites,
            UiWidget::container().with_clip(),
            Style {
                size: Size { width: length(SITES_WIDTH), height: length(TILE_HEIGHT) },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        let track = ui.add(
            viewport,
            UiWidget::container(),
            Style { flex_direction: FlexDirection::Row, ..Default::default() },
        );
        let dots = ui.add(sites, UiWidget::container(), row(0.0));

        // ---- Shields ----
        let stats = ui.add(
            root,
            UiWidget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect {
                    left: length(SIDE_MARGIN),
                    right: length(SIDE_MARGIN),
                    top: auto(),
                    bottom: length(STATS_BOTTOM),
                },
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(28.0), height: length(4.0) },
                ..Default::default()
            },
        );
        ui.set_passthrough(stats, true);
        let mut stat_values = [stats; 3];
        for (i, (text, ink)) in [
            (tr("trackers & ads blocked"), Ink::Blocked),
            (tr("bandwidth saved"), Ink::Bandwidth),
            (tr("time saved"), Ink::Primary),
        ]
        .into_iter()
        .enumerate()
        {
            let stat = ui.add(stats, UiWidget::container(), row(7.0));
            stat_values[i] =
                ink_label(ui, &mut labels, stat, "", TextStyle::BodyEmphasis, ink, Style::default());
            ink_label(ui, &mut labels, stat, text, TextStyle::Footnote, Ink::Secondary, Style::default());
        }

        // ---- gear, panel, editor, engine menu ----
        let gear = ui.add(
            root,
            UiWidget::container()
                .with_action(StartAction::Gear.id())
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_tooltip(tr("Customize start page"), None),
            Style {
                position: Position::Absolute,
                size: square(GEAR),
                inset: Rect {
                    left: auto(),
                    right: length(GEAR_MARGIN),
                    top: length(GEAR_MARGIN),
                    bottom: auto(),
                },
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        ui.add(gear, UiWidget::icon(Icon::Gear, 20.0).with_paint(Paint::Accent), Style::default());

        let popover = Popover::build(
            ui,
            overlay,
            PANEL_RADIUS,
            Style {
                size: Size { width: length(PANEL_WIDTH), height: auto() },
                padding: pad(PANEL_PAD, PANEL_PAD, 14.0, 12.0),
                ..column(0.0)
            },
        )
        .unjoined();
        let panel = Panel {
            popover,
            controls: Vec::new(),
            path_field: gear,
            note: None,
            signature: String::new(),
            height: 0.0,
        };
        let editor = Self::build_editor(ui, overlay, config);
        let menu = Menu::build(ui, overlay, MENU_BASE, Placement::BelowStart);

        let mut page = Self {
            root,
            overlay,
            main,
            gear,
            clock_block,
            clock_rows,
            greeting,
            private_card,
            search,
            engine,
            engine_icon,
            query,
            sites,
            viewport,
            track,
            dots,
            stats,
            stat_values,
            labels,
            tile_labels: Vec::new(),
            thumb_nodes: Vec::new(),
            tiles: Vec::new(),
            add_tile: None,
            panel,
            editor,
            menu,
            model: None,
            rect: Frame::default(),
            open: false,
            over_image: false,
            private: false,
            columns: 1,
            pages: 1,
            page: 0,
            offset: Animated::new(0.0, Spring::new(PAGE_SPRING.0, PAGE_SPRING.1)),
            swipe: 0.0,
            swipe_locked_until: None,
            focus: Focus::None,
            tag: "en".to_owned(),
            sig: String::new(),
            flat: false,
        };
        page.apply_springs(config);
        page
    }

    fn build_editor(ui: &mut Ui, root: NodeId, _config: &Config) -> Editor {
        let backdrop = ui.add(
            root,
            UiWidget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), 0.18))
                .without_feedback(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let card = ui.add(
            backdrop,
            UiWidget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(24.0),
            Style {
                size: Size { width: length(EDITOR_WIDTH), height: auto() },
                max_size: Size { width: percent(0.92), height: auto() },
                padding: uniform(20.0),
                ..column(10.0)
            },
        );
        let title = ui.add_label(card, tr("Add Site"), TextStyle::Title2, Style::default());
        let field = |ui: &mut Ui, caption: &str, placeholder: &str| {
            let c = ui.add_label(card, caption, TextStyle::Caption, Style::default());
            if let Some(w) = ui.widget_mut(c) {
                w.set_paint(Paint::Secondary);
            }
            let plate = ui.add(
                card,
                UiWidget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(12.0),
                Style {
                    size: Size { width: percent(1.0), height: length(38.0) },
                    padding: pad(12.0, 12.0, 0.0, 0.0),
                    flex_shrink: 0.0,
                    ..row(0.0)
                },
            );
            ui.add_text_field(
                plate,
                placeholder,
                TextStyle::Body,
                Style {
                    flex_grow: 1.0,
                    size: Size { width: auto(), height: length(22.0) },
                    ..Default::default()
                },
            )
        };
        let name = field(ui, tr("Name"), "");
        let url = field(ui, tr("Address"), "example.com");
        let buttons =
            ui.add(card, UiWidget::container(), Style { justify_content: JustifyContent::END, ..row(24.0) });
        let pill = |ui: &mut Ui, action: StartAction, text: &str, primary: bool| {
            let background = if primary {
                Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
            } else {
                Background::Glass(GlassLevel::Thin)
            };
            let node = ui.add(
                buttons,
                UiWidget::container()
                    .with_action(action.id())
                    .with_radius(CAPSULE)
                    .with_background(background),
                Style {
                    size: Size { width: auto(), height: length(36.0) },
                    padding: pad(18.0, 18.0, 0.0, 0.0),
                    ..row(0.0)
                },
            );
            let l = ui.add_label(node, text, TextStyle::Callout, Style::default());
            if primary && let Some(w) = ui.widget_mut(l) {
                w.set_paint(Paint::OnAccent);
            }
        };
        pill(ui, StartAction::EditorCancel, tr("Cancel"), false);
        pill(ui, StartAction::EditorSave, tr("Save"), true);
        ui.set_visible(backdrop, false);
        Editor {
            backdrop,
            title,
            name,
            url,
            shown: Animated::new(0.0, Spring::new(POP_SPRING.0, POP_SPRING.1)),
            open: false,
            old: String::new(),
        }
    }

    fn apply_springs(&mut self, config: &Config) {
        self.offset.spring = crate::drip::spring(
            config,
            axion_config::SpringSpec { response: PAGE_SPRING.0, damping: PAGE_SPRING.1 },
        );
        self.editor.shown.spring = crate::drip::spring(
            config,
            axion_config::SpringSpec { response: POP_SPRING.0, damping: POP_SPRING.1 },
        );
    }

    // ---- accessors ----

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The page's two planes, in drawing order.
    pub fn planes(&self) -> [NodeId; 2] {
        [self.root, self.overlay]
    }

    pub fn query_field(&self) -> NodeId {
        self.query
    }

    pub fn path_field(&self) -> NodeId {
        self.panel.path_field
    }

    pub fn editor_fields(&self) -> (NodeId, NodeId) {
        (self.editor.name, self.editor.url)
    }

    pub fn menu(&mut self) -> &mut Menu {
        &mut self.menu
    }

    pub fn panel_open(&self) -> bool {
        self.panel.popover.is_open()
    }

    pub fn panel_contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.panel.popover.contains(ui, p)
    }

    pub fn gear_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.gear)
    }

    pub fn editor_open(&self) -> bool {
        self.editor.open
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn current_page(&self) -> usize {
        self.page
    }

    pub fn model(&self) -> Option<&StartModel> {
        self.model.as_ref()
    }

    /// The tile of the site at `p` (not the Add Site tile).
    pub fn tile_at(&self, ui: &Ui, p: Vec2) -> Option<usize> {
        if !self.open {
            return None;
        }
        let action = ui.hit_action(p).and_then(|n| ui.widget(n)).and_then(|w| w.action)?;
        match StartAction::from_id(action)? {
            StartAction::Tile(i)
            | StartAction::TilePin(i)
            | StartAction::TileEdit(i)
            | StartAction::TileRemove(i)
                if i < self.model.as_ref().map_or(0, |m| m.sites.sites.len()) =>
            {
                Some(i)
            }
            _ => None,
        }
    }

    /// The site of tile `i`.
    pub fn site(&self, i: usize) -> Option<&Site> {
        self.model.as_ref().and_then(|m| m.sites.sites.get(i))
    }

    // ---- showing, placing ----

    pub fn open(&mut self, ui: &mut Ui, config: &Config, rect: Frame) {
        self.open = true;
        self.place(ui, config, rect);
        ui.set_visible(self.root, true);
        ui.set_visible(self.overlay, true);
        self.apply_springs(config);
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.open = false;
        self.focus = Focus::None;
        self.close_panel(ui, config);
        self.menu.close(ui, config);
        self.close_editor(ui);
        ui.set_visible(self.root, false);
        ui.set_visible(self.overlay, false);
        // A half-typed query stays with the page, but the focus goes.
        if ui.focused().is_some_and(|n| n == self.query) {
            let _ = ui.blur();
        }
    }

    /// Puts the page over `rect` (logical px) and sizes what depends on its width.
    pub fn place(&mut self, ui: &mut Ui, config: &Config, rect: Frame) {
        let changed = rect != self.rect;
        self.rect = rect;
        let mut style = ui.style(self.root);
        style.inset =
            Rect { left: length(rect.origin.x), right: auto(), top: length(rect.origin.y), bottom: auto() };
        style.size = Size { width: length(rect.size.x), height: length(rect.size.y) };
        ui.set_style(self.root, style.clone());
        ui.set_style(self.overlay, style);
        if let Some(w) = ui.widget_mut(self.root) {
            w.radius = config.effective_layout().page_corner_radius;
        }
        if !changed {
            return;
        }
        let inner = (rect.size.x - 2.0 * SIDE_MARGIN).max(1.0);
        let mut main = ui.style(self.main);
        // A share of the window's height (the page's bottom edge, about), like the HTML page's `9vh`.
        main.padding.top = length(((rect.origin.y + rect.size.y) * TOP_SHARE).max(TOP_MIN));
        ui.set_style(self.main, main);
        let mut search = ui.style(self.search);
        search.size.width = length(SEARCH_WIDTH.min(inner));
        ui.set_style(self.search, search);
        self.sync_clock(ui);
        self.sig.clear();
        self.sync_sites(ui, None);
    }

    /// The area the top sites use.
    fn sites_width(&self) -> f32 {
        SITES_WIDTH.min((self.rect.size.x - 2.0 * SIDE_MARGIN).max(1.0))
    }

    /// Whether the page draws over a wallpaper: its ground is transparent (the wallpaper layer
    /// lies under the plane) and its text turns light.
    pub fn set_wallpaper(&mut self, ui: &mut Ui, wallpaper: bool) {
        if self.over_image == wallpaper {
            return;
        }
        self.over_image = wallpaper;
        let alpha = if wallpaper { 0.0 } else { 1.0 };
        if let Some(w) = ui.widget_mut(self.root) {
            w.background = Background::Fill(Paint::Ground, alpha);
        }
        self.repaint_labels(ui);
    }

    /// The page's ground is the dark one (private tabs).
    pub fn set_private(&mut self, ui: &mut Ui, private: bool) {
        self.private = private;
        ui.set_plane_dark(self.root, private);
    }

    fn repaint_labels(&self, ui: &mut Ui) {
        for (node, ink) in self.labels.iter().chain(&self.tile_labels) {
            if *node == self.gear {
                continue;
            }
            if let Some(w) = ui.widget_mut(*node) {
                w.set_paint(paint_of(*ink, self.over_image));
            }
        }
        // The gear's glyph.
        if let Some(glyph) = ui.children(self.gear).first().copied()
            && let Some(w) = ui.widget_mut(glyph)
        {
            w.set_paint(paint_of(Ink::Accent, self.over_image));
        }
    }

    // ---- the model ----

    /// Shows `model`: the sections, the texts, the tiles. Cheap when nothing changed.
    pub fn set_model(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        model: StartModel,
        favicons: &Favicons,
        icons: u64,
    ) {
        let flat = config.glass.is_flat();
        let tag = axion_i18n::language().tag().to_owned();
        let sig = format!("{}|{}|{}|{flat}|{tag}", model.sites.sites.len(), icons, model.sites.columns);
        let changed = self.model.as_ref() != Some(&model) || self.flat != flat || self.tag != tag;
        if !changed && sig == self.sig {
            return;
        }
        self.flat = flat;
        self.tag = tag;
        self.private = model.private;
        ui.set_visible(self.clock_block, model.clock_show || model.greeting_show);
        self.model = Some(model.clone());
        self.sync_clock(ui);
        ui.set_visible(self.greeting, model.greeting_show);
        ui.set_visible(self.private_card, model.private);
        ui.set_visible(self.search, model.search.show);
        ui.set_visible(self.gear, !model.private);
        let name = model.search.engine_name().unwrap_or_else(|| tr("your search engine").to_owned());
        ui.set_placeholder(self.query, &tr_fmt!("Search with {engine} or enter website", engine = name));
        self.rebuild_engine_icon(ui, &model, favicons);
        ui.set_visible(self.stats, model.stats.show);
        let tag = self.tag.clone();
        ui.set_text(self.stat_values[0], &home::format_count(model.stats.blocked, &tag));
        ui.set_text(
            self.stat_values[1],
            &tr_fmt!("≈ {value}", value = home::format_bytes(model.stats.bytes, &tag)),
        );
        ui.set_text(
            self.stat_values[2],
            &tr_fmt!("≈ {value}", value = home::format_seconds(model.stats.seconds, &tag)),
        );
        self.model = Some(model);
        self.sig = sig;
        self.sync_sites(ui, Some(favicons));
        self.fill_panel_if_changed(ui, config);
    }

    /// Shows the clock row of the right size (the HTML page's `clamp(56px, 9vw, 96px)`) while
    /// the clock is on, with or without the AM/PM mark.
    fn sync_clock(&mut self, ui: &mut Ui) {
        let wanted = (self.rect.size.x * 0.09).clamp(56.0, 96.0);
        let class = if wanted >= 86.0 {
            0
        } else if wanted >= 66.0 {
            1
        } else {
            2
        };
        let (show, h24) = self.model.as_ref().map_or((false, true), |m| (m.clock_show, m.clock_24h));
        for (i, (r, _, ampm)) in self.clock_rows.iter().enumerate() {
            ui.set_visible(*r, show && i == class);
            ui.set_visible(*ampm, !h24);
        }
    }

    fn rebuild_engine_icon(&mut self, ui: &mut Ui, model: &StartModel, favicons: &Favicons) {
        ui.clear_children(self.engine_icon);
        let host = model.search.icon_host().to_owned();
        match (!host.is_empty()).then(|| favicons.get(&format!("https://{host}/"))).flatten() {
            Some(image) => {
                let node = ui.add(
                    self.engine_icon,
                    UiWidget::image().with_radius(6.0),
                    Style { size: square(22.0), ..Default::default() },
                );
                ui.set_image(node, Some(image));
            }
            None => {
                let letter = home::monogram(&host);
                let plate = ui.add(
                    self.engine_icon,
                    UiWidget::container()
                        .with_background(Background::Fill(Paint::Fixed(home::monogram_color(&host)), 1.0))
                        .with_radius(6.0),
                    Style { size: square(22.0), justify_content: JustifyContent::CENTER, ..row(0.0) },
                );
                let l = ui.add_label(plate, &letter, TextStyle::Caption, Style::default());
                if let Some(w) = ui.widget_mut(l) {
                    w.set_paint(Paint::Fixed(WHITE));
                }
            }
        }
    }

    /// Updates the clock and the greeting (call when the minute or the hour changes).
    pub fn set_time(&mut self, ui: &mut Ui, hour: u32, minute: u32) {
        let Some(model) = &self.model else { return };
        let (digits, mark) = home::clock_text(hour, minute, model.clock_24h);
        for (_, time, ampm) in &self.clock_rows {
            ui.set_text(*time, &digits);
            ui.set_text(*ampm, mark.unwrap_or(""));
        }
        let greeting = home::greeting_for_hour(hour, &model.greeting_name);
        ui.set_text(self.greeting, &greeting);
    }

    // ---- top sites ----

    fn per_page(&self) -> usize {
        let m = self.model.as_ref();
        let rows = m.map_or(1, |m| m.sites.rows) as usize;
        self.columns as usize * rows
    }

    /// Rebuilds the tiles and the dots (the sites, the width or the icons changed).
    fn sync_sites(&mut self, ui: &mut Ui, favicons: Option<&Favicons>) {
        let Some(model) = self.model.clone() else { return };
        ui.set_visible(self.sites, model.sites.show);
        ui.clear_children(self.track);
        ui.clear_children(self.dots);
        self.tiles.clear();
        self.tile_labels.clear();
        self.add_tile = None;
        if !model.sites.show {
            return;
        }
        let width = self.sites_width();
        self.columns = home::columns_for(width, COLUMN_ESTIMATE, model.sites.columns);
        let sites = &model.sites.sites;
        let with_add = sites.len() < model.sites.count as usize;
        let items = sites.len() + usize::from(with_add);
        let per_page = self.per_page().max(1);
        self.pages = home::page_count(items, per_page);
        self.page = self.page.min(self.pages - 1);
        // A single page with fewer tiles than columns is as wide as its tiles (centred).
        let columns = if self.pages == 1 { (items as u32).clamp(1, self.columns) } else { self.columns };
        let tile_w = TILE_WIDTH.min((width - (columns as f32 - 1.0) * TILE_GAP.x) / columns as f32).max(40.0);
        let grid_w = columns as f32 * tile_w + (columns as f32 - 1.0) * TILE_GAP.x;
        let rows_first = items.min(per_page).div_ceil(columns as usize).max(1);
        let height = rows_first as f32 * TILE_HEIGHT + (rows_first as f32 - 1.0) * TILE_GAP.y;
        let mut vp = ui.style(self.viewport);
        vp.size = Size { width: length(width), height: length(height) };
        ui.set_style(self.viewport, vp);

        let icons = favicons;
        for chunk_start in (0..items).step_by(per_page) {
            let slot = ui.add(
                self.track,
                UiWidget::container(),
                Style {
                    size: Size { width: length(width), height: auto() },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..Default::default()
                },
            );
            let grid = ui.add(
                slot,
                UiWidget::container(),
                Style {
                    size: Size { width: length(grid_w), height: auto() },
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    align_content: AlignContent::FLEX_START,
                    gap: Size { width: length(TILE_GAP.x), height: length(TILE_GAP.y) },
                    ..Default::default()
                },
            );
            for i in chunk_start..(chunk_start + per_page).min(items) {
                let tile = match sites.get(i) {
                    Some(site) => self.build_tile(ui, grid, i, site, tile_w, icons),
                    None => self.build_add_tile(ui, grid, tile_w),
                };
                if i < sites.len() {
                    self.tiles.push(tile);
                } else {
                    self.add_tile = Some(tile);
                }
            }
        }
        if self.pages > 1 {
            for p in 0..self.pages {
                let selected = p == self.page;
                let hit = ui.add(
                    self.dots,
                    UiWidget::container()
                        .with_action(StartAction::Dot(p).id())
                        .with_radius(CAPSULE)
                        .with_tooltip(&tr_fmt!("Page {n}", n = p + 1), None),
                    Style {
                        size: Size {
                            width: length(if selected { 28.0 } else { 16.0 }),
                            height: length(20.0),
                        },
                        justify_content: JustifyContent::CENTER,
                        ..row(0.0)
                    },
                );
                let paint = if selected { Paint::Accent } else { Paint::Secondary };
                ui.add(
                    hit,
                    UiWidget::container()
                        .with_background(Background::Fill(paint, if selected { 1.0 } else { 0.3 }))
                        .with_radius(CAPSULE),
                    Style {
                        size: Size { width: length(if selected { 20.0 } else { 8.0 }), height: length(8.0) },
                        ..Default::default()
                    },
                );
            }
        }
        self.offset.snap(-(self.page as f32) * width);
        self.apply_offset(ui);
        self.apply_focus(ui);
    }

    fn build_tile(
        &mut self,
        ui: &mut Ui,
        parent: NodeId,
        i: usize,
        site: &Site,
        width: f32,
        favicons: Option<&Favicons>,
    ) -> TileNodes {
        let root = ui.add(
            parent,
            UiWidget::container(),
            Style {
                position: Position::Relative,
                size: Size { width: length(width), height: length(TILE_HEIGHT) },
                flex_shrink: 0.0,
                align_items: AlignItems::CENTER,
                padding: pad(2.0, 2.0, 6.0, 6.0),
                ..column(8.0)
            },
        );
        let face = ui.add(
            root,
            UiWidget::container()
                .with_action(StartAction::Tile(i).id())
                .with_radius(FACE_RADIUS)
                .with_background(Background::Glass(GlassLevel::Regular))
                .with_tooltip(&tile_tip(site), None),
            Style {
                size: square(FACE),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        match favicons.and_then(|f| f.get(&site.url)) {
            Some(image) => {
                let node = ui.add(
                    face,
                    UiWidget::image().with_radius(ICON_RADIUS),
                    Style { size: square(ICON), flex_shrink: 0.0, ..Default::default() },
                );
                ui.set_image(node, Some(image));
            }
            None => {
                let plate = ui.add(
                    face,
                    UiWidget::container()
                        .with_background(Background::Fill(
                            Paint::Fixed(home::monogram_color(&site.host)),
                            1.0,
                        ))
                        .with_radius(ICON_RADIUS),
                    Style { size: square(ICON), justify_content: JustifyContent::CENTER, ..row(0.0) },
                );
                let l = ui.add_label(plate, &home::monogram(&site.host), TextStyle::Title2, Style::default());
                if let Some(w) = ui.widget_mut(l) {
                    w.set_paint(Paint::Fixed(WHITE));
                }
            }
        }
        let name_box = ui.add(
            root,
            UiWidget::container().with_action(StartAction::Tile(i).id()).without_hover_fill(),
            Style {
                size: Size { width: percent(1.0), height: length(20.0) },
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        let name = if site.title.is_empty() {
            site.host.trim_start_matches("www.").to_owned()
        } else {
            site.title.clone()
        };
        let label = ui.add_label(name_box, &name, TextStyle::Footnote, shrinking());
        ui.set_label_align(label, Align::Center);
        self.tile_labels.push((label, Ink::Primary));
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(paint_of(Ink::Primary, self.over_image));
        }
        if site.pinned {
            ui.add(
                root,
                UiWidget::container()
                    .with_background(Background::Fill(Paint::Accent, 1.0))
                    .with_radius(CAPSULE),
                Style {
                    position: Position::Absolute,
                    size: square(8.0),
                    inset: Rect {
                        left: length(width / 2.0 - 40.0),
                        right: auto(),
                        top: length(4.0),
                        bottom: auto(),
                    },
                    ..Default::default()
                },
            );
        }
        let actions = ui.add(
            root,
            UiWidget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect {
                    left: auto(),
                    right: length(width / 2.0 - 50.0),
                    top: length(-2.0),
                    bottom: auto(),
                },
                ..row(4.0)
            },
        );
        for (action, icon, tip) in [
            (StartAction::TilePin(i), Icon::Pin, if site.pinned { tr("Unpin") } else { tr("Pin") }),
            (StartAction::TileEdit(i), Icon::Pencil, tr("Edit")),
            (StartAction::TileRemove(i), Icon::Close, tr("Remove")),
        ] {
            let b = ui.add(
                actions,
                UiWidget::container()
                    .with_action(action.id())
                    .with_radius(CAPSULE)
                    .with_background(Background::Glass(GlassLevel::Thin))
                    .with_isolated()
                    .with_tooltip(tip, None),
                Style { size: square(MINI), justify_content: JustifyContent::CENTER, ..row(0.0) },
            );
            ui.add(b, UiWidget::icon(icon, 12.0).with_icon_weight(IconWeight::Bold), Style::default());
        }
        ui.set_visible(actions, false);
        let mut hover = Animated::new(0.0, Spring::new(POP_SPRING.0, POP_SPRING.1));
        hover.spring = self.offset.spring;
        TileNodes { root, face, actions: Some(actions), hover }
    }

    fn build_add_tile(&mut self, ui: &mut Ui, parent: NodeId, width: f32) -> TileNodes {
        let root = ui.add(
            parent,
            UiWidget::container(),
            Style {
                size: Size { width: length(width), height: length(TILE_HEIGHT) },
                flex_shrink: 0.0,
                align_items: AlignItems::CENTER,
                padding: pad(2.0, 2.0, 6.0, 6.0),
                ..column(8.0)
            },
        );
        let face = ui.add(
            root,
            UiWidget::container()
                .with_action(StartAction::AddSite.id())
                .with_radius(FACE_RADIUS)
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_tooltip(tr("Add Site"), None),
            Style {
                size: square(FACE),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        ui.add(face, UiWidget::icon(Icon::Plus, 26.0).with_paint(Paint::Secondary), Style::default());
        let name_box = ui.add(
            root,
            UiWidget::container().with_action(StartAction::AddSite.id()).without_hover_fill(),
            Style {
                size: Size { width: percent(1.0), height: length(20.0) },
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        let label = ui.add_label(name_box, tr("Add Site"), TextStyle::Footnote, shrinking());
        ui.set_label_align(label, Align::Center);
        self.tile_labels.push((label, Ink::Secondary));
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(paint_of(Ink::Secondary, self.over_image));
        }
        TileNodes { root, face, actions: None, hover: Animated::new(0.0, self.offset.spring) }
    }

    // ---- paging ----

    pub fn set_page(&mut self, ui: &mut Ui, page: usize) {
        let page = page.min(self.pages.saturating_sub(1));
        if page == self.page {
            return;
        }
        self.page = page;
        self.offset.target = -(page as f32) * self.sites_width();
        self.sig.clear();
        // The dots follow at once; the tiles glide.
        let width = self.sites_width();
        let current = self.offset.value;
        self.rebuild_dots(ui);
        self.offset.value = current;
        self.offset.target = -(page as f32) * width;
    }

    fn rebuild_dots(&mut self, ui: &mut Ui) {
        for (i, hit) in ui.children(self.dots).into_iter().enumerate() {
            let selected = i == self.page;
            let mut s = ui.style(hit);
            s.size.width = length(if selected { 28.0 } else { 16.0 });
            ui.set_style(hit, s);
            if let Some(dot) = ui.children(hit).first().copied() {
                let mut s = ui.style(dot);
                s.size.width = length(if selected { 20.0 } else { 8.0 });
                ui.set_style(dot, s);
                if let Some(w) = ui.widget_mut(dot) {
                    w.background = Background::Fill(
                        if selected { Paint::Accent } else { Paint::Secondary },
                        if selected { 1.0 } else { 0.3 },
                    );
                }
            }
        }
    }

    /// A horizontal touchpad swipe over the sites: flips a page after the configured travel.
    pub fn swipe(&mut self, ui: &mut Ui, config: &Config, dx: f32) -> bool {
        if self.pages < 2 {
            return false;
        }
        let now = std::time::Instant::now();
        if self.swipe_locked_until.is_some_and(|t| now < t) {
            return true;
        }
        self.swipe += dx;
        if self.swipe.abs() > config.home.top_sites_swipe_px {
            let forward = self.swipe < 0.0;
            self.swipe = 0.0;
            self.swipe_locked_until =
                Some(now + Duration::from_millis(u64::from(config.home.top_sites_swipe_lock_ms)));
            let next = if forward { self.page + 1 } else { self.page.saturating_sub(1) };
            self.set_page(ui, next);
        }
        true
    }

    /// Whether `p` is over the sites area (where swipes flip pages).
    pub fn sites_contain(&self, ui: &Ui, p: Vec2) -> bool {
        self.open && ui.is_visible(self.sites) && ui.frame(self.viewport).contains(p)
    }

    fn apply_offset(&mut self, ui: &mut Ui) {
        let mut style = ui.style(self.track);
        style.margin = Rect {
            left: length(self.offset.value),
            right: length(0.0),
            top: length(0.0),
            bottom: length(0.0),
        };
        ui.set_style(self.track, style);
    }

    // ---- keyboard focus ----

    fn focus_order(&self) -> Vec<Focus> {
        let mut order = Vec::new();
        let Some(model) = &self.model else { return order };
        if !model.private {
            order.push(Focus::Gear);
        }
        if model.search.show {
            order.push(Focus::Engine);
            order.push(Focus::Query);
        }
        if model.sites.show {
            let n = self.tiles.len() + usize::from(self.add_tile.is_some());
            order.extend((0..n).map(Focus::Tile));
        }
        order
    }

    /// Tab (or Shift+Tab): the next control. Returns the new focus.
    pub fn move_focus(&mut self, ui: &mut Ui, forward: bool) -> Focus {
        let order = self.focus_order();
        if order.is_empty() {
            return Focus::None;
        }
        let at = order.iter().position(|f| *f == self.focus);
        let next = match (at, forward) {
            (None, true) => Some(0),
            (None, false) => Some(order.len() - 1),
            (Some(i), true) => (i + 1 < order.len()).then_some(i + 1),
            (Some(i), false) => i.checked_sub(1),
        };
        // Past the ends the focus leaves the page (the browser's chrome takes it).
        self.set_focus(ui, next.map_or(Focus::None, |i| order[i]));
        self.focus
    }

    pub fn set_focus(&mut self, ui: &mut Ui, focus: Focus) {
        self.focus = focus;
        if focus == Focus::Query {
            let _ = ui.focus(self.query);
        } else if ui.focused() == Some(self.query) {
            let _ = ui.blur();
        }
        if let Focus::Tile(i) = focus {
            self.set_page(ui, i / self.per_page().max(1));
        }
        self.apply_focus(ui);
    }

    /// Arrow keys between the tiles.
    pub fn move_tile_focus(&mut self, ui: &mut Ui, dx: i32, dy: i32) {
        let Focus::Tile(i) = self.focus else { return };
        let n = self.tiles.len() + usize::from(self.add_tile.is_some());
        let columns = self.columns as i32;
        let next = (i as i32 + dx + dy * columns).clamp(0, n as i32 - 1) as usize;
        self.set_focus(ui, Focus::Tile(next));
    }

    /// What Enter or Space on the focused control does.
    pub fn activate_focus(&self) -> Option<StartAction> {
        match self.focus {
            Focus::Gear => Some(StartAction::Gear),
            Focus::Engine => Some(StartAction::Engine),
            Focus::Tile(i) if i < self.tiles.len() => Some(StartAction::Tile(i)),
            Focus::Tile(_) => Some(StartAction::AddSite),
            Focus::Query | Focus::None => None,
        }
    }

    fn apply_focus(&mut self, ui: &mut Ui) {
        let tinted = |on: bool, level: GlassLevel| {
            if on { Background::TintedGlass(level, Paint::Accent) } else { Background::Glass(level) }
        };
        if let Some(w) = ui.widget_mut(self.gear) {
            w.background =
                tinted(self.focus == Focus::Gear || self.panel.popover.is_open(), GlassLevel::Thin);
        }
        if let Some(w) = ui.widget_mut(self.engine) {
            w.selected = self.focus == Focus::Engine;
        }
        let count = self.tiles.len();
        for (i, tile) in self.tiles.iter().chain(self.add_tile.iter()).enumerate() {
            let level = if i < count { GlassLevel::Regular } else { GlassLevel::Thin };
            if let Some(w) = ui.widget_mut(tile.face) {
                w.background = tinted(self.focus == Focus::Tile(i), level);
            }
        }
    }

    // ---- editor ----

    /// Opens the Add Site (`None`) or Edit Site dialog.
    pub fn open_editor(&mut self, ui: &mut Ui, site: Option<&Site>) {
        ui.set_text(self.editor.title, if site.is_some() { tr("Edit Site") } else { tr("Add Site") });
        ui.set_field_text(self.editor.name, site.map_or("", |s| s.title.as_str()));
        ui.set_field_text(self.editor.url, site.map_or("", |s| s.url.as_str()));
        self.editor.old = site.map(|s| s.url.clone()).unwrap_or_default();
        self.editor.open = true;
        ui.set_visible(self.editor.backdrop, true);
        ui.set_opacity(self.editor.backdrop, self.editor.shown.value);
        let _ = ui.focus(if site.is_some() { self.editor.name } else { self.editor.url });
    }

    pub fn close_editor(&mut self, ui: &mut Ui) {
        if self.editor.open {
            self.editor.open = false;
            if ui.focused().is_some_and(|n| n == self.editor.name || n == self.editor.url) {
                let _ = ui.blur();
            }
        }
    }

    /// What the dialog holds: the site's old address, the name and the address typed.
    pub fn editor_values(&self, ui: &Ui) -> (String, String, String) {
        let text = |n| ui.field_text(n).unwrap_or_default().trim().to_owned();
        (self.editor.old.clone(), text(self.editor.name), text(self.editor.url))
    }

    // ---- the gear's panel ----

    pub fn open_panel(&mut self, ui: &mut Ui, config: &Config) {
        if self.model.as_ref().is_none_or(|m| m.private) {
            return;
        }
        self.panel.signature.clear();
        self.fill_panel(ui, config);
        let anchor = ui.frame(self.gear);
        let w = PANEL_WIDTH.min((self.rect.size.x - 2.0 * GEAR_MARGIN).max(120.0));
        let max_h = (self.rect.size.y - GEAR + 2.0 * GEAR_MARGIN - 18.0).max(120.0);
        let target = Frame::new(
            self.rect.origin.x + self.rect.size.x - GEAR_MARGIN - w,
            anchor.origin.y + anchor.size.y + 8.0,
            w,
            self.panel.height.min(max_h),
        );
        self.panel.popover.open(ui, config, anchor, target);
        self.apply_focus(ui);
    }

    pub fn close_panel(&mut self, ui: &mut Ui, config: &Config) {
        if self.panel.popover.is_open() {
            self.panel.popover.close(ui, config);
            if ui.focused() == Some(self.panel.path_field) {
                let _ = ui.blur();
            }
            self.apply_focus(ui);
        }
    }

    pub fn toggle_panel(&mut self, ui: &mut Ui, config: &Config) {
        if self.panel.popover.is_open() {
            self.close_panel(ui, config);
        } else {
            self.open_panel(ui, config);
        }
    }

    /// The structure of the panel (everything but the values of its controls).
    fn panel_signature(model: &StartModel) -> String {
        let names: Vec<&str> = model.background.list.iter().map(|e| e.name.as_str()).collect();
        format!(
            "{}|{}|{}|{:?}|{}|{}|{:?}|{}|{}",
            model.clock_show,
            model.sites.hidden,
            model.background.can_add,
            model.background.selected.as_ref().map(|e| (&e.name, e.kind)),
            names.join(","),
            model.background.error,
            model.background.live_setting,
            model.background.list.len(),
            model.clock_format as u8,
        )
    }

    fn fill_panel_if_changed(&mut self, ui: &mut Ui, config: &Config) {
        if !self.panel.popover.is_open() {
            return;
        }
        let Some(model) = &self.model else { return };
        if Self::panel_signature(model) != self.panel.signature {
            self.fill_panel(ui, config);
            let mut target = self.panel.popover.target();
            target.size.y = self.panel.height.min(target.size.y.max(self.panel.height));
            self.panel.popover.retarget(ui, target);
        } else {
            self.sync_controls(ui);
        }
    }

    /// Puts the model's values into the panel's switches and sliders (not the one being dragged).
    fn sync_controls(&mut self, ui: &mut Ui) {
        let Some(model) = &self.model else { return };
        let dragging = ui.dragging();
        for (node, control) in &self.panel.controls {
            if dragging == Some(*node) {
                continue;
            }
            let value = match control {
                Control::Toggle(home::Widget::Clock) => f32::from(u8::from(model.clock_show)),
                Control::Toggle(home::Widget::Greeting) => f32::from(u8::from(model.greeting_show)),
                Control::Toggle(home::Widget::Search) => f32::from(u8::from(model.search.show)),
                Control::Toggle(home::Widget::TopSites) => f32::from(u8::from(model.sites.show)),
                Control::Toggle(home::Widget::Stats) => f32::from(u8::from(model.stats.show)),
                Control::Toggle(home::Widget::LiveWallpapers) => {
                    f32::from(u8::from(model.background.live_setting))
                }
                Control::Dim => model.background.dim / DIM_MAX,
                Control::Blur => model.background.blur / BLUR_MAX,
            };
            ui.set_value(*node, value);
        }
    }

    fn fill_panel(&mut self, ui: &mut Ui, _config: &Config) {
        let Some(model) = self.model.clone() else { return };
        self.panel.signature = Self::panel_signature(&model);
        let plate = self.panel.popover.root();
        ui.clear_children(plate);
        self.panel.controls.clear();
        self.thumb_nodes.clear();
        self.panel.note = None;
        let mut height = 14.0 + 12.0;
        let secondary = |ui: &mut Ui, node: NodeId| {
            if let Some(w) = ui.widget_mut(node) {
                w.set_paint(Paint::Secondary);
            }
        };

        let header = ui.add(
            plate,
            UiWidget::container(),
            Style { size: Size { width: auto(), height: length(28.0) }, flex_shrink: 0.0, ..row(0.0) },
        );
        let l = ui.add_label(header, tr("Start Page"), TextStyle::Caption, Style::default());
        secondary(ui, l);
        height += 28.0;

        let switch = |ui: &mut Ui,
                      controls: &mut Vec<(NodeId, Control)>,
                      text: &str,
                      widget: home::Widget,
                      on: bool,
                      indent: bool| {
            let r = ui.add(
                plate,
                UiWidget::container(),
                Style {
                    size: Size { width: auto(), height: length(PANEL_ROW) },
                    flex_shrink: 0.0,
                    padding: pad(if indent { 14.0 } else { 0.0 }, 0.0, 0.0, 0.0),
                    ..row(10.0)
                },
            );
            let l = ui.add_label(r, text, TextStyle::Callout, Style { flex_grow: 1.0, ..shrinking() });
            if indent && let Some(w) = ui.widget_mut(l) {
                w.set_paint(Paint::Secondary);
            }
            let t = ui.add(r, UiWidget::toggle(on), Style::default());
            controls.push((t, Control::Toggle(widget)));
        };
        switch(ui, &mut self.panel.controls, tr("Clock"), home::Widget::Clock, model.clock_show, false);
        height += PANEL_ROW;
        if model.clock_show {
            let r = ui.add(
                plate,
                UiWidget::container(),
                Style {
                    size: Size { width: auto(), height: length(PANEL_ROW) },
                    flex_shrink: 0.0,
                    padding: pad(14.0, 0.0, 0.0, 0.0),
                    ..row(10.0)
                },
            );
            let options = [
                (tr("Auto"), StartAction::Format(ClockFormat::Auto).id()),
                (tr("12 h"), StartAction::Format(ClockFormat::H12).id()),
                (tr("24 h"), StartAction::Format(ClockFormat::H24).id()),
            ];
            let current = match model.clock_format {
                ClockFormat::Auto => 0,
                ClockFormat::H12 => 1,
                ClockFormat::H24 => 2,
            };
            crate::settings::segmented(ui, r, &options, Some(current));
            height += PANEL_ROW;
        }
        switch(
            ui,
            &mut self.panel.controls,
            tr("Greeting"),
            home::Widget::Greeting,
            model.greeting_show,
            false,
        );
        switch(
            ui,
            &mut self.panel.controls,
            tr("Search field"),
            home::Widget::Search,
            model.search.show,
            false,
        );
        switch(
            ui,
            &mut self.panel.controls,
            tr("Top sites"),
            home::Widget::TopSites,
            model.sites.show,
            false,
        );
        height += 3.0 * PANEL_ROW;
        if model.sites.hidden > 0 {
            let r = ui.add(
                plate,
                UiWidget::container(),
                Style {
                    size: Size { width: auto(), height: length(PANEL_ROW) },
                    flex_shrink: 0.0,
                    padding: pad(14.0, 0.0, 0.0, 0.0),
                    ..row(10.0)
                },
            );
            let text = tr_fmt!("{n} removed sites", n = model.sites.hidden);
            let l = ui.add_label(r, &text, TextStyle::Callout, Style { flex_grow: 1.0, ..shrinking() });
            secondary(ui, l);
            let link = ui.add(
                r,
                UiWidget::container().with_action(StartAction::Restore.id()).with_radius(CAPSULE),
                Style { padding: pad(10.0, 10.0, 6.0, 6.0), ..row(0.0) },
            );
            let l = ui.add_label(link, tr("Restore"), TextStyle::Callout, Style::default());
            if let Some(w) = ui.widget_mut(l) {
                w.set_paint(Paint::Accent);
            }
            height += PANEL_ROW;
        }
        switch(
            ui,
            &mut self.panel.controls,
            tr("Shields statistics"),
            home::Widget::Stats,
            model.stats.show,
            false,
        );
        height += PANEL_ROW;

        let r = ui.add(
            plate,
            UiWidget::container(),
            Style { size: Size { width: auto(), height: length(PANEL_ROW) }, flex_shrink: 0.0, ..row(10.0) },
        );
        ui.add_label(r, tr("Background"), TextStyle::Callout, shrinking());
        height += PANEL_ROW;

        // The picker: none, one swatch per background, add.
        let picker = ui.add(
            plate,
            UiWidget::container(),
            Style {
                size: Size { width: percent(1.0), height: auto() },
                flex_wrap: FlexWrap::Wrap,
                flex_direction: FlexDirection::Row,
                gap: Size { width: length(SWATCH_GAP), height: length(SWATCH_GAP) },
                padding: pad(2.0, 2.0, 2.0, 2.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        let selected = model.background.selected.as_ref().map(|e| e.name.clone());
        let swatch = |ui: &mut Ui, action: StartAction, on: bool, tip: &str| {
            let ring = ui.add(
                picker,
                UiWidget::container()
                    .with_action(action.id())
                    .with_radius(12.0)
                    .with_background(if on { Background::Fill(Paint::Accent, 1.0) } else { Background::None })
                    .with_tooltip(tip, None),
                Style {
                    size: Size { width: length(SWATCH.x), height: length(SWATCH.y) },
                    padding: uniform(2.0),
                    flex_shrink: 0.0,
                    ..row(0.0)
                },
            );
            let inner = ui.add(
                ring,
                UiWidget::container()
                    .with_background(Background::Fill(Paint::Label, 0.08))
                    .with_radius(10.0)
                    .with_clip(),
                Style {
                    size: Size { width: percent(1.0), height: percent(1.0) },
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            (ring, inner)
        };
        let (_, none) = swatch(ui, StartAction::BackgroundNone, selected.is_none(), tr("No background"));
        ui.add(none, UiWidget::icon(Icon::EyeOff, 16.0).with_paint(Paint::Secondary), Style::default());
        let mut rows = 1;
        for (i, entry) in model.background.list.iter().take(MAX_PICKER).enumerate() {
            let on = selected.as_deref() == Some(entry.name.as_str());
            let tip = match entry.kind {
                Kind::Image => tr_fmt!("Background {n}", n = i + 1),
                _ => tr_fmt!("{label} (animated)", label = tr_fmt!("Background {n}", n = i + 1)),
            };
            let (ring, inner) = swatch(ui, StartAction::Background(i), on, &tip);
            let thumb = ui.add(
                inner,
                UiWidget::image(),
                Style { size: Size { width: percent(1.0), height: percent(1.0) }, ..Default::default() },
            );
            ui.set_visible(thumb, false);
            // The picture arrives with `sync_thumbnails`; until then the plate shows.
            self.thumb_nodes.push((entry.name.clone(), thumb, false));
            let x = ui.add(
                ring,
                UiWidget::container()
                    .with_action(StartAction::BackgroundRemove(i).id())
                    .with_radius(CAPSULE)
                    .with_background(Background::Glass(GlassLevel::Thin))
                    .with_isolated()
                    .with_tooltip(tr("Remove background"), None),
                Style {
                    position: Position::Absolute,
                    size: square(16.0),
                    inset: Rect { left: auto(), right: length(2.0), top: length(2.0), bottom: auto() },
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            ui.add(x, UiWidget::icon(Icon::Close, 9.0).with_icon_weight(IconWeight::Bold), Style::default());
            rows = 1 + (i + 2) / 4;
        }
        if model.background.can_add {
            let (_, add) = swatch(ui, StartAction::ChooseBackground, false, tr("Add background image"));
            ui.add(add, UiWidget::icon(Icon::Plus, 16.0).with_paint(Paint::Secondary), Style::default());
            rows = rows.max(1 + (model.background.list.len() + 1) / 4);
        }
        height += rows as f32 * (SWATCH.y + SWATCH_GAP) + 4.0;

        let path_row = ui.add(
            plate,
            UiWidget::container(),
            Style {
                size: Size { width: percent(1.0), height: length(PANEL_ROW) },
                flex_shrink: 0.0,
                ..row(8.0)
            },
        );
        let plate_f = ui.add(
            path_row,
            UiWidget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(10.0),
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(32.0) },
                min_size: Size { width: length(0.0), height: auto() },
                padding: pad(10.0, 10.0, 0.0, 0.0),
                ..row(0.0)
            },
        );
        let field = ui.add_text_field(
            plate_f,
            tr("Add from a path, e.g. ~/Pictures/sea.jpg"),
            TextStyle::Footnote,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(20.0) },
                ..Default::default()
            },
        );
        self.panel.path_field = field;
        let add = ui.add(
            path_row,
            UiWidget::container()
                .with_action(StartAction::AddPath.id())
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin)),
            Style {
                size: Size { width: auto(), height: length(30.0) },
                padding: pad(14.0, 14.0, 0.0, 0.0),
                flex_shrink: 0.0,
                ..row(0.0)
            },
        );
        ui.add_label(add, tr("Add"), TextStyle::Callout, Style::default());
        height += PANEL_ROW;

        if !model.background.error.is_empty() {
            let note = ui.add_label(plate, &model.background.error, TextStyle::Footnote, shrinking());
            secondary(ui, note);
            ui.set_label_lines(note, 3);
            self.panel.note = Some(note);
            height += 3.0 * 18.0;
        }
        if let Some(entry) = &model.background.selected {
            if entry.kind != Kind::Image {
                switch(
                    ui,
                    &mut self.panel.controls,
                    tr("Live wallpaper"),
                    home::Widget::LiveWallpapers,
                    model.background.live_setting,
                    true,
                );
                height += PANEL_ROW;
            }
            for (text, control, value) in [
                (tr("Dim"), Control::Dim, model.background.dim / DIM_MAX),
                (tr("Blur"), Control::Blur, model.background.blur / BLUR_MAX),
            ] {
                let r = ui.add(
                    plate,
                    UiWidget::container(),
                    Style {
                        size: Size { width: auto(), height: length(PANEL_ROW) },
                        flex_shrink: 0.0,
                        padding: pad(14.0, 0.0, 0.0, 0.0),
                        ..row(10.0)
                    },
                );
                let l = ui.add_label(r, text, TextStyle::Callout, Style { flex_grow: 1.0, ..shrinking() });
                secondary(ui, l);
                let s = ui.add(
                    r,
                    UiWidget::slider(value.clamp(0.0, 1.0)),
                    Style {
                        size: Size { width: length(150.0), height: auto() },
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                );
                self.panel.controls.push((s, control));
                height += PANEL_ROW;
            }
        }
        let more = ui.add(
            plate,
            UiWidget::container().with_action(StartAction::MoreSettings.id()).with_radius(CAPSULE),
            Style { size: Size { width: auto(), height: length(36.0) }, flex_shrink: 0.0, ..row(0.0) },
        );
        let l = ui.add_label(more, tr("More Settings…"), TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(l) {
            w.set_paint(Paint::Accent);
        }
        height += 36.0;
        self.panel.height = height;
    }

    /// Puts the thumbnails that are ready into the picker's swatches.
    pub fn sync_thumbnails(&mut self, ui: &mut Ui, get: &dyn Fn(&str) -> Option<crate::home::Image>) {
        for (name, node, done) in &mut self.thumb_nodes {
            if *done {
                continue;
            }
            if let Some(image) = get(name) {
                ui.set_image(*node, Some(image));
                ui.set_visible(*node, true);
                *done = true;
            }
        }
    }

    /// A switch or slider of the panel moved.
    pub fn value_changed(&self, node: NodeId, value: f32) -> Option<StartAction> {
        let (_, control) = self.panel.controls.iter().find(|(n, _)| *n == node)?;
        Some(match control {
            Control::Toggle(widget) => StartAction::Widget(*widget, value > 0.5),
            Control::Dim => {
                StartAction::Look { dim: Some((value * DIM_MAX).clamp(0.0, DIM_MAX)), blur: None }
            }
            Control::Blur => {
                StartAction::Look { dim: None, blur: Some((value * BLUR_MAX).round().clamp(0.0, BLUR_MAX)) }
            }
        })
    }

    /// What `Enter` in a field means: the text of the search field, or a dialog / path.
    pub fn submitted(&self, node: NodeId) -> Option<StartAction> {
        if node == self.editor.name || node == self.editor.url {
            Some(StartAction::EditorSave)
        } else if node == self.panel.path_field {
            Some(StartAction::AddPath)
        } else {
            None
        }
    }

    /// The engine menu's rows.
    pub fn open_engine_menu(&mut self, ui: &mut Ui, config: &Config) {
        let Some(model) = &self.model else { return };
        let items: Vec<MenuItem> = model
            .search
            .engines
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut item = MenuItem::new(Icon::Globe, e.name, MenuAction::StartEngine(i));
                item.checked = model.search.current == Some(i);
                item
            })
            .collect();
        let anchor = ui.frame(self.engine);
        let anchor =
            Frame::new(anchor.origin.x, anchor.origin.y, anchor.size.x, anchor.size.y + MENU_ANCHOR_GAP);
        let window = self.rect.size + self.rect.origin;
        self.menu.open(ui, config, anchor, window, items);
    }

    // ---- per frame ----

    /// Advances the paging, the tiles' hover, the dialog and the panel; `pointer` is the
    /// pointer in logical px.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32, pointer: Option<Vec2>) {
        if !self.open {
            return;
        }
        if config.glass.reduce_motion {
            self.offset.snap(self.offset.target);
        } else {
            self.offset.step(dt);
        }
        let want = -(self.page as f32) * self.sites_width();
        if (self.offset.target - want).abs() > 0.5 && !self.tiles.is_empty() {
            self.offset.target = want;
        }
        let current = ui.style(self.track).margin.left;
        if current != length(self.offset.value) {
            self.apply_offset(ui);
        }
        let covered = self.editor.open || self.menu.is_open() && false;
        for (i, tile) in self.tiles.iter_mut().enumerate() {
            let hovered = !covered
                && (pointer
                    .is_some_and(|p| ui.frame(tile.root).contains(p) && ui.frame(self.viewport).contains(p))
                    || self.focus == Focus::Tile(i));
            tile.hover.target = f32::from(u8::from(hovered));
            if config.glass.reduce_motion {
                tile.hover.snap(tile.hover.target);
            } else {
                tile.hover.step(dt);
            }
            if let Some(actions) = tile.actions {
                let v = tile.hover.value.clamp(0.0, 1.0);
                let show = v > SETTLED_UNIT || hovered;
                if ui.is_visible(actions) != show {
                    ui.set_visible(actions, show);
                }
                if show {
                    ui.set_opacity(actions, v);
                }
            }
        }
        // The dialog fades.
        self.editor.shown.target = f32::from(u8::from(self.editor.open));
        if config.glass.reduce_motion {
            self.editor.shown.snap(self.editor.shown.target);
        } else {
            self.editor.shown.step(dt);
        }
        let shown = self.editor.shown.value.clamp(0.0, 1.0);
        if self.editor.open || shown > SETTLED_UNIT {
            ui.set_opacity(self.editor.backdrop, shown);
        } else if ui.is_visible(self.editor.backdrop) {
            ui.set_visible(self.editor.backdrop, false);
        }
        self.panel.popover.animate(ui, dt);
        // The gear turns accent-tinted while its panel is open.
        self.apply_gear(ui);
        self.menu.track_hover(ui, config);
        self.menu.animate(ui, dt);
        self.fit_stats(ui);
        let _ = SETTLED;
    }

    fn apply_gear(&mut self, ui: &mut Ui) {
        let on = self.focus == Focus::Gear || self.panel.popover.is_open();
        let wanted = if on {
            Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
        } else {
            Background::Glass(GlassLevel::Thin)
        };
        if let Some(w) = ui.widget_mut(self.gear)
            && w.background != wanted
        {
            w.background = wanted;
        }
        // The glyph reads on the tint (it is the accent colour on plain glass).
        let ink = if on { Paint::OnAccent } else { paint_of(Ink::Accent, self.over_image) };
        if let Some(glyph) = ui.children(self.gear).first().copied()
            && let Some(g) = ui.widget_mut(glyph)
        {
            g.set_paint(ink);
        }
    }

    /// The Shields row hides while the content above would reach into it.
    fn fit_stats(&mut self, ui: &mut Ui) {
        if !self.model.as_ref().is_some_and(|m| m.stats.show) {
            return;
        }
        let stats = ui.frame(self.stats);
        if stats.size.y < 1.0 {
            return;
        }
        let limit = stats.origin.y - STATS_CLEARANCE;
        let mut bottom = 0.0f32;
        for node in ui.children(self.main) {
            if ui.is_visible(node) {
                let f = ui.frame(node);
                if f.size.y > 0.0 {
                    bottom = bottom.max(f.origin.y + f.size.y);
                }
            }
        }
        let opacity = if bottom > limit { 0.0 } else { 1.0 };
        ui.set_opacity(self.stats, opacity);
    }
}

/// "Title · host" for a tile's tooltip.
fn tile_tip(site: &Site) -> String {
    let host = site.host.trim_start_matches("www.");
    if site.title.is_empty() || site.title == site.host || site.title == host {
        host.to_owned()
    } else {
        format!("{} \u{b7} {host}", site.title)
    }
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    pad(v, v, v, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(i: usize, pinned: bool) -> Site {
        Site {
            url: format!("https://site{i}.example/"),
            title: format!("Site {i}"),
            host: format!("site{i}.example"),
            pinned,
        }
    }

    fn model(sites: usize, count: u32) -> StartModel {
        let mut config = Config::default();
        config.home.top_sites_count = count;
        let facts = home::BackgroundFacts { selected: None, entries: Vec::new(), live: true, error: "" };
        let mut m = StartModel::build(&config, None, 12, "Ada Lovelace", false, &facts, 0);
        m.sites.sites = (0..sites).map(|i| site(i, i == 0)).collect();
        m
    }

    fn page_at(width: f32, height: f32, m: StartModel) -> (Ui, StartPage) {
        let mut config = Config::default();
        config.glass.reduce_motion = true;
        let mut ui = Ui::new(axion_ui::Theme::new(&config, true), 1.0);
        ui.set_viewport(Vec2::new(width, height), 1.0);
        let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
        let favicons = Favicons::new(device, queue, None);
        let mut page = StartPage::build(&mut ui, &config);
        page.open(
            &mut ui,
            &config,
            Frame { origin: Vec2::new(10.0, 70.0), size: Vec2::new(width - 20.0, height - 80.0) },
        );
        page.set_model(&mut ui, &config, m, &favicons, 1);
        page.set_time(&mut ui, 9, 5);
        ui.layout();
        (ui, page)
    }

    #[test]
    fn tiles_fill_a_row_and_page() {
        // 30 sites at 6 columns x 2 rows = 12 per page: three pages, the third holds the rest.
        let (ui, page) = page_at(1100.0, 800.0, model(30, 40));
        assert_eq!(page.columns, 6);
        assert_eq!(page.pages, 3);
        let first = ui.frame(page.tiles[0].root);
        let sixth = ui.frame(page.tiles[5].root);
        let seventh = ui.frame(page.tiles[6].root);
        assert!((first.origin.y - sixth.origin.y).abs() < 0.5, "one row");
        assert!(seventh.origin.y > first.origin.y + first.size.y, "the next row");
        // The second page lies to the right of the first, one viewport away.
        let next_page = ui.frame(page.tiles[12].root);
        assert!(
            (next_page.origin.x - first.origin.x - page.sites_width()).abs() < 1.0,
            "{next_page:?} {first:?}"
        );
        // The viewport shows only the first page.
        let viewport = ui.frame(page.viewport);
        assert!(viewport.contains(first.center()) && !viewport.contains(next_page.center()));
    }

    #[test]
    fn few_tiles_are_centred_with_the_add_tile() {
        let (ui, page) = page_at(1100.0, 800.0, model(3, 5));
        assert_eq!(page.pages, 1);
        assert!(page.add_tile.is_some(), "fewer sites than the limit: Add Site follows");
        let all: Vec<Frame> =
            page.tiles.iter().chain(page.add_tile.iter()).map(|t| ui.frame(t.root)).collect();
        let left = all.first().map_or(0.0, |f| f.origin.x);
        let right = all.last().map_or(0.0, |f| f.origin.x + f.size.x);
        let middle = ui.frame(page.root).origin.x + ui.frame(page.root).size.x / 2.0;
        assert!(((left + right) / 2.0 - middle).abs() < 1.0, "{left} {right} {middle}");
        let (_, none) = page_at(1100.0, 800.0, model(0, 5));
        assert!(none.tiles.is_empty() && none.add_tile.is_some(), "empty: only Add Site");
        let (_, full) = page_at(1100.0, 800.0, model(5, 5));
        assert!(full.add_tile.is_none(), "at the limit there is no Add Site tile");
    }

    #[test]
    fn narrow_windows_use_fewer_columns() {
        let (ui, page) = page_at(420.0, 800.0, model(8, 8));
        assert!(page.columns <= 3, "{}", page.columns);
        let viewport = ui.frame(page.viewport);
        assert!(viewport.size.x <= 420.0 - 2.0 * SIDE_MARGIN - 20.0 + 1.0);
        for tile in &page.tiles[..(page.columns as usize).min(8)] {
            let f = ui.frame(tile.root);
            assert!(
                f.origin.x >= viewport.origin.x - 0.5
                    && f.origin.x + f.size.x <= viewport.origin.x + viewport.size.x + 0.5
            );
        }
    }

    #[test]
    fn focus_walks_the_controls_in_order() {
        let (mut ui, mut page) = page_at(1100.0, 800.0, model(2, 5));
        let seen: Vec<Focus> = (0..6).map(|_| page.move_focus(&mut ui, true)).collect();
        assert_eq!(
            seen,
            [Focus::Gear, Focus::Engine, Focus::Query, Focus::Tile(0), Focus::Tile(1), Focus::Tile(2)],
        );
        // Past the last control the focus leaves the page; Shift+Tab walks back.
        assert_eq!(page.move_focus(&mut ui, true), Focus::None);
        assert_eq!(page.move_focus(&mut ui, false), Focus::Tile(2));
        assert_eq!(page.activate_focus(), Some(StartAction::AddSite));
        page.move_tile_focus(&mut ui, -1, 0);
        assert_eq!(page.activate_focus(), Some(StartAction::Tile(1)));
    }

    #[test]
    fn private_page_has_no_gear_tiles_or_stats() {
        let mut m = model(3, 5);
        m.private = true;
        m.sites.show = false;
        m.stats.show = false;
        m.greeting_show = false;
        let (ui, page) = page_at(1100.0, 800.0, m);
        assert!(!ui.is_visible(page.gear));
        assert!(!ui.is_visible(page.sites));
        assert!(ui.is_visible(page.private_card));
        assert!(page.tiles.is_empty());
    }

    #[test]
    fn clock_and_greeting_follow_the_time() {
        let (ui, page) = page_at(1100.0, 800.0, model(0, 5));
        let text = |n: NodeId| match ui.widget(n).map(|w| &w.kind) {
            Some(axion_ui::WidgetKind::Label(l)) => l.text().to_owned(),
            _ => String::new(),
        };
        let shown = page.clock_rows.iter().find(|(r, _, _)| ui.is_visible(*r)).copied();
        assert_eq!(shown.map(|(_, t, _)| text(t)).as_deref().map(|s| s.len()), Some(5));
        assert!(text(page.greeting).contains("Ada"));
    }

    #[test]
    fn action_ids_round_trip() {
        for action in SIMPLE.into_iter().chain([
            StartAction::Format(ClockFormat::H24),
            StartAction::Tile(0),
            StartAction::Tile(MAX_TILES - 1),
            StartAction::TilePin(3),
            StartAction::TileEdit(7),
            StartAction::TileRemove(11),
            StartAction::Background(0),
            StartAction::Background(MAX_PICKER - 1),
            StartAction::BackgroundRemove(5),
            StartAction::Dot(0),
            StartAction::Dot(MAX_DOTS - 1),
        ]) {
            assert_eq!(StartAction::from_id(action.id()), Some(action), "{action:?}");
        }
        assert_eq!(StartAction::from_id(ActionId(BASE - 1)), None);
        assert_eq!(StartAction::from_id(ActionId(TILE_BASE + 5)), None);
        assert!(MENU_BASE + crate::menu::MAX_ITEMS as u32 <= 52_000);
        assert!(PICKER_BASE > TILE_BASE + TILE_KINDS * (MAX_TILES as u32 - 1) + 3);
        assert!(DOT_BASE > PICKER_BASE + 2 * MAX_PICKER as u32);
        assert!(MENU_BASE > DOT_BASE + MAX_DOTS as u32);
    }
}
