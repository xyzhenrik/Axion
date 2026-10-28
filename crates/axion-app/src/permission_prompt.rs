//! The permission prompt that drips out of the address field ("discord.com wants to: use your
//! microphone"), and the capture popover of the address field's recording indicator (what the
//! tab records, with Stop buttons). Both are glass popovers (see [`Popover`]); flat mode makes
//! them plain cards. The model behind them is `crate::permissions`.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_engine::{PermissionId, PermissionKind, ViewId};
use axion_i18n::tr;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::drip::Phase;
use crate::home::Image;
use crate::permissions::{Capture, Choice, Request, kind_request, offers_visit};
use crate::popover::Popover;

const ACTION_BASE: u32 = 6700;
const CLOSE: ActionId = ActionId(ACTION_BASE);
const DENY: ActionId = ActionId(ACTION_BASE + 1);
const ALLOW: ActionId = ActionId(ACTION_BASE + 2);
const VISIT: ActionId = ActionId(ACTION_BASE + 3);
const STOP_BASE: u32 = ACTION_BASE + 10;
const STOP_ALL: ActionId = ActionId(ACTION_BASE + 19);
/// The screen picker's "share sound" row, and its source cards (one id each).
const PICK_AUDIO: ActionId = ActionId(ACTION_BASE + 4);
const SOURCE_BASE: u32 = ACTION_BASE + 100;
/// Most source cards the picker can have (ids reserved for them).
pub const MAX_SOURCES: usize = 64;
const PICKER_COLUMNS: usize = 3;
const CARD_GAP: f32 = 8.0;
const CARD_PADDING: f32 = 8.0;
const CARD_LABEL: f32 = 18.0;
const CARD_RADIUS: f32 = 14.0;
const THUMB_RADIUS: f32 = 8.0;
const SOUND_ROW: f32 = 34.0;

const WIDTH: f32 = 340.0;
const RADIUS: f32 = 22.0;
const PADDING: f32 = 14.0;
const ROW_GAP: f32 = 8.0;
const GAP_BELOW_ANCHOR: f32 = 10.0;
const MARGIN: f32 = 8.0;
/// Fixed row heights, so the drip knows the prompt's size before layout.
const HEADER: f32 = 44.0;
const KIND_LINE: f32 = 26.0;
const NOTE: f32 = 36.0;
const BUTTON: f32 = 40.0;
const CAPTURE_WIDTH: f32 = 300.0;
const CAPTURE_ROW: f32 = 40.0;

/// The indicator colours: microphone orange, camera green, screen red (as the desktop's own
/// recording indicators).
pub const MICROPHONE_COLOR: Color = Color::rgb(0xFF9500);
pub const CAMERA_COLOR: Color = Color::rgb(0x30B650);
pub const SCREEN_COLOR: Color = Color::rgb(0xFF3B30);
/// The Stop buttons: solid red, legible on any glass.
const STOP_COLOR: Color = Color::rgb(0xE2453A);

/// The icon of a kind.
pub fn kind_icon(kind: PermissionKind) -> Icon {
    match kind {
        PermissionKind::Microphone => Icon::Microphone,
        PermissionKind::Camera => Icon::Camera,
        PermissionKind::ScreenShare | PermissionKind::ScreenAudio => Icon::Screen,
        PermissionKind::Notifications => Icon::Bell,
        PermissionKind::Geolocation => Icon::Location,
        PermissionKind::Clipboard => Icon::Clipboard,
        PermissionKind::AutomaticDownloads => Icon::Download,
        PermissionKind::LocalNetwork => Icon::Globe,
        PermissionKind::LocalFonts => Icon::Note,
        PermissionKind::Midi => Icon::Media,
        _ => Icon::Lock,
    }
}

/// The most telling thing the tab records: what the indicator shows (screen, camera, microphone).
pub fn capture_icon(capture: Capture) -> Option<(Icon, Color)> {
    if capture.screen {
        Some((Icon::Screen, SCREEN_COLOR))
    } else if capture.camera {
        Some((Icon::Camera, CAMERA_COLOR))
    } else if capture.microphone {
        Some((Icon::Microphone, MICROPHONE_COLOR))
    } else {
        None
    }
}

