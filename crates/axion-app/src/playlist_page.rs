//! The Playlist page: a native Axion page that fills the window (the toolbar gives way to it,
//! the sidebar stays). Beside the sidebar a column lists the saved videos — a round play-all
//! button, the list's name and size, Edit, and rows with a thumbnail, up to three lines of
//! title, the length and a "…" menu — and to the right the player: the video in a black
//! rounded area, its title, a progress bar, shuffle / previous / play / next / repeat, speed,
//! sleep timer and fullscreen; Picture in Picture, search and close sit at the top right.
//!
//! This module only builds and updates the nodes; what the buttons mean is decided by the
//! browser (`browser/playlist.rs`), what the rows show by `playlist`.

use std::time::{Duration, Instant};

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{Color, Config};
use axion_i18n::{tr, tr_fmt};
use axion_media::format_time;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Align, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::favicons::{Favicons, favicon_or};
use crate::menu::{Menu, MenuItem, Placement};
use crate::playlist::{Repeat, wrap_title};

/// Action ids of the page: the buttons, then the rows' (`ROW_BASE + 8 * row + kind`), then
/// the page's menu.
const BASE: u32 = 40_000;
const ROW_BASE: u32 = BASE + 100;
const ROW_KINDS: u32 = 8;
/// The most rows the page lays out (further ones are reached through the search).
pub const MAX_ROWS: usize = 1000;
pub const MENU_BASE: u32 = ROW_BASE + ROW_KINDS * MAX_ROWS as u32 + 100;

/// What a click on the page means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageAction {
    Close,
    PlayAll,
    Edit,
    Sort,
    Search,
    PictureInPicture,
    Fullscreen,
    Shuffle,
    Previous,
    PlayPause,
    Next,
    Repeat,
    Speed,
    /// The speaker button of the volume control: mutes and unmutes.
    Mute,
    Sleep,
    SelectAll,
    DeleteSelected,
    DownloadSelected,
    /// The player area's *Open page* button (a video that can't play there).
    OpenPage,
    /// The row (index into the shown rows) was clicked.
    Row(usize),
    RowMenu(usize),
    RowSelect(usize),
    RowUp(usize),
    RowDown(usize),
}

const SIMPLE: [PageAction; 19] = [
    PageAction::Close,
    PageAction::PlayAll,
    PageAction::Edit,
    PageAction::Sort,
    PageAction::Search,
    PageAction::PictureInPicture,
    PageAction::Fullscreen,
    PageAction::Shuffle,
    PageAction::Previous,
    PageAction::PlayPause,
    PageAction::Next,
    PageAction::Repeat,
    PageAction::Speed,
    PageAction::Mute,
    PageAction::Sleep,
    PageAction::SelectAll,
    PageAction::DeleteSelected,
    PageAction::DownloadSelected,
    PageAction::OpenPage,
];

impl PageAction {
    pub fn id(self) -> ActionId {
        let row = |i: usize, k: u32| ActionId(ROW_BASE + i.min(MAX_ROWS - 1) as u32 * ROW_KINDS + k);
        match self {
            PageAction::Row(i) => row(i, 0),
            PageAction::RowMenu(i) => row(i, 1),
            PageAction::RowSelect(i) => row(i, 2),
            PageAction::RowUp(i) => row(i, 3),
            PageAction::RowDown(i) => row(i, 4),
            simple => ActionId(BASE + SIMPLE.iter().position(|&a| a == simple).unwrap_or(0) as u32),
        }
    }

    pub fn from_id(id: ActionId) -> Option<Self> {
        if let Some(n) = id.0.checked_sub(ROW_BASE).filter(|&n| n < ROW_KINDS * MAX_ROWS as u32) {
            let i = (n / ROW_KINDS) as usize;
            return Some(match n % ROW_KINDS {
                0 => PageAction::Row(i),
                1 => PageAction::RowMenu(i),
                2 => PageAction::RowSelect(i),
                3 => PageAction::RowUp(i),
                4 => PageAction::RowDown(i),
                _ => return None,
            });
        }
        SIMPLE.get(id.0.checked_sub(BASE)? as usize).copied()
    }

    /// Whether the id belongs to the page's buttons or rows.
    #[cfg(test)]
    pub fn owns(id: ActionId) -> bool {
        Self::from_id(id).is_some()
    }
}

// Layout, in logical pixels.
const PAD: f32 = 16.0;
const LIST_WIDTH: f32 = 360.0;
/// The list gets at most this share of the page (narrow windows).
const LIST_SHARE: f32 = 0.46;
const LIST_MIN: f32 = 260.0;
const HEADER_HEIGHT: f32 = 72.0;
const PLAY_ALL: f32 = 46.0;
const THUMB: Vec2 = Vec2::new(112.0, 70.0);
const THUMB_RADIUS: f32 = 11.0;
const ROW_PADDING: f32 = 8.0;
const ROW_RADIUS: f32 = 16.0;
const ROW_GAP: f32 = 12.0;
const TITLE_LINES: usize = 3;
const TITLE_LINE_HEIGHT: f32 = 20.0;
const ROW_HEIGHT: f32 = 2.0 * ROW_PADDING + 78.0;
const MENU_BUTTON: f32 = 34.0;
const SELECT_CIRCLE: f32 = 24.0;
/// Average width of a character of the row titles (14 px type), for wrapping them.
const CHAR_WIDTH: f32 = 7.4;
const TOP_BUTTON: f32 = 42.0;
/// Wide enough that the glass circles stay separate drops.
const TOP_GAP: f32 = 18.0;
const SEARCH_WIDTH: f32 = 280.0;
const VIDEO_RADIUS: f32 = 20.0;
const VIDEO_MIN_HEIGHT: f32 = 140.0;
const CONTROL_SMALL: f32 = 40.0;
const CONTROL_MEDIUM: f32 = 48.0;
const CONTROL_BIG: f32 = 68.0;
const BOTTOM_BUTTON: f32 = 42.0;
/// The volume capsule beside the speed button (at most; it shrinks in a narrow player).
const VOLUME_WIDTH: f32 = 184.0;
/// Space between the speed button and the volume capsule: just over `glass.merge_distance` (20), so
/// their glass stays two shapes.
const VOLUME_GAP: f32 = 24.0;
const SCRUB_SETTLE: Duration = Duration::from_millis(600);
const SEARCH_SPRING: (f32, f32) = (0.3, 0.8);
const SEARCH_SETTLED: f32 = 0.5;

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