struct Shown {
    view: ViewId,
    id: PermissionId,
}

/// One screen or window offered in the picker.
#[derive(Clone)]
pub struct PickerSource {
    pub name: String,
    pub screen: bool,
    pub image: Option<Image>,
}

/// What the screen picker (a permission prompt that lists sources) shows.
#[derive(Clone)]
pub struct PickerData {
    pub sources: Vec<PickerSource>,
    /// The page also asked for the computer's sound.
    pub audio_asked: bool,
    /// Thumbnail size (logical px).
    pub thumbnail: (f32, f32),
}

struct Picker {
    data: PickerData,
    selected: usize,
    audio: bool,
}

pub struct PermissionPrompt {
    popover: Popover,
    root: NodeId,
    shown: Option<Shown>,
    height: f32,
    /// The address field it flows out of (logical px), as of the last frame.
    address: Frame,
    /// Set while the prompt is the screen picker.
    picker: Option<Picker>,
    request: Option<Request>,
}

impl PermissionPrompt {
    /// `bar` is the toolbar: the prompt's glass flows out of the address field in it.
    pub fn build(ui: &mut Ui, bar: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            bar,
            RADIUS,
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                padding: uniform(PADDING),
                ..column(ROW_GAP)
            },
        );
        let root = popover.root();
        Self {
            popover,
            root,
            shown: None,
            height: 0.0,
            address: Frame::default(),
            picker: None,
            request: None,
        }
    }

    /// The request shown (or flowing out).
    pub fn shown(&self) -> Option<(ViewId, PermissionId)> {
        self.shown.as_ref().map(|s| (s.view, s.id))
    }

    pub fn is_open(&self) -> bool {
        self.shown.is_some() && self.popover.is_open()
    }

    /// Shows `request` (rebuilt only when it is another one). `picker`: the desktop asks which
    /// screen to share (else the whole screen is shared).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        request: &Request,
        picker: bool,
        sources: Option<PickerData>,
    ) {
        if self.shown().is_some_and(|s| s == (request.view, request.id)) {
            return;
        }
        self.picker = sources.map(|data| Picker { data, selected: 0, audio: true });
        self.request = Some(request.clone());
        self.build_content(ui, request, picker);
        self.shown = Some(Shown { view: request.view, id: request.id });
        self.open(ui, config);
    }

    /// Closes it (answered, or its tab is no longer shown).
    pub fn hide(&mut self) {
        self.shown = None;
    }

    /// The width of what is shown: the picker's grid is wider than a plain prompt.
    fn width(&self) -> f32 {
        match &self.picker {
            Some(p) => picker_width(p.data.thumbnail.0),
            None => WIDTH,
        }
    }

    fn open(&mut self, ui: &mut Ui, config: &Config) {
        let a = self.address;
        if a.size.x <= 0.0 {
            return;
        }
        let height = self.height + 2.0 * PADDING;
        let width = self.width();
        let left = (a.origin.x + (a.size.x - width) / 2.0).max(MARGIN);
        let target = Frame::new(left, a.origin.y + a.size.y + GAP_BELOW_ANCHOR, width, height);
        self.popover.open(ui, config, a, target);
    }

    /// A click on one of the screen picker's cards or its sound row; whether it was one.
    pub fn picker_click(&mut self, ui: &mut Ui, id: ActionId) -> bool {
        let Some(picker) = self.picker.as_mut() else { return false };
        if id == PICK_AUDIO {
            picker.audio = !picker.audio;
        } else if let Some(i) = id.0.checked_sub(SOURCE_BASE).map(|i| i as usize)
            && i < picker.data.sources.len()
        {
            picker.selected = i;
        } else {
            return false;
        }
        if let Some(request) = self.request.clone() {
            self.build_content(ui, &request, false);
        }
        true
    }

    /// The picked source (index) and whether the sound is shared too; `None` unless this is
    /// the screen picker with something to pick.
    pub fn picked(&self) -> Option<(usize, bool)> {
        let p = self.picker.as_ref()?;
        (!p.data.sources.is_empty()).then_some((p.selected, p.audio && p.data.audio_asked))
    }

    /// Whether the shown prompt is the screen picker.
    pub fn is_picker(&self) -> bool {
        self.picker.is_some()
    }

    fn build_picker(&mut self, ui: &mut Ui, request: &Request) {
        let root = self.root;
        let Some(picker) = self.picker.as_ref() else { return };
        let (tw, th) = picker.data.thumbnail;
        let header = ui.add(root, Widget::container(), fixed(HEADER, 10.0));
        ui.add(header, Widget::icon(Icon::Screen, 22.0).with_paint(Paint::Accent), Style::default());
        let titles = ui.add(header, Widget::container(), Style { flex_grow: 1.0, ..column(0.0) });
        let mut style = ui.style(titles);
        style.min_size = Size { width: length(0.0), height: auto() };
        ui.set_style(titles, style);
        let host = crate::url::display_host(&request.origin);
        let host = if host.is_empty() { request.origin.clone() } else { host };
        ui.add_label(titles, &host, TextStyle::BodyEmphasis, shrinking());
        secondary(ui, titles, tr("wants to share a screen or window:"), TextStyle::Footnote);
        close_button(ui, header);
        let mut height = HEADER;

        let count = picker.data.sources.len();
        if count == 0 {
            let row =
                ui.add(root, Widget::container(), Style { padding: padding(4.0, 0.0), ..fixed(NOTE, 0.0) });
            let label =
                secondary(ui, row, tr("No screens or windows to share were found."), TextStyle::Footnote);
            ui.set_label_lines(label, 2);
            height += ROW_GAP + NOTE;
        } else {
            let grid = ui.add(
                root,
                Widget::container(),
                Style {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    gap: Size { width: length(CARD_GAP), height: length(CARD_GAP) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            for (i, source) in picker.data.sources.iter().enumerate() {
                let selected = i == picker.selected;
                let background = if selected {
                    Background::Fill(Paint::Accent, 0.28)
                } else {
                    Background::Fill(Paint::Label, 0.06)
                };
                let card = ui.add(
                    grid,
                    Widget::container()
                        .with_action(ActionId(SOURCE_BASE + i as u32))
                        .with_radius(CARD_RADIUS)
                        .with_background(background),
                    Style {
                        size: Size { width: length(tw + 2.0 * CARD_PADDING), height: auto() },
                        padding: uniform(CARD_PADDING),
                        flex_shrink: 0.0,
                        ..column(6.0)
                    },
                );
                let thumb = ui.add(
                    card,
                    Widget::image()
                        .with_radius(THUMB_RADIUS)
                        .with_background(Background::Fill(Paint::Label, 0.1)),
                    Style {
                        size: Size { width: length(tw), height: length(th) },
                        justify_content: JustifyContent::CENTER,
                        align_items: AlignItems::CENTER,
                        ..Default::default()
                    },
                );
                match &source.image {
                    Some(image) => ui.set_image(thumb, Some(image.clone())),
                    None => {
                        ui.add(
                            thumb,
                            Widget::icon(if source.screen { Icon::Screen } else { Icon::Globe }, 28.0)
                                .with_paint(Paint::Secondary),
                            Style::default(),
                        );
                    }
                }
                let line = ui.add(
                    card,
                    Widget::container(),
                    Style { size: Size { width: auto(), height: length(CARD_LABEL) }, ..row(4.0) },
                );
                if selected {
                    ui.add(line, Widget::icon(Icon::Check, 12.0).with_paint(Paint::Label), Style::default());
                }
                ui.add_label(line, &source.name, TextStyle::Footnote, shrinking());
            }
            let rows = count.div_ceil(PICKER_COLUMNS);
            let card_height = th + 2.0 * CARD_PADDING + 6.0 + CARD_LABEL;
            height += ROW_GAP + rows as f32 * card_height + (rows as f32 - 1.0) * CARD_GAP;
        }

        if picker.data.audio_asked && count > 0 {
            let sound = ui.add(
                root,
                Widget::container().with_action(PICK_AUDIO).with_radius(CAPSULE),
                Style { padding: padding(4.0, 0.0), ..fixed(SOUND_ROW, 8.0) },
            );
            let on = picker.audio;
            ui.add(
                sound,
                Widget::icon(if on { Icon::Check } else { Icon::Volume }, 16.0).with_paint(if on {
                    Paint::Accent
                } else {
                    Paint::Secondary
                }),
                Style::default(),
            );
            ui.add_label(sound, tr("Share the sound of your computer"), TextStyle::Callout, shrinking());
            height += ROW_GAP + SOUND_ROW;
        }

        let buttons = ui.add(root, Widget::container(), fixed(BUTTON, 8.0));
        capsule_button(ui, buttons, DENY, tr("Cancel"), false);
        if count > 0 {
            capsule_button(ui, buttons, ALLOW, tr("Share"), true);
        }
        height += ROW_GAP + BUTTON;
        self.height = height;
    }

    fn build_content(&mut self, ui: &mut Ui, request: &Request, picker: bool) {
        let root = self.root;
        ui.clear_children(root);
        if self.picker.is_some() {
            self.build_picker(ui, request);
            return;
        }
        let screen = request.kinds.contains(&PermissionKind::ScreenShare);
        let first = request.kinds.first().copied().unwrap_or(PermissionKind::Microphone);

        let header = ui.add(root, Widget::container(), fixed(HEADER, 10.0));
        ui.add(header, Widget::icon(kind_icon(first), 22.0).with_paint(Paint::Accent), Style::default());
        let titles = ui.add(header, Widget::container(), Style { flex_grow: 1.0, ..column(0.0) });
        let mut style = ui.style(titles);
        style.min_size = Size { width: length(0.0), height: auto() };
        ui.set_style(titles, style);
        let host = crate::url::display_host(&request.origin);
        let host = if host.is_empty() { request.origin.clone() } else { host };
        ui.add_label(titles, &host, TextStyle::BodyEmphasis, shrinking());
        secondary(ui, titles, tr("wants to:"), TextStyle::Footnote);
        close_button(ui, header);
        let mut height = HEADER;

        for &kind in &request.kinds {
            // A shared screen's sound is said in the screen's line.
            if kind == PermissionKind::ScreenAudio && screen {
                continue;
            }
            let line = ui.add(
                root,
                Widget::container(),
                Style { padding: padding(4.0, 0.0), ..fixed(KIND_LINE, 10.0) },
            );
            ui.add(line, Widget::icon(kind_icon(kind), 16.0).with_paint(Paint::Secondary), Style::default());
            let text = if kind == PermissionKind::ScreenShare
                && request.kinds.contains(&PermissionKind::ScreenAudio)
            {
                tr("Share your screen and its sound")
            } else {
                tr(kind_request(kind))
            };
            ui.add_label(line, text, TextStyle::Callout, shrinking());
            height += ROW_GAP + KIND_LINE;
        }

        let note = if screen {
            Some(if picker {
                tr("Your desktop asks next which screen or window to share.")
            } else {
                tr("The whole screen is shared, including other windows.")
            })
        } else if request.private {
            Some(tr("Decisions in private tabs are forgotten when Axion quits."))
        } else {
            None
        };
        if let Some(note) = note {
            let row =
                ui.add(root, Widget::container(), Style { padding: padding(4.0, 0.0), ..fixed(NOTE, 0.0) });
            let label = secondary(ui, row, note, TextStyle::Footnote);
            ui.set_label_lines(label, 2);
            height += ROW_GAP + NOTE;
        }

        if screen {
            let buttons = ui.add(root, Widget::container(), fixed(BUTTON, 8.0));
            capsule_button(ui, buttons, DENY, tr("Don't Allow"), false);
            capsule_button(ui, buttons, ALLOW, tr("Share"), true);
            height += ROW_GAP + BUTTON;
        } else if offers_visit(&request.kinds) {
            let first_row = ui.add(root, Widget::container(), fixed(BUTTON, 8.0));
            capsule_button(ui, first_row, VISIT, tr("Allow While Visiting"), true);
            let buttons = ui.add(root, Widget::container(), fixed(BUTTON, 8.0));
            capsule_button(ui, buttons, DENY, tr("Don't Allow"), false);
            capsule_button(ui, buttons, ALLOW, tr("Always Allow"), false);
            height += 2.0 * (ROW_GAP + BUTTON);
        } else {
            let buttons = ui.add(root, Widget::container(), fixed(BUTTON, 8.0));
            capsule_button(ui, buttons, DENY, tr("Don't Allow"), false);
            capsule_button(ui, buttons, ALLOW, tr("Allow"), true);
            height += ROW_GAP + BUTTON;
        }
        self.height = height;
    }

    /// Per frame: follows the address field, closes when hidden, animates.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32, address: Frame) {
        let moved = address != self.address;
        self.address = address;
        if self.shown.is_none() {
            if self.popover.is_open() {
                self.popover.close(ui, config);
            }
        } else if (moved || !self.popover.is_open()) && address.size.x > 0.0 {
            self.open(ui, config);
        }
        if self.popover.animate(ui, dt) == Phase::Closed && self.shown.is_none() {
            ui.clear_children(self.root);
        }
    }

    /// The choice a click in the prompt means.
    pub fn action(&self, id: ActionId) -> Option<Choice> {
        match id {
            CLOSE => Some(Choice::Dismiss),
            DENY => Some(Choice::Block),
            ALLOW => Some(Choice::Allow),
            VISIT => Some(Choice::AllowWhileVisiting),
            _ => None,
        }
    }

    /// Whether the action id is one of the prompt's or the capture popover's.
    #[cfg(test)]
    pub fn owns(id: ActionId) -> bool {
        (ACTION_BASE..=STOP_ALL.0).contains(&id.0)
            || (SOURCE_BASE..SOURCE_BASE + MAX_SOURCES as u32).contains(&id.0)
    }
}

/// What a click in the capture popover asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureAction {
    Stop(PermissionKind),
    StopAll,
}

const CAPTURE_KINDS: [PermissionKind; 3] =
    [PermissionKind::Microphone, PermissionKind::Camera, PermissionKind::ScreenShare];

/// The popover of the address field's recording indicator.
pub struct CapturePopover {
    popover: Popover,
    root: NodeId,
    view: Option<ViewId>,
}

impl CapturePopover {
    pub fn build(ui: &mut Ui, bar: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            bar,
            RADIUS,
            Style {
                size: Size { width: length(CAPTURE_WIDTH), height: auto() },
                padding: uniform(PADDING),
                ..column(ROW_GAP)
            },
        );
        let root = popover.root();
        Self { popover, root, view: None }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    /// The tab it is about while open.
    pub fn view(&self) -> Option<ViewId> {
        self.view.filter(|_| self.is_open())
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }

    /// Opens below `anchor` (the indicator) for what `tab` records (its view, host and
    /// capture), or refreshes it; closes if nothing is recorded.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        tab: (ViewId, &str, Capture),
    ) {
        let (view, host, capture) = tab;
        if !capture.any() {
            self.close(ui, config);
            return;
        }
        ui.clear_children(self.root);
        let header = ui.add(self.root, Widget::container(), fixed(HEADER, 10.0));
        if let Some((icon, color)) = capture_icon(capture) {
            ui.add(header, Widget::icon(icon, 22.0).with_paint(Paint::Fixed(color)), Style::default());
        }
        let titles = ui.add(header, Widget::container(), Style { flex_grow: 1.0, ..column(0.0) });
        ui.add_label(titles, host, TextStyle::BodyEmphasis, shrinking());
        secondary(ui, titles, tr("is recording:"), TextStyle::Footnote);
        let mut height = HEADER;
        let mut rows = 0;
        for (i, kind) in CAPTURE_KINDS.into_iter().enumerate() {
            let (on, name, color) = match kind {
                PermissionKind::Microphone => (capture.microphone, tr("Microphone"), MICROPHONE_COLOR),
                PermissionKind::Camera => (capture.camera, tr("Camera"), CAMERA_COLOR),
                _ => (capture.screen, tr("Screen"), SCREEN_COLOR),
            };
            if !on {
                continue;
            }
            let row = ui.add(
                self.root,
                Widget::container(),
                Style { padding: padding(4.0, 0.0), ..fixed(CAPTURE_ROW, 10.0) },
            );
            ui.add(
                row,
                Widget::icon(kind_icon(kind), 18.0).with_paint(Paint::Fixed(color)),
                Style::default(),
            );
            ui.add_label(row, name, TextStyle::Body, Style { flex_grow: 1.0, ..shrinking() });
            pill(ui, row, ActionId(STOP_BASE + i as u32), tr("Stop"));
            height += ROW_GAP + CAPTURE_ROW;
            rows += 1;
        }
        if rows > 1 {
            let buttons = ui.add(self.root, Widget::container(), fixed(BUTTON, 8.0));
            capsule_button(ui, buttons, STOP_ALL, tr("Stop All"), true);
            height += ROW_GAP + BUTTON;
        }
        let height = height + 2.0 * PADDING;
        let left = (anchor.origin.x + anchor.size.x / 2.0 - CAPTURE_WIDTH / 2.0)
            .clamp(MARGIN, (window_width - CAPTURE_WIDTH - MARGIN).max(MARGIN));
        let target =
            Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, CAPTURE_WIDTH, height);
        self.view = Some(view);
        self.popover.open(ui, config, anchor, target);
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        if self.popover.phase() != Phase::Closed {
            self.popover.close(ui, config);
        }
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    pub fn action(&self, id: ActionId) -> Option<CaptureAction> {
        if id == STOP_ALL {
            return Some(CaptureAction::StopAll);
        }
        let i = id.0.checked_sub(STOP_BASE)? as usize;
        CAPTURE_KINDS.get(i).map(|k| CaptureAction::Stop(*k))
    }
}