fn secondary(ui: &mut Ui, label: NodeId) -> NodeId {
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

/// A glass circle with an icon.
fn glass_circle(
    ui: &mut Ui,
    parent: NodeId,
    action: PageAction,
    icon: Icon,
    side: f32,
    icon_size: f32,
    tip: &str,
) -> (NodeId, NodeId) {
    let node = ui.add(
        parent,
        Widget::container()
            .with_action(action.id())
            .with_radius(CAPSULE)
            .with_background(Background::Glass(GlassLevel::Thin))
            .with_tooltip(tip, None),
        Style { size: square(side), flex_shrink: 0.0, justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let glyph = ui.add(node, Widget::icon(icon, icon_size), Style::default());
    (node, glyph)
}

/// A bare icon button (no glass), for the transport row.
fn plain_button(
    ui: &mut Ui,
    parent: NodeId,
    action: PageAction,
    icon: Icon,
    side: f32,
    icon_size: f32,
    tip: &str,
) -> (NodeId, NodeId) {
    let node = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE).with_tooltip(tip, None),
        Style { size: square(side), flex_shrink: 0.0, justify_content: JustifyContent::CENTER, ..row(0.0) },
    );
    let glyph = ui.add(node, Widget::icon(icon, icon_size), Style::default());
    (node, glyph)
}

/// A glass capsule with a text, for the edit bar.
fn capsule(ui: &mut Ui, parent: NodeId, action: PageAction, icon: Icon, text: &str, danger: bool) -> NodeId {
    let background = if danger {
        Background::Fill(Paint::Fixed(DANGER), 0.16)
    } else {
        Background::Fill(Paint::Label, 0.08)
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(action.id()).with_radius(CAPSULE).with_background(background),
        Style {
            size: Size { width: auto(), height: length(34.0) },
            padding: pad(12.0, 14.0, 0.0, 0.0),
            flex_shrink: 0.0,
            ..row(6.0)
        },
    );
    let paint = if danger { Paint::Fixed(DANGER) } else { Paint::Label };
    ui.add(node, Widget::icon(icon, 16.0).with_paint(paint), Style::default());
    let label = ui.add_label(node, text, TextStyle::Callout, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(paint);
    }
    node
}

const DANGER: Color = Color::rgb(0xE5483E);

/// What a row shows.
pub struct RowModel {
    pub id: i64,
    pub title: String,
    /// The address whose site icon stands in for a missing thumbnail.
    pub site_url: String,
    /// "12:05 · Downloaded · 120 MB".
    pub detail: String,
    pub thumb: Option<(wgpu::TextureView, (u32, u32))>,
    pub downloaded: bool,
    pub watched: bool,
    pub playing: bool,
    pub selected: bool,
    /// A download in progress (0–1), or `Some(None)` when its size is unknown.
    pub progress: Option<Option<f32>>,
}

/// The video's picture: a frame of the page showing it.
pub struct VideoPicture {
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    /// The part of the frame that is the video (0–1).
    pub uv: Frame,
    /// Width / height of the picture.
    pub aspect: f32,
}

/// What the player half shows.
pub struct PlayerModel<'a> {
    pub title: &'a str,
    pub site: &'a str,
    pub elapsed: f64,
    pub duration: f64,
    /// A live stream: the timeline is full and says *Live*.
    pub live: bool,
    pub playing: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub rate: f64,
    pub sleep: bool,
    /// The Playlist's shared volume (0–1) and mute state.
    pub volume: f32,
    pub muted: bool,
    /// Text over the black area instead of a picture (nothing chosen, loading, …).
    pub note: Option<&'a str>,
    /// The note comes with an *Open page* button.
    pub open_page: bool,
    pub video: Option<VideoPicture>,
}

struct RowNodes {
    id: i64,
    row: NodeId,
    detail: NodeId,
    progress: Option<NodeId>,
}

pub struct PlaylistPage {
    root: NodeId,
    list_col: NodeId,
    player_col: NodeId,
    header: NodeId,
    header_name: NodeId,
    header_count: NodeId,
    edit_label: NodeId,
    sort_button: NodeId,
    edit_bar: NodeId,
    delete_label: NodeId,
    viewport: NodeId,
    list: NodeId,
    /// The list has no items: the video area says "No Videos".
    list_empty: bool,
    search_box: NodeId,
    search_field: NodeId,
    search_width: Animated<f32>,
    search_open: bool,
    video_box: NodeId,
    video: NodeId,
    video_note: NodeId,
    open_page_button: NodeId,
    title: NodeId,
    site: NodeId,
    elapsed: NodeId,
    remaining: NodeId,
    scrubber: NodeId,
    scrubbed_at: Option<Instant>,
    play_icon: NodeId,
    shuffle_icon: NodeId,
    repeat_icon: NodeId,
    repeat_one: NodeId,
    speed_label: NodeId,
    volume: NodeId,
    volume_icon: NodeId,
    /// The volume slider was moved by the user a moment ago.
    volume_set_at: Option<Instant>,
    sleep_icon: NodeId,
    rows: Vec<RowNodes>,
    scroll: f32,
    content_height: f32,
    rect: Frame,
    toggle_inset: f32,
    open: bool,
    menu: Menu,
    shown_aspect: f32,
}