/// Width of the picker: the thumbnails' columns, their padding and gaps.
fn picker_width(thumbnail_width: f32) -> f32 {
    PICKER_COLUMNS as f32 * (thumbnail_width + 2.0 * CARD_PADDING)
        + (PICKER_COLUMNS as f32 - 1.0) * CARD_GAP
        + 2.0 * PADDING
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
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

/// A row of fixed height.
fn fixed(height: f32, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row(gap) }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, shrinking());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

fn capsule_button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) {
    let background =
        if primary { Background::Fill(Paint::Accent, 1.0) } else { Background::Fill(Paint::Label, 0.08) };
    let button = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            flex_basis: length(0.0),
            size: Size { width: auto(), height: length(BUTTON) },
            padding: padding(14.0, 0.0),
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(button, text, TextStyle::BodyEmphasis, shrinking());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(if primary { Paint::Fixed(Color::rgb(0xFFFFFF)) } else { Paint::Label });
    }
}

fn pill(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str) {
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_radius(CAPSULE)
            .with_background(Background::Fill(Paint::Fixed(STOP_COLOR), 1.0)),
        Style {
            size: Size { width: auto(), height: length(32.0) },
            padding: padding(14.0, 0.0),
            flex_shrink: 0.0,
            ..row(0.0)
        },
    );
    let label = ui.add_label(button, text, TextStyle::BodyEmphasis, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Fixed(Color::rgb(0xFFFFFF)));
    }
}

fn close_button(ui: &mut Ui, parent: NodeId) {
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(CLOSE)
            .with_radius(CAPSULE)
            .with_background(Background::Fill(Paint::Label, 0.06)),
        Style {
            size: Size { width: length(28.0), height: length(28.0) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(0.0)
        },
    );
    ui.add(button, Widget::icon(Icon::Close, 14.0).with_paint(Paint::Secondary), Style::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_indicator_shows_the_most_telling_capture() {
        let all = Capture { microphone: true, camera: true, screen: true };
        assert_eq!(capture_icon(all), Some((Icon::Screen, SCREEN_COLOR)));
        let call = Capture { microphone: true, camera: true, screen: false };
        assert_eq!(capture_icon(call), Some((Icon::Camera, CAMERA_COLOR)));
        let voice = Capture { microphone: true, ..Capture::default() };
        assert_eq!(capture_icon(voice), Some((Icon::Microphone, MICROPHONE_COLOR)));
        assert_eq!(capture_icon(Capture::default()), None);
    }
}