impl PlaylistPage {
    pub fn build(ui: &mut Ui, config: &Config) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Ground, 1.0))
                .with_radius(config.effective_layout().page_corner_radius)
                .with_clip(),
            Style { position: Position::Absolute, flex_direction: FlexDirection::Row, ..Default::default() },
        );

        // ---- the list column ----
        let list_col = ui.add(
            root,
            Widget::container(),
            Style { size: Size { width: length(LIST_WIDTH), height: percent(1.0) }, ..column(0.0) },
        );
        let header = ui.add(
            list_col,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: length(HEADER_HEIGHT) },
                flex_shrink: 0.0,
                padding: pad(PAD, PAD, 0.0, 0.0),
                ..row(12.0)
            },
        );
        let play_all = ui.add(
            header,
            Widget::container()
                .with_action(PageAction::PlayAll.id())
                .with_radius(CAPSULE)
                .with_background(Background::TintedGlass(GlassLevel::Thin, Paint::Accent))
                .with_tooltip(tr("Play All"), None),
            Style {
                size: square(PLAY_ALL),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        ui.add(
            play_all,
            Widget::icon(Icon::Play, 22.0)
                .with_paint(Paint::Fixed(Color::rgb(0xFFFFFF)))
                .with_icon_weight(IconWeight::Heavy),
            Style::default(),
        );
        let titles = ui.add(
            header,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..column(0.0)
            },
        );
        let header_name = ui.add_label(titles, "", TextStyle::Title2, shrinking());
        let header_count = dim_label(ui, titles, "", TextStyle::Footnote);
        let sort_button = ui.add(
            header,
            Widget::container()
                .with_action(PageAction::Sort.id())
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Label, 0.08))
                .with_tooltip(tr("Sort"), None),
            Style {
                size: square(34.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        ui.add(sort_button, Widget::icon(Icon::Sort, 18.0).with_paint(Paint::Accent), Style::default());
        let edit = ui.add(
            header,
            Widget::container().with_action(PageAction::Edit.id()).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(34.0) },
                padding: pad(10.0, 10.0, 0.0, 0.0),
                flex_shrink: 0.0,
                ..row(0.0)
            },
        );
        let edit_label = ui.add_label(edit, tr("Edit"), TextStyle::BodyEmphasis, Style::default());
        if let Some(w) = ui.widget_mut(edit_label) {
            w.set_paint(Paint::Accent);
        }

        // Edit mode: select all, download, delete.
        let edit_bar = ui.add(
            list_col,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: auto() },
                flex_shrink: 0.0,
                padding: pad(PAD, PAD, 0.0, 8.0),
                ..row(8.0)
            },
        );
        capsule(ui, edit_bar, PageAction::SelectAll, Icon::Check, tr("All"), false);
        capsule(ui, edit_bar, PageAction::DownloadSelected, Icon::Download, tr("Download"), false);
        let delete = capsule(ui, edit_bar, PageAction::DeleteSelected, Icon::Trash, tr("Remove"), true);
        let delete_label = ui.children(delete).get(1).copied().unwrap_or(delete);
        ui.set_visible(edit_bar, false);

        let viewport = ui.add(
            list_col,
            Widget::container().with_clip(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                size: Size { width: percent(1.0), height: auto() },
                ..column(0.0)
            },
        );
        let list = ui.add(
            viewport,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: auto() },
                padding: pad(PAD - ROW_PADDING, PAD - ROW_PADDING, 0.0, PAD),
                ..column(2.0)
            },
        );

        // ---- the player column ----
        let player_col = ui.add(
            root,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                size: Size { width: auto(), height: percent(1.0) },
                padding: pad(8.0, PAD + 4.0, PAD, PAD + 4.0),
                ..column(14.0)
            },
        );
        let top = ui.add(
            player_col,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: length(TOP_BUTTON) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::END,
                ..row(TOP_GAP)
            },
        );
        glass_circle(
            ui,
            top,
            PageAction::PictureInPicture,
            Icon::PictureInPicture,
            TOP_BUTTON,
            20.0,
            tr("Picture in Picture"),
        );
        let search_box = ui.add(
            top,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_radius(CAPSULE)
                .with_action(PageAction::Search.id())
                .with_tooltip(tr("Search Videos"), None)
                .with_clip(),
            Style {
                size: Size { width: length(TOP_BUTTON), height: length(TOP_BUTTON) },
                flex_shrink: 0.0,
                padding: pad(11.0, 12.0, 0.0, 0.0),
                ..row(8.0)
            },
        );
        ui.add(
            search_box,
            Widget::icon(Icon::Search, 20.0),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        let search_field = ui.add_text_field(
            search_box,
            tr("Search"),
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        glass_circle(ui, top, PageAction::Close, Icon::Close, TOP_BUTTON, 18.0, tr("Close Playlist"));

        let video_box = ui.add(
            player_col,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), 1.0))
                .with_radius(VIDEO_RADIUS)
                .with_clip(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                size: Size { width: percent(1.0), height: auto() },
                min_size: Size { width: length(0.0), height: length(VIDEO_MIN_HEIGHT) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let video = ui.add(video_box, Widget::image(), Style::default());
        ui.set_visible(video, false);
        let note_box =
            ui.add(video_box, Widget::container(), Style { align_items: AlignItems::CENTER, ..column(10.0) });
        ui.add(
            note_box,
            Widget::icon(Icon::Playlist, 40.0).with_paint(Paint::Fixed(Color::rgb(0x8E8E93))),
            Style::default(),
        );
        let video_note = ui.add_label(note_box, "", TextStyle::Body, Style::default());
        if let Some(w) = ui.widget_mut(video_note) {
            w.set_paint(Paint::Fixed(Color::rgb(0x8E8E93)));
        }
        let open_page_button =
            capsule(ui, note_box, PageAction::OpenPage, Icon::Globe, tr("Open page"), false);
        ui.set_visible(open_page_button, false);

        let info = ui.add(
            player_col,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: auto() },
                flex_shrink: 0.0,
                padding: pad(4.0, 4.0, 0.0, 0.0),
                ..column(0.0)
            },
        );
        let title = ui.add_label(info, "", TextStyle::Title2, shrinking());
        let site = dim_label(ui, info, "", TextStyle::Footnote);

        // The thick progress bar's grab area is as tall as its row.
        let hit = config.media.progress_hit_height.max(crate::player::MIN_HIT_HEIGHT);
        let scrub = ui.add(
            player_col,
            Widget::container(),
            Style { size: Size { width: percent(1.0), height: length(hit) }, flex_shrink: 0.0, ..row(10.0) },
        );
        let elapsed = dim_label(ui, scrub, "0:00", TextStyle::Caption);
        let scrubber = ui.add(
            scrub,
            Widget::line_slider(0.0).with_track(config.media.progress_thickness),
            Style {
                flex_grow: 1.0,
                min_size: Size { width: length(40.0), height: length(hit) },
                ..Default::default()
            },
        );
        let remaining = dim_label(ui, scrub, "−0:00", TextStyle::Caption);

        let transport = ui.add(
            player_col,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: length(CONTROL_BIG) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(22.0)
            },
        );
        let (_, shuffle_icon) = plain_button(
            ui,
            transport,
            PageAction::Shuffle,
            Icon::Shuffle,
            CONTROL_SMALL,
            22.0,
            tr("Shuffle"),
        );
        plain_button(
            ui,
            transport,
            PageAction::Previous,
            Icon::Previous,
            CONTROL_MEDIUM,
            26.0,
            tr("Previous"),
        );
        let (_, play_icon) = glass_circle(
            ui,
            transport,
            PageAction::PlayPause,
            Icon::Play,
            CONTROL_BIG,
            30.0,
            tr("Play or Pause"),
        );
        plain_button(ui, transport, PageAction::Next, Icon::Next, CONTROL_MEDIUM, 26.0, tr("Next"));
        let (repeat_button, repeat_icon) =
            plain_button(ui, transport, PageAction::Repeat, Icon::Repeat, CONTROL_SMALL, 22.0, tr("Repeat"));
        let repeat_one = ui.add_label(
            repeat_button,
            "1",
            TextStyle::Caption,
            Style {
                position: Position::Absolute,
                inset: Rect { left: auto(), right: length(6.0), top: length(4.0), bottom: auto() },
                ..Default::default()
            },
        );
        ui.set_visible(repeat_one, false);

        let bottom = ui.add(
            player_col,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: length(BOTTOM_BUTTON) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::SPACE_BETWEEN,
                ..row(0.0)
            },
        );
        // Three equal slots, so the sleep button sits exactly under the play button whatever
        // the speed button's width is.
        let slot = |ui: &mut Ui, justify: JustifyContent| {
            ui.add(
                bottom,
                Widget::container(),
                Style {
                    flex_grow: 1.0,
                    flex_basis: length(0.0),
                    size: Size { width: auto(), height: length(BOTTOM_BUTTON) },
                    justify_content: justify,
                    ..row(0.0)
                },
            )
        };
        let (left_slot, middle_slot, right_slot) = (
            slot(ui, JustifyContent::START),
            slot(ui, JustifyContent::CENTER),
            slot(ui, JustifyContent::END),
        );
        // The sleep button's slot is only as wide as the button: the side slots (equal, so it
        // stays under the play button) get all the rest, room for the longer volume control.
        let mut middle = ui.style(middle_slot);
        middle.flex_grow = 0.0;
        middle.flex_basis = auto();
        middle.size.width = length(BOTTOM_BUTTON);
        ui.set_style(middle_slot, middle);
        let speed = ui.add(
            left_slot,
            Widget::container()
                .with_action(PageAction::Speed.id())
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin))
                .with_tooltip(tr("Playback Speed"), None),
            Style {
                size: Size { width: length(BOTTOM_BUTTON + 14.0), height: length(BOTTOM_BUTTON) },
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        let speed_label = ui.add_label(speed, "1×", TextStyle::BodyEmphasis, Style::default());
        ui.set_label_align(speed_label, Align::Center);
        // The volume: a glass capsule with the speaker (mute) and a slider, right of the speed.
        let volume_capsule = ui.add(
            left_slot,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                size: Size { width: length(VOLUME_WIDTH), height: length(BOTTOM_BUTTON) },
                flex_shrink: 1.0,
                margin: Rect {
                    left: length(VOLUME_GAP),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                padding: pad(2.0, 16.0, 0.0, 0.0),
                ..row(4.0)
            },
        );
        let volume_button = ui.add(
            volume_capsule,
            Widget::container()
                .with_action(PageAction::Mute.id())
                .with_radius(CAPSULE)
                .with_tooltip(tr("Mute"), None),
            Style {
                size: square(BOTTOM_BUTTON - 4.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        let volume_icon = ui.add(volume_button, Widget::icon(Icon::Volume, 20.0), Style::default());
        let volume = ui.add(
            volume_capsule,
            Widget::line_slider(1.0).with_track(config.media.progress_thickness),
            Style {
                flex_grow: 1.0,
                min_size: Size { width: length(24.0), height: length(hit) },
                ..Default::default()
            },
        );
        let (_, sleep_icon) = glass_circle(
            ui,
            middle_slot,
            PageAction::Sleep,
            Icon::Moon,
            BOTTOM_BUTTON,
            20.0,
            tr("Sleep Timer"),
        );
        glass_circle(
            ui,
            right_slot,
            PageAction::Fullscreen,
            Icon::Fullscreen,
            BOTTOM_BUTTON,
            20.0,
            tr("Fullscreen"),
        );

        // The menu of the rows, the speeds and the sleep timer: after
        // everything, so it lies above.
        let menu = Menu::build(ui, root, MENU_BASE, Placement::BelowStart);

        ui.set_visible(root, false);
        Self {
            root,
            list_col,
            player_col,
            header,
            header_name,
            header_count,
            edit_label,
            sort_button,
            edit_bar,
            delete_label,
            viewport,
            list,
            list_empty: true,
            search_box,
            search_field,
            search_width: Animated::new(
                TOP_BUTTON,
                Spring::new(SEARCH_SPRING.0 * config.motion.speed, config.glass.damping(SEARCH_SPRING.1)),
            ),
            search_open: false,
            video_box,
            video,
            video_note,
            open_page_button,
            title,
            site,
            elapsed,
            remaining,
            scrubber,
            scrubbed_at: None,
            play_icon,
            shuffle_icon,
            repeat_icon,
            repeat_one,
            speed_label,
            volume,
            volume_icon,
            volume_set_at: None,
            sleep_icon,
            rows: Vec::new(),
            scroll: 0.0,
            content_height: 0.0,
            rect: Frame::default(),
            toggle_inset: 0.0,
            open: false,
            menu,
            shown_aspect: 0.0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn search_field(&self) -> NodeId {
        self.search_field
    }

    pub fn scrubber(&self) -> NodeId {
        self.scrubber
    }

    /// The volume slider's node.
    pub fn volume(&self) -> NodeId {
        self.volume
    }

    /// The volume slider was moved by the user.
    pub fn volume_adjusted(&mut self) {
        self.volume_set_at = Some(Instant::now());
    }

    pub fn rect(&self) -> Frame {
        self.rect
    }

    /// Shows the page over `rect` (logical px). `toggle_inset` keeps the header clear of the
    /// toolbar's sidebar button when the sidebar is closed.
    pub fn open(&mut self, ui: &mut Ui, config: &Config, rect: Frame, toggle_inset: f32) {
        self.open = true;
        self.scroll = 0.0;
        self.place(ui, config, rect, toggle_inset);
        ui.set_visible(self.root, true);
        self.apply_search_spring(config);
    }

    /// Hides the page while something else (the fullscreen player) covers the window, or
    /// shows it again.
    pub fn set_hidden(&mut self, ui: &mut Ui, hidden: bool) {
        ui.set_visible(self.root, self.open && !hidden);
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.open = false;
        self.menu.close(ui, config);
        ui.set_visible(self.root, false);
        self.set_search(ui, false);
        ui.set_field_text(self.search_field, "");
    }

    /// Puts the page over `rect` and sizes its columns.
    pub fn place(&mut self, ui: &mut Ui, config: &Config, rect: Frame, toggle_inset: f32) {
        self.rect = rect;
        self.toggle_inset = toggle_inset;
        let mut style = ui.style(self.root);
        style.inset =
            Rect { left: length(rect.origin.x), right: auto(), top: length(rect.origin.y), bottom: auto() };
        style.size = Size { width: length(rect.size.x), height: length(rect.size.y) };
        ui.set_style(self.root, style);
        if let Some(w) = ui.widget_mut(self.root) {
            w.radius = config.effective_layout().page_corner_radius;
        }
        let list_width = LIST_WIDTH.min((rect.size.x * LIST_SHARE).max(LIST_MIN));
        let mut col = ui.style(self.list_col);
        col.size.width = length(list_width);
        col.flex_shrink = 0.0;
        ui.set_style(self.list_col, col);
        let mut head = ui.style(self.header);
        head.padding.left = length(PAD + toggle_inset);
        ui.set_style(self.header, head);
        // The column to the right of the sidebar button starts under it too.
        let mut player = ui.style(self.player_col);
        player.padding.top = length(PAD);
        ui.set_style(self.player_col, player);
    }

    fn apply_search_spring(&mut self, config: &Config) {
        self.search_width.spring =
            Spring::new(SEARCH_SPRING.0 * config.motion.speed, config.glass.damping(SEARCH_SPRING.1));
    }

    /// The width of the list column (logical px).
    fn list_width(&self) -> f32 {
        LIST_WIDTH.min((self.rect.size.x * LIST_SHARE).max(LIST_MIN))
    }

    /// Opens or closes the search field in the top bar (it grows out of the button).
    pub fn set_search(&mut self, ui: &mut Ui, open: bool) -> Vec<axion_ui::UiEvent> {
        self.search_open = open;
        self.search_width.target = if open { SEARCH_WIDTH } else { TOP_BUTTON };
        if open {
            ui.focus(self.search_field)
        } else {
            ui.set_field_text(self.search_field, "");
            ui.blur()
        }
    }

    pub fn search_open(&self) -> bool {
        self.search_open
    }

    /// The list's name, size line and the Edit button's state.
    pub fn set_header(&mut self, ui: &mut Ui, name: &str, summary: &str, editing: bool, selected: usize) {
        ui.set_text(self.header_name, name);
        ui.set_text(self.header_count, summary);
        ui.set_text(self.edit_label, if editing { tr("Done") } else { tr("Edit") });
        ui.set_visible(self.edit_bar, editing);
        let text = if selected > 0 {
            tr_fmt!("Remove ({count})", count = selected)
        } else {
            tr("Remove").to_owned()
        };
        ui.set_text(self.delete_label, &text);
    }

    /// Replaces the rows. `playing_index` is the row to keep in view is not needed: the
    /// scroll position stays unless the list got shorter.
    pub fn set_rows(&mut self, ui: &mut Ui, models: &[RowModel], editing: bool, favicons: &Favicons) {
        ui.clear_children(self.list);
        self.rows.clear();
        let width =
            self.list_width() - 2.0 * PAD - 2.0 * ROW_PADDING - THUMB.x - ROW_GAP - MENU_BUTTON - ROW_GAP;
        let width = if editing { width - SELECT_CIRCLE - ROW_GAP } else { width };
        let chars = (width / CHAR_WIDTH).floor().max(8.0) as usize;
        for (i, m) in models.iter().take(MAX_ROWS).enumerate() {
            let nodes = self.build_row(ui, i, m, editing, chars, favicons);
            self.rows.push(nodes);
        }
        self.list_empty = models.is_empty();
        self.content_height = models.len() as f32 * (ROW_HEIGHT + 2.0) + PAD;
        self.clamp_scroll(ui);
    }

    fn build_row(
        &self,
        ui: &mut Ui,
        i: usize,
        m: &RowModel,
        editing: bool,
        chars: usize,
        favicons: &Favicons,
    ) -> RowNodes {
        let background = if m.selected {
            Background::Fill(Paint::Accent, 0.16)
        } else if m.playing {
            Background::Fill(Paint::Label, 0.09)
        } else {
            Background::None
        };
        let row_node = ui.add(
            self.list,
            Widget::container()
                .with_action(PageAction::Row(i).id())
                .with_radius(ROW_RADIUS)
                .with_background(background),
            Style {
                size: Size { width: percent(1.0), height: length(ROW_HEIGHT) },
                flex_shrink: 0.0,
                padding: pad(ROW_PADDING, ROW_PADDING, ROW_PADDING, ROW_PADDING),
                ..row(ROW_GAP)
            },
        );
        if editing {
            let circle = ui.add(
                row_node,
                Widget::container()
                    .with_action(PageAction::RowSelect(i).id())
                    .with_radius(CAPSULE)
                    .with_background(if m.selected {
                        Background::Fill(Paint::Accent, 1.0)
                    } else {
                        Background::Fill(Paint::Label, 0.12)
                    }),
                Style {
                    size: square(SELECT_CIRCLE),
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            if m.selected {
                ui.add(
                    circle,
                    Widget::icon(Icon::Check, 14.0)
                        .with_paint(Paint::Fixed(Color::rgb(0xFFFFFF)))
                        .with_icon_weight(IconWeight::Bold),
                    Style::default(),
                );
            }
        }
        // The thumbnail, or the site's icon on a neutral glass tile.
        let thumb = ui.add(
            row_node,
            Widget::container()
                .with_background(Background::Fill(Paint::Label, 0.07))
                .with_radius(THUMB_RADIUS)
                .with_clip(),
            Style {
                size: Size { width: length(THUMB.x), height: length(THUMB.y) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        match &m.thumb {
            Some(image) => {
                let node = ui.add(
                    thumb,
                    Widget::image(),
                    Style {
                        size: Size { width: length(THUMB.x), height: length(THUMB.y) },
                        ..Default::default()
                    },
                );
                ui.set_image(node, Some(image.clone()));
                ui.set_image_crop(node, Some(cover(image.1, THUMB)));
            }
            None => {
                let tile = ui.add(
                    thumb,
                    Widget::container()
                        .with_background(Background::Glass(GlassLevel::Thin))
                        .with_radius(14.0),
                    Style {
                        size: square(46.0),
                        justify_content: JustifyContent::CENTER,
                        align_items: AlignItems::CENTER,
                        ..Default::default()
                    },
                );
                favicon_or(ui, tile, &m.site_url, 28.0, favicons, Icon::Media);
            }
        }
        if m.downloaded || m.watched {
            let badge = ui.add(
                thumb,
                Widget::container()
                    .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), 0.55))
                    .with_radius(CAPSULE),
                Style {
                    position: Position::Absolute,
                    inset: Rect { left: length(5.0), right: auto(), top: auto(), bottom: length(5.0) },
                    size: Size { width: auto(), height: length(18.0) },
                    padding: pad(5.0, 5.0, 0.0, 0.0),
                    ..row(3.0)
                },
            );
            if m.downloaded {
                ui.add(
                    badge,
                    Widget::icon(Icon::Download, 11.0).with_paint(Paint::Fixed(Color::rgb(0xFFFFFF))),
                    Style::default(),
                );
            }
            if m.watched {
                ui.add(
                    badge,
                    Widget::icon(Icon::Check, 11.0).with_paint(Paint::Fixed(Color::rgb(0xFFFFFF))),
                    Style::default(),
                );
            }
        }

        // Title (up to three lines), then the length and state.
        let text = ui.add(
            row_node,
            Widget::container(),
            Style {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..column(1.0)
            },
        );
        for line in wrap_title(&m.title, chars, TITLE_LINES) {
            ui.add_label(
                text,
                &line,
                TextStyle::Callout,
                Style {
                    size: Size { width: auto(), height: length(TITLE_LINE_HEIGHT) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
        }
        let detail = dim_label(ui, text, &m.detail, TextStyle::Footnote);
        let progress = m.progress.map(|p| {
            let bar = ui.add(
                text,
                Widget::progress(Paint::Accent),
                Style {
                    size: Size { width: percent(1.0), height: length(3.0) },
                    margin: Rect {
                        left: length(0.0),
                        right: length(0.0),
                        top: length(3.0),
                        bottom: length(0.0),
                    },
                    ..Default::default()
                },
            );
            ui.set_progress(bar, p.unwrap_or(0.0));
            bar
        });

        if editing {
            let arrows = ui.add(row_node, Widget::container(), Style { flex_shrink: 0.0, ..column(0.0) });
            for (action, icon) in
                [(PageAction::RowUp(i), Icon::ChevronUp), (PageAction::RowDown(i), Icon::ChevronDown)]
            {
                let b = ui.add(
                    arrows,
                    Widget::container().with_action(action.id()).with_radius(CAPSULE),
                    Style {
                        size: Size { width: length(MENU_BUTTON), height: length(MENU_BUTTON * 0.8) },
                        justify_content: JustifyContent::CENTER,
                        ..row(0.0)
                    },
                );
                ui.add(b, Widget::icon(icon, 16.0).with_paint(Paint::Secondary), Style::default());
            }
        } else {
            let more = ui.add(
                row_node,
                Widget::container()
                    .with_action(PageAction::RowMenu(i).id())
                    .with_radius(CAPSULE)
                    .with_tooltip(tr("More"), None),
                Style {
                    size: square(MENU_BUTTON),
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            ui.add(
                more,
                Widget::icon(Icon::More, 20.0)
                    .with_paint(Paint::Secondary)
                    .with_icon_weight(IconWeight::Heavy),
                Style::default(),
            );
        }
        RowNodes { id: m.id, row: row_node, detail, progress }
    }

    /// Updates a row's second line and download bar in place (downloads report often).
    pub fn update_row(&mut self, ui: &mut Ui, id: i64, detail: &str, progress: Option<Option<f32>>) {
        let Some(row) = self.rows.iter().find(|r| r.id == id) else { return };
        ui.set_text(row.detail, detail);
        if let (Some(bar), Some(p)) = (row.progress, progress) {
            ui.set_progress(bar, p.unwrap_or(0.0));
        }
    }

    /// Whether the row of `id` has a download bar (else the list is rebuilt to give it one).
    pub fn row_has_bar(&self, id: i64) -> bool {
        self.rows.iter().find(|r| r.id == id).is_some_and(|r| r.progress.is_some())
    }

    /// The frame of row `i` (logical px), for anchoring its menu.
    pub fn row_frame(&self, ui: &Ui, i: usize) -> Option<Frame> {
        self.rows.get(i).map(|r| ui.frame(r.row))
    }

    /// Placement and covered nodes for the menu of row `i`: it grows out of the row and covers
    /// it (the row's content fades out), like a tab row's context menu.
    pub fn row_menu_at(&self, ui: &Ui, i: usize) -> (Placement, Vec<NodeId>) {
        (Placement::Over, self.rows.get(i).map(|r| ui.children(r.row)).unwrap_or_default())
    }

    pub fn frame_of(&self, ui: &Ui, action: PageAction) -> Frame {
        let node = match action {
            PageAction::Speed => ui.parent(self.speed_label),
            PageAction::Sleep => ui.parent(self.sleep_icon),
            PageAction::Sort => Some(self.sort_button),
            _ => None,
        };
        node.map_or_else(|| self.rect, |n| ui.frame(n))
    }

    pub fn list_contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.open && ui.frame(self.viewport).contains(p)
    }

    /// The video area on screen (logical px): clicks inside it reach the page showing it.
    pub fn video_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.video_box)
    }

    pub fn scroll_by(&mut self, ui: &mut Ui, dy: f32) {
        self.scroll += -dy;
        self.clamp_scroll(ui);
    }

    fn clamp_scroll(&mut self, ui: &mut Ui) {
        let visible = ui.frame(self.viewport).size.y;
        let max = (self.content_height - visible).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
        let mut style = ui.style(self.list);
        style.margin =
            Rect { left: length(0.0), right: length(0.0), top: length(-self.scroll), bottom: length(0.0) };
        ui.set_style(self.list, style);
    }

    /// Scrolls so row `i` is in view.
    pub fn reveal(&mut self, ui: &mut Ui, i: usize) {
        let top = i as f32 * (ROW_HEIGHT + 2.0);
        let visible = ui.frame(self.viewport).size.y;
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_HEIGHT > self.scroll + visible {
            self.scroll = top + ROW_HEIGHT - visible;
        }
        self.clamp_scroll(ui);
    }

    /// Menus (rows, list, speed, sleep timer) opening from `anchor`.
    ///
    /// `cover`: the nodes the menu grows over (the button it flows out of, the controls
    /// next to it); they fade out as the menu grows.
    pub fn open_menu(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window: Vec2,
        items: Vec<MenuItem>,
        (placement, cover): (Placement, Vec<NodeId>),
    ) {
        self.menu.set_placement(placement);
        self.menu.set_covered_nodes(ui, cover);
        self.menu.open(ui, config, anchor, window, items);
    }

    /// Placement and covered node for the menu of a button of the player.
    pub fn menu_at(&self, ui: &Ui, action: PageAction) -> (Placement, Vec<NodeId>) {
        let parent = |n: NodeId| ui.parent(n);
        match action {
            PageAction::Sleep => {
                // The sleep button and the whole transport row it grows over.
                let transport = parent(self.play_icon).and_then(parent);
                let mut nodes: Vec<NodeId> = transport.map(|t| ui.children(t)).unwrap_or_default();
                nodes.extend(parent(self.sleep_icon));
                (Placement::OverCentered, nodes)
            }
            PageAction::Speed => {
                // The speed button, and the shuffle and previous buttons it grows over.
                let transport = parent(self.play_icon).and_then(parent);
                let mut nodes: Vec<NodeId> =
                    transport.map(|t| ui.children(t).into_iter().take(2).collect()).unwrap_or_default();
                nodes.extend(parent(self.speed_label));
                (Placement::Over, nodes)
            }
            // The sort button, which the menu grows out of.
            PageAction::Sort => (Placement::Over, vec![self.sort_button]),
            _ => (Placement::BelowStart, Vec::new()),
        }
    }

    pub fn menu(&mut self) -> &mut Menu {
        &mut self.menu
    }

    /// The player half: texts, buttons' states, the picture.
    pub fn set_player(&mut self, ui: &mut Ui, m: &PlayerModel<'_>) {
        ui.set_text(self.title, m.title);
        ui.set_text(self.site, m.site);
        ui.set_text(self.elapsed, &format_time(m.elapsed));
        let remaining = if m.live {
            tr("Live").to_owned()
        } else if m.duration > 0.0 {
            format!("−{}", format_time(m.duration - m.elapsed))
        } else {
            String::new()
        };
        ui.set_text(self.remaining, &remaining);
        let scrubbing = ui.dragging() == Some(self.scrubber)
            || self.scrubbed_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
        if m.live {
            ui.set_value(self.scrubber, 1.0);
        } else if !scrubbing && m.duration > 0.0 {
            ui.set_value(self.scrubber, (m.elapsed / m.duration).clamp(0.0, 1.0) as f32);
        }
        ui.set_icon(self.play_icon, if m.playing { Icon::Pause } else { Icon::Play });
        let on = |on: bool| if on { Paint::Accent } else { Paint::Label };
        for (node, paint) in [
            (self.shuffle_icon, on(m.shuffle)),
            (self.repeat_icon, on(m.repeat != Repeat::Off)),
            (self.sleep_icon, on(m.sleep)),
        ] {
            if let Some(w) = ui.widget_mut(node) {
                w.set_paint(paint);
            }
        }
        ui.set_visible(self.repeat_one, m.repeat == Repeat::One);
        let rate = format!("{}×", trim_rate(m.rate));
        ui.set_text(self.speed_label, &rate);
        let adjusting = ui.dragging() == Some(self.volume)
            || self.volume_set_at.is_some_and(|t| t.elapsed() < SCRUB_SETTLE);
        if !adjusting {
            ui.set_value(self.volume, if m.muted { 0.0 } else { m.volume });
        }
        ui.set_icon(self.volume_icon, if m.muted || m.volume <= 0.0 { Icon::Muted } else { Icon::Volume });

        match (&m.video, m.note) {
            (Some(video), _) => {
                ui.set_image(self.video, Some((video.view.clone(), video.size)));
                ui.set_image_crop(self.video, Some(video.uv));
                if (video.aspect - self.shown_aspect).abs() > 0.001 || !ui.is_visible(self.video) {
                    self.shown_aspect = video.aspect;
                    ui.set_visible(self.video, true);
                }
                let bounds = ui.frame(self.video_box).size;
                let fit = fit_aspect(bounds, video.aspect);
                // A picture that fills the area shares its rounded corners.
                if let Some(w) = ui.widget_mut(self.video) {
                    w.radius = if (fit - bounds).abs().max_element() < 1.5 { VIDEO_RADIUS } else { 0.0 };
                }
                let mut style = ui.style(self.video);
                let wanted = Size { width: length(fit.x), height: length(fit.y) };
                if style.size != wanted {
                    style.size = wanted;
                    ui.set_style(self.video, style);
                }
                ui.set_text(self.video_note, "");
                if let Some(parent) = ui.parent(self.video_note) {
                    ui.set_visible(parent, false);
                }
            }
            (None, note) => {
                ui.set_visible(self.video, false);
                self.shown_aspect = 0.0;
                ui.set_text(
                    self.video_note,
                    note.unwrap_or(if self.list_empty { tr("No Videos") } else { tr("Choose a video") }),
                );
                ui.set_visible(self.open_page_button, m.open_page);
                if let Some(parent) = ui.parent(self.video_note) {
                    ui.set_visible(parent, true);
                }
            }
        }
    }

    /// The scrubber was moved by the user.
    pub fn scrubbed(&mut self) {
        self.scrubbed_at = Some(Instant::now());
    }

    /// Advances the search field's growth and the menu.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32) {
        if !self.open {
            return;
        }
        if config.glass.reduce_motion {
            self.search_width.snap(self.search_width.target);
        } else {
            self.search_width.step(dt);
        }
        if !self.search_width.is_settled(SEARCH_SETTLED)
            || ui.style(self.search_box).size.width != length(self.search_width.target)
        {
            let w = self.search_width.value.max(TOP_BUTTON);
            let mut style = ui.style(self.search_box);
            style.size.width = length(w);
            ui.set_style(self.search_box, style);
        }
        self.menu.track_hover(ui, config);
        self.menu.animate(ui, dt);
    }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

/// A secondary-coloured label that may shrink.
fn dim_label(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, shrinking());
    secondary(ui, label)
}

/// "1", "1.25", "0.5".
fn trim_rate(rate: f64) -> String {
    let text = format!("{rate:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The largest size of aspect `aspect` that fits in `bounds`.
pub fn fit_aspect(bounds: Vec2, aspect: f32) -> Vec2 {
    let aspect = if aspect > 0.0 { aspect } else { 16.0 / 9.0 };
    if bounds.x / bounds.y.max(1.0) > aspect {
        Vec2::new(bounds.y * aspect, bounds.y)
    } else {
        Vec2::new(bounds.x, bounds.x / aspect)
    }
}

/// The part of an image of `size` px that fills a frame of `frame` (centred, like CSS
/// `cover`), in 0–1 coordinates.
pub fn cover(size: (u32, u32), frame: Vec2) -> Frame {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let scale = (frame.x / w).max(frame.y / h);
    let visible = Vec2::new((frame.x / scale / w).min(1.0), (frame.y / scale / h).min(1.0));
    Frame { origin: (Vec2::ONE - visible) / 2.0, size: visible }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        for action in SIMPLE.into_iter().chain([
            PageAction::Row(0),
            PageAction::Row(999),
            PageAction::RowMenu(3),
            PageAction::RowSelect(12),
            PageAction::RowUp(7),
            PageAction::RowDown(8),
        ]) {
            assert_eq!(PageAction::from_id(action.id()), Some(action), "{action:?}");
        }
        assert_eq!(PageAction::from_id(ActionId(BASE - 1)), None);
        assert_eq!(PageAction::from_id(ActionId(ROW_BASE + 5)), None);
        assert!(MENU_BASE > ROW_BASE + ROW_KINDS * (MAX_ROWS as u32 - 1) + 4);
    }

    #[test]
    fn pictures_fit_and_thumbnails_cover() {
        assert_eq!(fit_aspect(Vec2::new(800.0, 600.0), 16.0 / 9.0), Vec2::new(800.0, 450.0));
        assert_eq!(fit_aspect(Vec2::new(800.0, 300.0), 16.0 / 9.0), Vec2::new(300.0 * 16.0 / 9.0, 300.0));
        // A square image in a wide frame shows its middle band.
        let c = cover((100, 100), Vec2::new(200.0, 100.0));
        assert_eq!(c.size, Vec2::new(1.0, 0.5));
        assert_eq!(c.origin, Vec2::new(0.0, 0.25));
        assert_eq!(trim_rate(1.0), "1");
        assert_eq!(trim_rate(1.25), "1.25");
    }
}
