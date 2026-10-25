//! Passwords: the sheet that manages the vault (search, a list grouped by kind, details with
//! copy and reveal, editing and a password generator), the unlock card shown while the vault is
//! locked (styled like the private browsing one), and the small glass prompt under the address
//! bar that offers to fill a saved login or to save a new one.
//!
//! Secrets stay out of logs; only the user's clicks put them on the clipboard or into a page.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_engine::ViewId;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use axion_vault::autofill::PageEvent;
use axion_vault::{ItemData, Kind, Login, Vault, VaultError, site};
use glam::Vec2;
use zeroize::Zeroizing;

const BASE: u32 = 12_000;
const CLOSE: ActionId = ActionId(BASE);
const LOCK: ActionId = ActionId(BASE + 1);
const ADD: ActionId = ActionId(BASE + 2);
const UNLOCK: ActionId = ActionId(BASE + 3);
const CANCEL_UNLOCK: ActionId = ActionId(BASE + 4);
const EDIT: ActionId = ActionId(BASE + 5);
const DELETE: ActionId = ActionId(BASE + 6);
const SAVE: ActionId = ActionId(BASE + 7);
const CANCEL_EDIT: ActionId = ActionId(BASE + 8);
const GENERATE: ActionId = ActionId(BASE + 9);
const KIND_BASE: u32 = BASE + 10;
const REVEAL_BASE: u32 = BASE + 20;
const COPY_BASE: u32 = BASE + 40;
const ITEM_BASE: u32 = BASE + 100;
const PROMPT_CLOSE: ActionId = ActionId(BASE + 1000);
const PROMPT_SAVE: ActionId = ActionId(BASE + 1001);
const PROMPT_UNLOCK: ActionId = ActionId(BASE + 1002);
const PROMPT_FILL_BASE: u32 = BASE + 1010;
const LAST: u32 = BASE + 1100;
const MAX_FIELDS: u32 = 20;
/// Rows the list shows at most (one action id each).
const MAX_ROWS: usize = (PROMPT_CLOSE.0 - ITEM_BASE) as usize;

const SHEET_MAX: Vec2 = Vec2::new(880.0, 600.0);
const SHEET_MARGIN: f32 = 32.0;
const SHEET_RADIUS: f32 = 30.0;
const LIST_WIDTH: f32 = 300.0;
const LIST_ROW: f32 = 52.0;
const FIELD_ROW: f32 = 58.0;
const EDIT_ROW: f32 = 50.0;
const EDIT_LABEL_WIDTH: f32 = 130.0;
const GROUP_RADIUS: f32 = 14.0;
const GROUP_INSET: f32 = 16.0;
const BACKDROP_DIM: f32 = 0.28;
const ALERT_WIDTH: f32 = 340.0;
const ALERT_RADIUS: f32 = 22.0;
const ALERT_START_SCALE: f32 = 0.88;
const PROMPT_WIDTH: f32 = 320.0;
const PROMPT_RADIUS: f32 = 22.0;
const PROMPT_GAP: f32 = 10.0;
/// Most logins the fill prompt lists.
const PROMPT_LOGINS: usize = 3;
const MIN_MASTER_LENGTH: usize = 8;
const GENERATOR_RANGE: (f32, f32) = (8.0, 64.0);
const HIDDEN: &str = "••••••••••";
const ERROR: Color = Color::rgb(0xD0362B);

fn kind_icon(kind: Kind) -> Icon {
    match kind {
        Kind::Login => Icon::Key,
        Kind::Card => Icon::Card,
        Kind::Address => Icon::Home,
        Kind::Note => Icon::Note,
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Login => "Password",
        Kind::Card => "Card",
        Kind::Address => "Address",
        Kind::Note => "Note",
    }
}

/// The fields of each kind: label and whether it is a secret (hidden until revealed).
fn fields(kind: Kind) -> &'static [(&'static str, bool)] {
    match kind {
        Kind::Login => &[("Website", false), ("User name", false), ("Password", true), ("Notes", false)],
        Kind::Card => &[
            ("Name", false),
            ("Cardholder", false),
            ("Card number", true),
            ("Expiry (MM/YY)", false),
            ("Security code", true),
        ],
        Kind::Address => &[
            ("Full name", false),
            ("Street", false),
            ("City", false),
            ("ZIP code", false),
            ("Country", false),
            ("Email", false),
            ("Phone", false),
        ],
        Kind::Note => &[("Title", false), ("Note", true)],
    }
}

fn field_mut(data: &mut ItemData, i: usize) -> Option<&mut String> {
    Some(match data {
        ItemData::Login(l) => {
            [&mut l.site, &mut l.username, &mut l.password, &mut l.notes].into_iter().nth(i)?
        }
        ItemData::Card(c) => {
            [&mut c.label, &mut c.cardholder, &mut c.number, &mut c.expiry, &mut c.cvc].into_iter().nth(i)?
        }
        ItemData::Address(a) => {
            [&mut a.name, &mut a.street, &mut a.city, &mut a.zip, &mut a.country, &mut a.email, &mut a.phone]
                .into_iter()
                .nth(i)?
        }
        ItemData::Note(n) => [&mut n.title, &mut n.text].into_iter().nth(i)?,
    })
}

fn field(data: &ItemData, i: usize) -> &str {
    let value = match data {
        ItemData::Login(l) => [&l.site, &l.username, &l.password, &l.notes].get(i).copied(),
        ItemData::Card(c) => [&c.label, &c.cardholder, &c.number, &c.expiry, &c.cvc].get(i).copied(),
        ItemData::Address(a) => {
            [&a.name, &a.street, &a.city, &a.zip, &a.country, &a.email, &a.phone].get(i).copied()
        }
        ItemData::Note(n) => [&n.title, &n.text].get(i).copied(),
    };
    value.map_or("", String::as_str)
}

/// What the browser has to do after an event.
pub enum Outcome {
    Nothing,
    /// A password setting changed (generator options); save the config.
    ConfigChanged,
    /// Events of focusing a field, for the browser to handle.
    Events(Vec<UiEvent>),
    /// Run this script in the view's top frame (fills a login the user picked).
    Fill {
        view: ViewId,
        script: Zeroizing<String>,
    },
}

#[derive(Clone, PartialEq, Eq)]
enum Detail {
    Empty,
    View(String),
    /// Editing an item (`None`: a new one).
    Edit {
        id: Option<String>,
        kind: Kind,
    },
}

/// Credentials the user submitted on a page, waiting for "Save".
struct PendingSave {
    url: String,
    username: String,
    password: Zeroizing<String>,
}

enum PromptState {
    Fill {
        view: ViewId,
        url: String,
        ids: Vec<String>,
    },
    /// A sign-in form while the vault is locked: unlock right here, then pick a login.
    Unlock {
        view: ViewId,
        url: String,
    },
    Save(PendingSave),
}

/// The glass prompt under the address bar.
struct Prompt {
    root: NodeId,
    state: Option<PromptState>,
    /// The master password field and its error line (unlock prompt only).
    field: Option<NodeId>,
    error: Option<NodeId>,
    appear: Animated<f32>,
}

/// The vault, its sheet and the autofill prompt.
pub struct Passwords {
    vault: Option<Vault>,
    root: NodeId,
    sheet: NodeId,
    list_viewport: NodeId,
    list: NodeId,
    list_scroll: f32,
    search: NodeId,
    query: String,
    detail: NodeId,
    alert_layer: NodeId,
    alert: NodeId,
    alert_title: NodeId,
    alert_text: NodeId,
    /// A second line under `alert_text` (labels don't wrap).
    alert_note: NodeId,
    alert_field: NodeId,
    confirm_box: NodeId,
    confirm_field: NodeId,
    alert_error: NodeId,
    alert_button: NodeId,
    alert_appear: Animated<f32>,
    open: bool,
    /// Item ids of the list's rows, by row.
    list_ids: Vec<String>,
    shown: Detail,
    revealed: Vec<bool>,
    /// The field whose copy button shows a check mark.
    copied_field: Option<usize>,
    confirm_delete: bool,
    edit_fields: Vec<NodeId>,
    reveal_icons: Vec<(usize, NodeId)>,
    edit_error: Option<NodeId>,
    generator_length: Option<NodeId>,
    generator_length_label: Option<NodeId>,
    generator_symbols: Option<NodeId>,
    /// What was copied, and when to clear it from the clipboard.
    copied: Option<(Instant, Zeroizing<String>)>,
    /// Credentials to save once the vault is unlocked.
    pending: Option<PendingSave>,
    prompt: Prompt,
    /// Sites whose unlock prompt was dismissed ("Not Now"); not offered again this session.
    unlock_dismissed: HashSet<String>,
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

fn grow() -> Style {
    Style {
        flex_grow: 1.0,
        flex_shrink: 1.0,
        min_size: Size { width: length(0.0), height: auto() },
        ..Default::default()
    }
}

fn painted(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle, paint: Paint) -> NodeId {
    let label = ui.add_label(parent, text, style, Style::default());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(paint);
    }
    label
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    painted(ui, parent, text, style, Paint::Secondary)
}

fn capsule_button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) -> NodeId {
    let background =
        if primary { Background::Fill(Paint::Accent, 1.0) } else { Background::Fill(Paint::Label, 0.08) };
    let button = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            size: Size { width: auto(), height: length(40.0) },
            padding: padding(22.0, 0.0),
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let paint = if primary { Paint::Fixed(Color::rgb(0xFFFFFF)) } else { Paint::Label };
    painted(ui, button, text, TextStyle::BodyEmphasis, paint)
}

/// A small capsule button (accent tint, or red for destructive ones).
fn pill(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, destructive: bool) {
    let color = if destructive { Paint::Fixed(ERROR) } else { Paint::Accent };
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_radius(CAPSULE)
            .with_background(Background::Fill(color, 0.14)),
        Style { size: Size { width: auto(), height: length(34.0) }, padding: padding(16.0, 0.0), ..row(0.0) },
    );
    painted(ui, button, text, TextStyle::Callout, color);
}

/// A round icon button.
fn icon_button(ui: &mut Ui, parent: NodeId, action: ActionId, icon: Icon, size: f32) -> NodeId {
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_radius(CAPSULE)
            .with_background(Background::Fill(Paint::Label, 0.06)),
        Style {
            size: Size { width: length(size), height: length(size) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(0.0)
        },
    );
    ui.add(button, Widget::icon(icon, size * 0.5).with_paint(Paint::Accent), Style::default())
}

/// The kind's icon in a tinted circle.
fn badge(ui: &mut Ui, parent: NodeId, kind: Kind, size: f32) {
    let circle = ui.add(
        parent,
        Widget::container().with_radius(CAPSULE).with_background(Background::Fill(Paint::Accent, 0.14)),
        Style {
            size: Size { width: length(size), height: length(size) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(0.0)
        },
    );
    ui.add(circle, Widget::icon(kind_icon(kind), size * 0.55).with_paint(Paint::Accent), Style::default());
}

/// A grey rounded box holding a text field; returns the field.
fn field_box(ui: &mut Ui, parent: NodeId, placeholder: &str, layout: Style) -> NodeId {
    let boxed = ui.add(
        parent,
        Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(12.0),
        Style {
            size: Size { width: auto(), height: length(38.0) },
            padding: padding(12.0, 0.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::CENTER,
            ..layout
        },
    );
    ui.add_text_field(
        boxed,
        placeholder,
        TextStyle::Body,
        Style { flex_grow: 1.0, size: Size { width: auto(), height: length(22.0) }, ..Default::default() },
    )
}

fn spring(config: &Config) -> Spring {
    if config.glass.reduce_motion {
        Spring::new(0.15, 1.0)
    } else {
        Spring::new(config.motion.snappy.response * config.motion.speed, config.motion.snappy.damping)
    }
}

/// Scales `node` from its layout frame by the appear value `k` (0–1).
fn scale_in(ui: &mut Ui, node: NodeId, k: f32) {
    let frame = ui.frame(node);
    if frame.size.x > 0.0 {
        let scale = ALERT_START_SCALE + (1.0 - ALERT_START_SCALE) * k;
        let size = frame.size * scale;
        let origin = frame.origin + (frame.size - size) / 2.0;
        ui.set_visual_frame(node, Some(Frame { origin, size: size.max(Vec2::ONE) }));
    }
}

impl Passwords {
    /// Builds the sheet and prompt and opens the vault at `path` (locked).
    pub fn build(ui: &mut Ui, path: &Path) -> Self {
        let vault = Vault::open(path).inspect_err(|e| tracing::error!("passwords unavailable: {e}")).ok();
        let prompt = Prompt::build(ui);
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .with_action(CLOSE),
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
            Style { position: Position::Absolute, flex_direction: FlexDirection::Row, ..Default::default() },
        );

        // Left: title, search, list.
        let side = ui.add(
            sheet,
            Widget::container(),
            Style {
                size: Size { width: length(LIST_WIDTH), height: auto() },
                flex_shrink: 0.0,
                padding: padding(14.0, 22.0),
                ..column(12.0)
            },
        );
        let header = ui.add(side, Widget::container(), Style { padding: padding(8.0, 0.0), ..row(8.0) });
        ui.add_label(header, "Passwords", TextStyle::Title1, grow());
        icon_button(ui, header, LOCK, Icon::Lock, 32.0);
        icon_button(ui, header, ADD, Icon::Plus, 32.0);
        let search_box = ui.add(
            side,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(CAPSULE),
            Style {
                size: Size { width: auto(), height: length(36.0) },
                flex_shrink: 0.0,
                padding: padding(12.0, 0.0),
                ..row(8.0)
            },
        );
        ui.add(search_box, Widget::icon(Icon::Search, 16.0).with_paint(Paint::Secondary), Style::default());
        let search = ui.add_text_field(
            search_box,
            "Search",
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        let list_viewport = ui.add(
            side,
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
            list_viewport,
            Widget::container(),
            Style { flex_shrink: 0.0, size: Size { width: percent(1.0), height: auto() }, ..column(2.0) },
        );
        ui.add(
            sheet,
            Widget::container().with_background(Background::Fill(Paint::Hairline, 1.0)),
            Style {
                size: Size { width: length(1.0), height: auto() },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        let detail = ui.add(
            sheet,
            Widget::container().with_clip(),
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0),
                padding: padding(28.0, 26.0),
                overflow: axion_ui::taffy::Point {
                    x: axion_ui::taffy::Overflow::Hidden,
                    y: axion_ui::taffy::Overflow::Hidden,
                },
                ..column(4.0)
            },
        );

        // The unlock / set-up card, centred over the dimmed browser.
        let alert_layer = ui.add(
            root,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let alert = ui.add(
            alert_layer,
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Thick))
                .with_radius(ALERT_RADIUS),
            Style {
                size: Size { width: length(ALERT_WIDTH), height: auto() },
                padding: Rect {
                    left: length(20.0),
                    right: length(20.0),
                    top: length(20.0),
                    bottom: length(18.0),
                },
                ..column(8.0)
            },
        );
        ui.add(alert, Widget::icon(Icon::Key, 30.0).with_paint(Paint::Accent), Style::default());
        let alert_title = ui.add_label(alert, "Unlock Passwords", TextStyle::BodyEmphasis, Style::default());
        let alert_text = secondary(ui, alert, "", TextStyle::Footnote);
        let alert_note = secondary(ui, alert, "", TextStyle::Footnote);
        let field_style = Style {
            size: Size { width: percent(1.0), height: length(40.0) },
            margin: Rect { left: length(0.0), right: length(0.0), top: length(6.0), bottom: length(0.0) },
            ..row(0.0)
        };
        let alert_field = field_box(ui, alert, "Master password", field_style.clone());
        ui.set_secure(alert_field, true);
        let confirm_field = field_box(ui, alert, "Repeat password", field_style);
        ui.set_secure(confirm_field, true);
        let confirm_box = ui.parent(confirm_field).unwrap_or(confirm_field);
        let alert_error = painted(ui, alert, "", TextStyle::Footnote, Paint::Fixed(ERROR));
        ui.set_visible(alert_error, false);
        let buttons = ui.add(
            alert,
            Widget::container(),
            Style {
                margin: Rect { left: length(0.0), right: length(0.0), top: length(8.0), bottom: length(0.0) },
                ..row(10.0)
            },
        );
        capsule_button(ui, buttons, CANCEL_UNLOCK, "Cancel", false);
        let alert_button = capsule_button(ui, buttons, UNLOCK, "Unlock", true);
        ui.set_visible(root, false);
        Self {
            vault,
            root,
            sheet,
            list_viewport,
            list,
            list_scroll: 0.0,
            search,
            query: String::new(),
            detail,
            alert_layer,
            alert,
            alert_title,
            alert_text,
            alert_note,
            alert_field,
            confirm_box,
            confirm_field,
            alert_error,
            alert_button,
            alert_appear: Animated::new(0.0, Spring::SNAPPY),
            open: false,
            list_ids: Vec::new(),
            shown: Detail::Empty,
            revealed: Vec::new(),
            copied_field: None,
            confirm_delete: false,
            edit_fields: Vec::new(),
            reveal_icons: Vec::new(),
            edit_error: None,
            generator_length: None,
            generator_length_label: None,
            generator_symbols: None,
            copied: None,
            pending: None,
            prompt,
            unlock_dismissed: HashSet::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_unlocked(&self) -> bool {
        self.vault.as_ref().is_some_and(Vault::is_unlocked)
    }

    /// Whether `p` (logical px) is on the sheet or the unlock card (the backdrop closes).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        (ui.is_visible(self.sheet) && ui.frame(self.sheet).contains(p))
            || (ui.is_visible(self.alert_layer) && ui.frame(self.alert).contains(p))
    }

    /// Whether this event belongs to the passwords UI.
    pub fn wants(&self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Clicked { action, .. } => (BASE..LAST).contains(&action.0),
            UiEvent::Submitted { node, .. } | UiEvent::ValueChanged { node, .. } => self.owns(*node),
            _ => false,
        }
    }

    fn owns(&self, node: NodeId) -> bool {
        [self.alert_field, self.confirm_field, self.search].contains(&node)
            || self.prompt.field == Some(node)
            || self.edit_fields.contains(&node)
            || [self.generator_length, self.generator_symbols].contains(&Some(node))
    }

    pub fn open(&mut self, ui: &mut Ui, config: &Config, window: Vec2) -> Outcome {
        self.open = true;
        self.prompt.hide();
        ui.set_visible(self.root, true);
        self.resize(ui, window);
        self.refresh(ui, config)
    }

    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
        self.confirm_delete = false;
        // Edits in progress are dropped; what's typed doesn't stay in hidden fields.
        if matches!(self.shown, Detail::Edit { .. }) {
            self.shown = Detail::Empty;
        }
        self.clear_detail(ui);
        ui.set_field_text(self.alert_field, "");
        ui.set_field_text(self.confirm_field, "");
        ui.set_visible(self.root, false);
    }

    pub fn resize(&mut self, ui: &mut Ui, window: Vec2) {
        let size = (window - Vec2::splat(2.0 * SHEET_MARGIN)).min(SHEET_MAX).max(Vec2::new(480.0, 360.0));
        let origin = ((window - size) / 2.0).max(Vec2::ZERO);
        let mut style = ui.style(self.sheet);
        style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
        style.size = Size { width: length(size.x), height: length(size.y) };
        ui.set_style(self.sheet, style);
    }

    /// Scrolls the list by `dy` logical px (wheel: positive scrolls up).
    pub fn scroll_by(&mut self, ui: &mut Ui, dy: f32) {
        let max = (ui.frame(self.list).size.y - ui.frame(self.list_viewport).size.y).max(0.0);
        self.list_scroll = (self.list_scroll - dy).clamp(0.0, max);
        let mut style = ui.style(self.list);
        style.margin = Rect {
            left: length(0.0),
            right: length(0.0),
            top: length(-self.list_scroll),
            bottom: length(0.0),
        };
        ui.set_style(self.list, style);
    }

    /// Shows the unlock card or the sheet, whichever fits the vault's state.
    fn refresh(&mut self, ui: &mut Ui, config: &Config) -> Outcome {
        if !self.open {
            return Outcome::Nothing;
        }
        if self.is_unlocked() {
            ui.set_visible(self.alert_layer, false);
            ui.set_visible(self.sheet, true);
            self.rebuild_list(ui);
            self.show_detail(ui, config);
            return Outcome::Nothing;
        }
        ui.set_visible(self.sheet, false);
        ui.set_visible(self.alert_layer, true);
        ui.set_visible(self.alert_error, false);
        let set_up = self.vault.as_ref().is_some_and(Vault::is_set_up);
        let (title, text, note, button) = match &self.vault {
            None => ("Passwords Unavailable", "The password vault couldn't be opened.", "", "Close"),
            Some(_) if set_up => {
                ("Unlock Passwords", "Enter your master password to continue.", "", "Unlock")
            }
            Some(_) => (
                "Set Up Passwords",
                "Choose a master password to encrypt them.",
                "Axion can't recover it if you forget it.",
                "Create",
            ),
        };
        ui.set_text(self.alert_title, title);
        ui.set_text(self.alert_text, text);
        ui.set_text(self.alert_note, note);
        ui.set_visible(self.alert_note, !note.is_empty());
        ui.set_text(self.alert_button, button);
        let has_vault = self.vault.is_some();
        ui.set_visible(ui.parent(self.alert_field).unwrap_or(self.alert_field), has_vault);
        ui.set_visible(self.confirm_box, has_vault && !set_up);
        self.alert_appear.spring = spring(config);
        self.alert_appear.snap(0.0);
        self.alert_appear.target = 1.0;
        if has_vault { Outcome::Events(ui.focus(self.alert_field)) } else { Outcome::Nothing }
    }

    fn try_unlock(&mut self, ui: &mut Ui, config: &Config) -> Outcome {
        let Some(vault) = &mut self.vault else {
            self.close(ui);
            return Outcome::Nothing;
        };
        let password = Zeroizing::new(ui.field_text(self.alert_field).unwrap_or_default().to_owned());
        let result = if vault.is_set_up() {
            vault.unlock(&password)
        } else {
            let confirm = Zeroizing::new(ui.field_text(self.confirm_field).unwrap_or_default().to_owned());
            if password.chars().count() < MIN_MASTER_LENGTH {
                return self.alert_failed(ui, &format!("Use at least {MIN_MASTER_LENGTH} characters."));
            }
            if *password != *confirm {
                return self.alert_failed(ui, "The passwords don't match.");
            }
            vault.setup(&password)
        };
        match result {
            Ok(()) => {
                ui.set_field_text(self.alert_field, "");
                ui.set_field_text(self.confirm_field, "");
                tracing::info!("passwords unlocked");
                if let Some(pending) = self.pending.take() {
                    self.save_login(pending);
                }
                self.refresh(ui, config)
            }
            Err(VaultError::WrongPassword) => self.alert_failed(ui, "Wrong password"),
            Err(e) => {
                tracing::error!("unlocking passwords failed: {e}");
                self.alert_failed(ui, &e.to_string())
            }
        }
    }

    fn alert_failed(&mut self, ui: &mut Ui, message: &str) -> Outcome {
        ui.set_text(self.alert_error, message);
        ui.set_visible(self.alert_error, true);
        ui.set_field_text(self.alert_field, "");
        ui.set_field_text(self.confirm_field, "");
        Outcome::Events(ui.focus(self.alert_field))
    }

    /// Locks the vault now (the sheet's lock button, or auto-lock).
    pub fn lock(&mut self, ui: &mut Ui, config: &Config) -> Outcome {
        if let Some(vault) = &mut self.vault {
            vault.lock();
        }
        self.shown = Detail::Empty;
        self.list_ids.clear();
        ui.clear_children(self.list);
        self.clear_detail(ui);
        if matches!(self.prompt.state, Some(PromptState::Fill { .. })) {
            self.prompt.hide();
        }
        self.refresh(ui, config)
    }

    fn rebuild_list(&mut self, ui: &mut Ui) {
        ui.clear_children(self.list);
        self.list_ids.clear();
        let Some(vault) = &self.vault else { return };
        let selected = match &self.shown {
            Detail::View(id) | Detail::Edit { id: Some(id), .. } => Some(id.clone()),
            _ => None,
        };
        for kind in Kind::ALL {
            let items: Vec<_> = vault
                .items()
                .iter()
                .filter(|i| i.data.kind() == kind && i.data.matches(&self.query))
                .collect();
            if items.is_empty() {
                continue;
            }
            let header = secondary(ui, self.list, &kind.title().to_uppercase(), TextStyle::Caption);
            let mut style = ui.style(header);
            style.margin = Rect {
                left: length(12.0),
                right: length(0.0),
                top: length(if self.list_ids.is_empty() { 4.0 } else { 14.0 }),
                bottom: length(4.0),
            };
            ui.set_style(header, style);
            for item in items.into_iter().take(MAX_ROWS.saturating_sub(self.list_ids.len())) {
                let index = self.list_ids.len() as u32;
                let active = selected.as_deref() == Some(item.id.as_str());
                let row_node = ui.add(
                    self.list,
                    Widget::container()
                        .with_action(ActionId(ITEM_BASE + index))
                        .with_radius(12.0)
                        .with_background(if active {
                            Background::Fill(Paint::Accent, 0.14)
                        } else {
                            Background::None
                        }),
                    Style {
                        size: Size { width: auto(), height: length(LIST_ROW) },
                        flex_shrink: 0.0,
                        padding: padding(10.0, 0.0),
                        ..row(10.0)
                    },
                );
                badge(ui, row_node, kind, 30.0);
                let texts = ui.add(row_node, Widget::container(), Style { ..column(0.0) });
                let mut style = ui.style(texts);
                style.flex_grow = 1.0;
                style.min_size = Size { width: length(0.0), height: auto() };
                ui.set_style(texts, style);
                ui.add_label(texts, &item.data.title(), TextStyle::Body, Style::default());
                let subtitle = item.data.subtitle();
                if !subtitle.is_empty() {
                    secondary(ui, texts, &subtitle, TextStyle::Footnote);
                }
                self.list_ids.push(item.id.clone());
            }
        }
        if self.list_ids.is_empty() {
            let text = if self.query.trim().is_empty() { "Nothing saved yet." } else { "No matches." };
            let label = secondary(ui, self.list, text, TextStyle::Footnote);
            let mut style = ui.style(label);
            style.margin =
                Rect { left: length(12.0), right: length(12.0), top: length(8.0), bottom: length(8.0) };
            ui.set_style(label, style);
        }
    }

    fn clear_detail(&mut self, ui: &mut Ui) {
        ui.clear_children(self.detail);
        self.edit_fields.clear();
        self.reveal_icons.clear();
        self.edit_error = None;
        self.generator_length = None;
        self.generator_length_label = None;
        self.generator_symbols = None;
    }

    fn show_detail(&mut self, ui: &mut Ui, config: &Config) {
        self.clear_detail(ui);
        match self.shown.clone() {
            Detail::Empty => {
                let empty = ui.add(
                    self.detail,
                    Widget::container(),
                    Style {
                        flex_grow: 1.0,
                        justify_content: JustifyContent::CENTER,
                        align_items: AlignItems::CENTER,
                        ..column(10.0)
                    },
                );
                ui.add(empty, Widget::icon(Icon::Key, 34.0).with_paint(Paint::Secondary), Style::default());
                let none = self.vault.as_ref().is_none_or(|v| v.items().is_empty());
                let text =
                    if none { "Click + to save a password, card or address." } else { "Select an item." };
                secondary(ui, empty, text, TextStyle::Body);
            }
            Detail::View(id) => self.build_view(ui, &id),
            Detail::Edit { id, kind } => self.build_edit(ui, config, id.as_deref(), kind),
        }
    }

    fn header(&mut self, ui: &mut Ui, kind: Kind, title: &str, subtitle: &str) -> NodeId {
        let header = ui.add(
            self.detail,
            Widget::container(),
            Style {
                flex_shrink: 0.0,
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(0.0),
                    bottom: length(14.0),
                },
                ..row(14.0)
            },
        );
        badge(ui, header, kind, 48.0);
        let titles = ui.add(header, Widget::container(), Style { ..column(2.0) });
        let mut style = ui.style(titles);
        style.flex_grow = 1.0;
        style.min_size = Size { width: length(0.0), height: auto() };
        ui.set_style(titles, style);
        ui.add_label(titles, title, TextStyle::Title2, Style::default());
        if !subtitle.is_empty() {
            secondary(ui, titles, subtitle, TextStyle::Footnote);
        }
        header
    }

    /// A card of rows.
    fn group(&mut self, ui: &mut Ui) -> NodeId {
        ui.add(
            self.detail,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, 0.85))
                .with_radius(GROUP_RADIUS),
            Style { flex_shrink: 0.0, padding: padding(GROUP_INSET, 0.0), ..column(0.0) },
        )
    }

    fn separator(ui: &mut Ui, group: NodeId) {
        if !ui.children(group).is_empty() {
            ui.add(
                group,
                Widget::container().with_background(Background::Fill(Paint::Hairline, 1.0)),
                Style {
                    size: Size { width: auto(), height: length(1.0) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
        }
    }

    fn build_view(&mut self, ui: &mut Ui, id: &str) {
        let Some(item) = self.vault.as_ref().and_then(|v| v.get(id)).cloned() else {
            self.shown = Detail::Empty;
            return;
        };
        let kind = item.data.kind();
        let subtitle = jiff::Timestamp::from_second(item.modified)
            .map(|t| format!("Edited {}", t.strftime("%Y-%m-%d")))
            .unwrap_or_default();
        self.header(ui, kind, &item.data.title(), &subtitle);
        let group = self.group(ui);
        for (i, &(label, secret)) in fields(kind).iter().enumerate() {
            let value = field(&item.data, i);
            if value.trim().is_empty() {
                continue;
            }
            Self::separator(ui, group);
            let row_node = ui.add(
                group,
                Widget::container(),
                Style {
                    size: Size { width: auto(), height: length(FIELD_ROW) },
                    flex_shrink: 0.0,
                    ..row(8.0)
                },
            );
            let texts = ui.add(row_node, Widget::container(), Style { ..column(1.0) });
            let mut style = ui.style(texts);
            style.flex_grow = 1.0;
            style.min_size = Size { width: length(0.0), height: auto() };
            ui.set_style(texts, style);
            secondary(ui, texts, label, TextStyle::Footnote);
            let revealed = self.revealed.get(i).copied().unwrap_or(false);
            ui.add_label(
                texts,
                if secret && !revealed { HIDDEN } else { value },
                TextStyle::Body,
                Style::default(),
            );
            if secret {
                let icon = if revealed { Icon::EyeOff } else { Icon::Eye };
                icon_button(ui, row_node, ActionId(REVEAL_BASE + i as u32), icon, 32.0);
            }
            let icon = if self.copied_field == Some(i) { Icon::Check } else { Icon::Copy };
            icon_button(ui, row_node, ActionId(COPY_BASE + i as u32), icon, 32.0);
        }
        let buttons = ui.add(
            self.detail,
            Widget::container(),
            Style {
                flex_shrink: 0.0,
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(16.0),
                    bottom: length(0.0),
                },
                ..row(10.0)
            },
        );
        pill(ui, buttons, EDIT, "Edit", false);
        pill(ui, buttons, DELETE, if self.confirm_delete { "Click again to delete" } else { "Delete" }, true);
    }

    fn build_edit(&mut self, ui: &mut Ui, config: &Config, id: Option<&str>, kind: Kind) {
        let item = id.and_then(|id| self.vault.as_ref()?.get(id)).cloned();
        let data = item.as_ref().map_or_else(|| ItemData::empty(kind), |i| i.data.clone());
        let title = if item.is_some() {
            format!("Edit {}", kind_name(kind))
        } else {
            format!("New {}", kind_name(kind))
        };
        self.header(ui, kind, &title, "");
        if item.is_none() {
            let kinds = ui.add(
                self.detail,
                Widget::container(),
                Style {
                    flex_shrink: 0.0,
                    margin: Rect {
                        left: length(0.0),
                        right: length(0.0),
                        top: length(0.0),
                        bottom: length(12.0),
                    },
                    ..row(0.0)
                },
            );
            let options: Vec<(&str, ActionId)> = Kind::ALL
                .iter()
                .enumerate()
                .map(|(i, &k)| (kind_name(k), ActionId(KIND_BASE + i as u32)))
                .collect();
            let current = Kind::ALL.iter().position(|&k| k == kind);
            crate::settings::segmented(ui, kinds, &options, current);
        }
        let group = self.group(ui);
        self.revealed.resize(fields(kind).len(), false);
        for (i, &(label, secret)) in fields(kind).iter().enumerate() {
            Self::separator(ui, group);
            let row_node = ui.add(
                group,
                Widget::container(),
                Style {
                    size: Size { width: auto(), height: length(EDIT_ROW) },
                    flex_shrink: 0.0,
                    ..row(10.0)
                },
            );
            ui.add_label(
                row_node,
                label,
                TextStyle::Body,
                Style {
                    size: Size { width: length(EDIT_LABEL_WIDTH), height: auto() },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            let field_node = field_box(ui, row_node, label, grow());
            ui.set_field_text(field_node, field(&data, i));
            if secret {
                let revealed = self.revealed[i];
                ui.set_secure(field_node, !revealed);
                let icon = if revealed { Icon::EyeOff } else { Icon::Eye };
                let icon_node = icon_button(ui, row_node, ActionId(REVEAL_BASE + i as u32), icon, 32.0);
                self.reveal_icons.push((i, icon_node));
            }
            self.edit_fields.push(field_node);
        }
        if kind == Kind::Login {
            self.generator(ui, config);
        }
        let error = painted(ui, self.detail, "", TextStyle::Footnote, Paint::Fixed(ERROR));
        ui.set_visible(error, false);
        self.edit_error = Some(error);
        let buttons = ui.add(
            self.detail,
            Widget::container(),
            Style {
                flex_shrink: 0.0,
                margin: Rect {
                    left: length(0.0),
                    right: length(0.0),
                    top: length(12.0),
                    bottom: length(0.0),
                },
                ..row(10.0)
            },
        );
        pill(ui, buttons, SAVE, "Save", false);
        pill(ui, buttons, CANCEL_EDIT, "Cancel", false);
    }

    /// The password generator under a login's fields: length, symbols, Generate.
    fn generator(&mut self, ui: &mut Ui, config: &Config) {
        let label = secondary(ui, self.detail, "PASSWORD GENERATOR", TextStyle::Caption);
        let mut style = ui.style(label);
        style.margin =
            Rect { left: length(GROUP_INSET), right: length(0.0), top: length(18.0), bottom: length(6.0) };
        ui.set_style(label, style);
        let group = self.group(ui);
        let row_node = ui.add(
            group,
            Widget::container(),
            Style { size: Size { width: auto(), height: length(EDIT_ROW) }, flex_shrink: 0.0, ..row(12.0) },
        );
        ui.add_label(row_node, "Length", TextStyle::Body, Style::default());
        let (min, max) = GENERATOR_RANGE;
        let value = ((config.passwords.generator_length as f32 - min) / (max - min)).clamp(0.0, 1.0);
        self.generator_length = Some(ui.add(
            row_node,
            Widget::slider(value),
            Style {
                size: Size { width: length(140.0), height: auto() },
                flex_shrink: 0.0,
                ..Default::default()
            },
        ));
        let length_label =
            secondary(ui, row_node, &config.passwords.generator_length.to_string(), TextStyle::Footnote);
        let mut style = ui.style(length_label);
        style.size = Size { width: length(24.0), height: auto() };
        ui.set_style(length_label, style);
        self.generator_length_label = Some(length_label);
        ui.add_label(row_node, "Symbols", TextStyle::Body, Style::default());
        self.generator_symbols =
            Some(ui.add(row_node, Widget::toggle(config.passwords.generator_symbols), Style::default()));
        ui.add(row_node, Widget::container(), grow());
        pill(ui, row_node, GENERATE, "Generate", false);
    }

    /// Reads the edited fields and saves the item.
    fn save_edit(&mut self, ui: &mut Ui, config: &Config, id: Option<String>, kind: Kind) {
        let mut data = id
            .as_deref()
            .and_then(|id| self.vault.as_ref()?.get(id))
            .map_or_else(|| ItemData::empty(kind), |i| i.data.clone());
        for (i, &node) in self.edit_fields.iter().enumerate() {
            if let Some(value) = field_mut(&mut data, i) {
                *value = ui.field_text(node).unwrap_or_default().trim().to_owned();
            }
        }
        let missing = match &data {
            ItemData::Login(l) if l.site.is_empty() || l.password.is_empty() => {
                Some("Add a website and a password.")
            }
            ItemData::Card(c) if c.number.is_empty() => Some("Add the card number."),
            ItemData::Address(_) | ItemData::Note(_)
                if (0..fields(kind).len()).all(|i| field(&data, i).is_empty()) =>
            {
                Some("Fill in at least one field.")
            }
            _ => None,
        };
        if let Some(message) = missing {
            if let Some(error) = self.edit_error {
                ui.set_text(error, message);
                ui.set_visible(error, true);
            }
            return;
        }
        let Some(vault) = &mut self.vault else { return };
        let result = match id {
            Some(id) => vault.update(&id, data).map(|()| id),
            None => vault.add(data),
        };
        match result {
            Ok(id) => self.select(ui, config, Detail::View(id)),
            Err(e) => {
                tracing::error!("saving to passwords failed: {e}");
                if let Some(error) = self.edit_error {
                    ui.set_text(error, &e.to_string());
                    ui.set_visible(error, true);
                }
            }
        }
    }

    fn select(&mut self, ui: &mut Ui, config: &Config, detail: Detail) {
        if detail != self.shown {
            self.revealed.clear();
            self.copied_field = None;
        }
        self.shown = detail;
        self.confirm_delete = false;
        self.rebuild_list(ui);
        self.show_detail(ui, config);
    }

    /// Puts `text` on the clipboard and schedules clearing it.
    fn copy(&mut self, config: &Config, clipboard: &mut Option<arboard::Clipboard>, text: &str) {
        let Some(clipboard) = clipboard else { return };
        if clipboard.set_text(text).is_err() {
            tracing::warn!("copying to the clipboard failed");
            return;
        }
        let after = config.passwords.clear_clipboard_seconds;
        self.copied = (after > 0.0)
            .then(|| (Instant::now() + Duration::from_secs_f32(after), Zeroizing::new(text.to_owned())));
    }

    /// A click, Enter or value change in the passwords UI (see [`Self::wants`]).
    pub fn event(
        &mut self,
        ui: &mut Ui,
        config: &mut Config,
        clipboard: &mut Option<arboard::Clipboard>,
        pointer: Option<Vec2>,
        event: UiEvent,
    ) -> Outcome {
        if let Some(vault) = &mut self.vault {
            vault.touch();
        }
        match event {
            UiEvent::Submitted { node, .. } if node == self.alert_field => {
                if ui.is_visible(self.confirm_box) {
                    return Outcome::Events(ui.focus(self.confirm_field));
                }
                self.try_unlock(ui, config)
            }
            UiEvent::Submitted { node, .. } if node == self.confirm_field => self.try_unlock(ui, config),
            UiEvent::Submitted { node, .. } if self.prompt.field == Some(node) => {
                self.unlock_from_prompt(ui, config)
            }
            UiEvent::Submitted { .. } => {
                if let Detail::Edit { id, kind } = self.shown.clone() {
                    self.save_edit(ui, config, id, kind);
                }
                Outcome::Nothing
            }
            UiEvent::ValueChanged { node, value } if Some(node) == self.generator_length => {
                let (min, max) = GENERATOR_RANGE;
                config.passwords.generator_length = (min + (max - min) * value).round() as u32;
                if let Some(label) = self.generator_length_label {
                    ui.set_text(label, &config.passwords.generator_length.to_string());
                }
                Outcome::ConfigChanged
            }
            UiEvent::ValueChanged { node, value } if Some(node) == self.generator_symbols => {
                config.passwords.generator_symbols = value > 0.5;
                Outcome::ConfigChanged
            }
            UiEvent::Clicked { action, .. } => self.click(ui, config, clipboard, pointer, action),
            _ => Outcome::Nothing,
        }
    }

    fn click(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        clipboard: &mut Option<arboard::Clipboard>,
        pointer: Option<Vec2>,
        action: ActionId,
    ) -> Outcome {
        match action {
            // The backdrop's action also fires for clicks on the sheet's background.
            CLOSE if pointer.is_some_and(|p| self.contains(ui, p)) => {}
            CLOSE | CANCEL_UNLOCK => self.close(ui),
            UNLOCK => return self.try_unlock(ui, config),
            LOCK => return self.lock(ui, config),
            ADD => self.select(ui, config, Detail::Edit { id: None, kind: Kind::Login }),
            EDIT => {
                if let Detail::View(id) = self.shown.clone() {
                    let kind = self.vault.as_ref().and_then(|v| v.get(&id)).map(|i| i.data.kind());
                    if let Some(kind) = kind {
                        self.select(ui, config, Detail::Edit { id: Some(id), kind });
                    }
                }
            }
            DELETE => {
                if let Detail::View(id) = self.shown.clone() {
                    if !self.confirm_delete {
                        self.confirm_delete = true;
                        self.show_detail(ui, config);
                    } else if let Some(vault) = &mut self.vault {
                        if let Err(e) = vault.delete(&id) {
                            tracing::error!("deleting from passwords failed: {e}");
                        }
                        self.select(ui, config, Detail::Empty);
                    }
                }
            }
            SAVE => {
                if let Detail::Edit { id, kind } = self.shown.clone() {
                    self.save_edit(ui, config, id, kind);
                }
            }
            CANCEL_EDIT => {
                let back = match self.shown.clone() {
                    Detail::Edit { id: Some(id), .. } => Detail::View(id),
                    _ => Detail::Empty,
                };
                self.select(ui, config, back);
            }
            GENERATE => {
                let password = Zeroizing::new(axion_vault::generator::generate(
                    config.passwords.generator_length as usize,
                    config.passwords.generator_symbols,
                ));
                // The login's password field, shown so the user sees what was made.
                if let Some(&node) = self.edit_fields.get(2) {
                    ui.set_field_text(node, &password);
                    self.set_revealed(ui, 2, true);
                }
            }
            PROMPT_CLOSE => {
                if let Some(PromptState::Unlock { url, .. }) = &self.prompt.state {
                    self.unlock_dismissed.insert(site::display_host(url));
                }
                self.prompt.hide();
            }
            PROMPT_UNLOCK => return self.unlock_from_prompt(ui, config),
            PROMPT_SAVE => {
                if let Some(PromptState::Save(pending)) = self.prompt.state.take() {
                    self.prompt.hide();
                    if self.is_unlocked() {
                        self.save_login(pending);
                    } else {
                        // Saved once the user unlocks (or sets up) the vault.
                        self.pending = Some(pending);
                        let window = ui.frame(ui.root()).size;
                        return self.open(ui, config, window);
                    }
                }
            }
            ActionId(n) if (PROMPT_FILL_BASE..PROMPT_FILL_BASE + PROMPT_LOGINS as u32).contains(&n) => {
                return self.fill((n - PROMPT_FILL_BASE) as usize);
            }
            ActionId(n) if (KIND_BASE..KIND_BASE + Kind::ALL.len() as u32).contains(&n) => {
                let kind = Kind::ALL[(n - KIND_BASE) as usize];
                self.select(ui, config, Detail::Edit { id: None, kind });
            }
            ActionId(n) if (REVEAL_BASE..REVEAL_BASE + MAX_FIELDS).contains(&n) => {
                let i = (n - REVEAL_BASE) as usize;
                let on = !self.revealed.get(i).copied().unwrap_or(false);
                if matches!(self.shown, Detail::Edit { .. }) {
                    self.set_revealed(ui, i, on);
                } else {
                    self.set_revealed(ui, i, on);
                    self.show_detail(ui, config);
                }
            }
            ActionId(n) if (COPY_BASE..COPY_BASE + MAX_FIELDS).contains(&n) => {
                let i = (n - COPY_BASE) as usize;
                if let Detail::View(id) = &self.shown {
                    let value = self
                        .vault
                        .as_ref()
                        .and_then(|v| v.get(id))
                        .map(|item| Zeroizing::new(field(&item.data, i).to_owned()));
                    if let Some(value) = value {
                        self.copy(config, clipboard, &value);
                        self.copied_field = Some(i);
                        self.show_detail(ui, config);
                    }
                }
            }
            ActionId(n) if (ITEM_BASE..PROMPT_CLOSE.0).contains(&n) => {
                if let Some(id) = self.list_ids.get((n - ITEM_BASE) as usize).cloned() {
                    self.select(ui, config, Detail::View(id));
                }
            }
            _ => {}
        }
        Outcome::Nothing
    }

    fn set_revealed(&mut self, ui: &mut Ui, i: usize, on: bool) {
        if self.revealed.len() <= i {
            self.revealed.resize(i + 1, false);
        }
        self.revealed[i] = on;
        if let Some(&node) = self.edit_fields.get(i) {
            ui.set_secure(node, !on);
        }
        if let Some(&(_, icon)) = self.reveal_icons.iter().find(|(f, _)| *f == i) {
            ui.set_icon(icon, if on { Icon::EyeOff } else { Icon::Eye });
        }
    }

    /// Saves submitted credentials: updates the login for that site and user, or adds one.
    fn save_login(&mut self, pending: PendingSave) {
        let Some(vault) = &mut self.vault else { return };
        let existing =
            vault.login_for(&pending.url, &pending.username).map(|i| (i.id.clone(), i.data.clone()));
        let result = match existing {
            Some((id, ItemData::Login(mut login))) => {
                if *login.password == *pending.password {
                    return;
                }
                login.password = pending.password.to_string();
                vault.update(&id, ItemData::Login(login))
            }
            _ => vault
                .add(ItemData::Login(Login {
                    site: site::origin(&pending.url),
                    username: pending.username.clone(),
                    password: pending.password.to_string(),
                    notes: String::new(),
                }))
                .map(|_| ()),
        };
        match result {
            Ok(()) => tracing::info!(site = %site::display_host(&pending.url), "password saved"),
            Err(e) => tracing::error!("saving the password failed: {e}"),
        }
    }

    fn fill(&mut self, i: usize) -> Outcome {
        let Some(PromptState::Fill { view, url, ids }) = &self.prompt.state else { return Outcome::Nothing };
        let (view, url) = (*view, url.clone());
        let login =
            ids.get(i).and_then(|id| self.vault.as_ref()?.get(id)).and_then(|item| match &item.data {
                ItemData::Login(l) if site::same_site(&l.site, &url) => Some(l.clone()),
                _ => None,
            });
        self.prompt.hide();
        let Some(login) = login else { return Outcome::Nothing };
        if let Some(vault) = &mut self.vault {
            vault.touch();
        }
        match axion_vault::autofill::fill_script(&url, &login.username, &login.password) {
            Some(script) => Outcome::Fill { view, script },
            None => Outcome::Nothing,
        }
    }

    /// The autofill script in the active tab's top frame reported something. `url` is the
    /// tab's own address (never one the page claims).
    pub fn page_event(&mut self, ui: &mut Ui, config: &Config, view: ViewId, url: &str, event: PageEvent) {
        if self.open {
            return;
        }
        match event {
            PageEvent::Form => {
                if !config.passwords.autofill || matches!(self.prompt.state, Some(PromptState::Save(_))) {
                    return;
                }
                let Some(vault) = &self.vault else { return };
                if !vault.is_unlocked() {
                    // Locked: which sites have logins is encrypted too, so offer to unlock on
                    // every sign-in form, unless the user said "Not Now" for this site.
                    if vault.is_set_up() && !self.unlock_dismissed.contains(&site::display_host(url)) {
                        self.prompt.state = Some(PromptState::Unlock { view, url: url.to_owned() });
                        self.build_prompt(ui, config);
                    }
                    return;
                }
                if !self.show_fill(ui, config, view, url) {
                    self.prompt.hide();
                }
            }
            PageEvent::NoForm => {
                if matches!(&self.prompt.state,
                    Some(PromptState::Fill { view: v, .. } | PromptState::Unlock { view: v, .. }) if *v == view)
                {
                    self.prompt.hide();
                }
            }
            PageEvent::Submit { username, password } => {
                if !config.passwords.offer_to_save || self.vault.is_none() {
                    return;
                }
                if let Some(vault) = self.vault.as_ref().filter(|v| v.is_unlocked())
                    && let Some(item) = vault.login_for(url, &username)
                    && matches!(&item.data, ItemData::Login(l) if *l.password == *password)
                {
                    return;
                }
                self.prompt.state =
                    Some(PromptState::Save(PendingSave { url: url.to_owned(), username, password }));
                self.build_prompt(ui, config);
            }
        }
    }

    /// Offers the logins saved for `url`. Returns false when there are none.
    fn show_fill(&mut self, ui: &mut Ui, config: &Config, view: ViewId, url: &str) -> bool {
        let ids: Vec<String> = match &self.vault {
            Some(vault) if vault.is_unlocked() => {
                vault.logins_for(url).iter().take(PROMPT_LOGINS).map(|i| i.id.clone()).collect()
            }
            _ => return false,
        };
        if ids.is_empty() {
            return false;
        }
        self.prompt.state = Some(PromptState::Fill { view, url: url.to_owned(), ids });
        self.build_prompt(ui, config);
        true
    }

    /// The unlock prompt's password was entered: unlock, then offer this site's logins.
    fn unlock_from_prompt(&mut self, ui: &mut Ui, config: &Config) -> Outcome {
        let (Some(PromptState::Unlock { view, url }), Some(field)) = (&self.prompt.state, self.prompt.field)
        else {
            return Outcome::Nothing;
        };
        let (view, url) = (*view, url.clone());
        let Some(vault) = &mut self.vault else { return Outcome::Nothing };
        let password = Zeroizing::new(ui.field_text(field).unwrap_or_default().to_owned());
        ui.set_field_text(field, "");
        match vault.unlock(&password) {
            Ok(()) => {
                tracing::info!("passwords unlocked");
                if let Some(pending) = self.pending.take() {
                    self.save_login(pending);
                }
                if !self.show_fill(ui, config, view, &url) {
                    self.prompt.hide();
                }
                Outcome::Nothing
            }
            Err(e) => {
                if !matches!(e, VaultError::WrongPassword) {
                    tracing::error!("unlocking passwords failed: {e}");
                }
                if let Some(error) = self.prompt.error {
                    let text = if matches!(e, VaultError::WrongPassword) {
                        "Wrong password".to_owned()
                    } else {
                        e.to_string()
                    };
                    ui.set_text(error, &text);
                    ui.set_visible(error, true);
                }
                Outcome::Events(ui.focus(field))
            }
        }
    }

    fn build_prompt(&mut self, ui: &mut Ui, config: &Config) {
        let root = self.prompt.root;
        ui.clear_children(root);
        self.prompt.field = None;
        self.prompt.error = None;
        let header = ui.add(root, Widget::container(), Style { padding: padding(6.0, 2.0), ..row(10.0) });
        ui.add(header, Widget::icon(Icon::Key, 20.0).with_paint(Paint::Accent), Style::default());
        let titles = ui.add(header, Widget::container(), Style { ..column(0.0) });
        let mut style = ui.style(titles);
        style.flex_grow = 1.0;
        style.min_size = Size { width: length(0.0), height: auto() };
        ui.set_style(titles, style);
        match &self.prompt.state {
            Some(PromptState::Fill { url, ids, .. }) => {
                ui.add_label(titles, "Fill with Axion", TextStyle::BodyEmphasis, Style::default());
                secondary(ui, titles, &site::display_host(url), TextStyle::Footnote);
                icon_button(ui, header, PROMPT_CLOSE, Icon::Close, 28.0);
                for (i, id) in ids.iter().enumerate() {
                    let Some(ItemData::Login(login)) =
                        self.vault.as_ref().and_then(|v| v.get(id)).map(|i| &i.data)
                    else {
                        continue;
                    };
                    let row_node = ui.add(
                        root,
                        Widget::container()
                            .with_action(ActionId(PROMPT_FILL_BASE + i as u32))
                            .with_radius(14.0)
                            .with_background(Background::Fill(Paint::Surface, 0.6)),
                        Style {
                            size: Size { width: auto(), height: length(46.0) },
                            padding: padding(12.0, 0.0),
                            ..row(10.0)
                        },
                    );
                    ui.add(
                        row_node,
                        Widget::icon(Icon::Key, 16.0).with_paint(Paint::Secondary),
                        Style::default(),
                    );
                    let name =
                        if login.username.is_empty() { "No user name" } else { login.username.as_str() };
                    ui.add_label(row_node, name, TextStyle::Body, grow());
                    secondary(ui, row_node, HIDDEN, TextStyle::Footnote);
                }
            }
            Some(PromptState::Unlock { url, .. }) => {
                ui.add_label(titles, "Unlock to fill", TextStyle::BodyEmphasis, Style::default());
                secondary(ui, titles, &site::display_host(url), TextStyle::Footnote);
                icon_button(ui, header, PROMPT_CLOSE, Icon::Close, 28.0);
                let line =
                    ui.add(root, Widget::container(), Style { padding: padding(2.0, 0.0), ..row(8.0) });
                let field = field_box(ui, line, "Master password", Style { flex_grow: 1.0, ..row(0.0) });
                ui.set_secure(field, true);
                // The field takes the room; the button keeps its natural width.
                let label = capsule_button(ui, line, PROMPT_UNLOCK, "Unlock", true);
                if let Some(button) = ui.parent(label) {
                    let mut style = ui.style(button);
                    style.flex_grow = 0.0;
                    ui.set_style(button, style);
                }
                let error = painted(ui, root, "", TextStyle::Footnote, Paint::Fixed(ERROR));
                ui.set_visible(error, false);
                self.prompt.field = Some(field);
                self.prompt.error = Some(error);
            }
            Some(PromptState::Save(pending)) => {
                let update = self.is_unlocked()
                    && self
                        .vault
                        .as_ref()
                        .is_some_and(|v| v.login_for(&pending.url, &pending.username).is_some());
                let title = if update { "Update saved password?" } else { "Save password?" };
                ui.add_label(titles, title, TextStyle::BodyEmphasis, Style::default());
                let host = site::display_host(&pending.url);
                let who = if pending.username.is_empty() {
                    host
                } else {
                    format!("{} · {host}", pending.username)
                };
                secondary(ui, titles, &who, TextStyle::Footnote);
                let buttons =
                    ui.add(root, Widget::container(), Style { padding: padding(2.0, 2.0), ..row(8.0) });
                capsule_button(ui, buttons, PROMPT_CLOSE, "Not Now", false);
                let save =
                    if self.is_unlocked() { if update { "Update" } else { "Save" } } else { "Save…" };
                capsule_button(ui, buttons, PROMPT_SAVE, save, true);
            }
            None => return,
        }
        ui.set_visible(root, true);
        self.prompt.appear.spring = spring(config);
        self.prompt.appear.snap(0.0);
        self.prompt.appear.target = 1.0;
    }

    /// Hides the prompt (another tab was selected, or the page navigated away).
    pub fn hide_prompt(&mut self, view: Option<ViewId>, keep_save: bool) {
        let hide = match &self.prompt.state {
            Some(PromptState::Fill { view: v, .. } | PromptState::Unlock { view: v, .. }) => {
                view.is_none_or(|view| view == *v)
            }
            Some(PromptState::Save(_)) => !keep_save,
            None => false,
        };
        if hide {
            self.prompt.hide();
        }
    }

    /// Per frame: the search, the prompt's place under the address bar, animations.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32, address: Frame) {
        if self.open && ui.is_visible(self.sheet) {
            let query = ui.field_text(self.search).unwrap_or_default().to_owned();
            if query != self.query {
                self.query = query;
                self.rebuild_list(ui);
            }
        }
        if self.open && ui.is_visible(self.alert_layer) {
            self.alert_appear.step(dt);
            let k = self.alert_appear.value.clamp(0.0, 1.2);
            ui.set_opacity(self.alert_layer, k.min(1.0));
            scale_in(ui, self.alert, k);
        }
        self.prompt.animate(ui, dt, address);
    }

    /// Every loop iteration: auto-lock and clipboard clearing.
    pub fn tick(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        clipboard: &mut Option<arboard::Clipboard>,
    ) -> Outcome {
        if let Some((at, _)) = &self.copied
            && Instant::now() >= *at
            && let Some((_, text)) = self.copied.take()
            && let Some(clipboard) = clipboard
        {
            // Only if it still holds what we copied (not something the user copied since).
            let current = Zeroizing::new(clipboard.get_text().unwrap_or_default());
            if *current == *text {
                let _ = clipboard.clear();
                tracing::info!("clipboard cleared");
            }
        }
        let minutes = config.passwords.auto_lock_minutes;
        if minutes > 0.0
            && let Some(vault) = &mut self.vault
            && vault.lock_if_idle(Duration::from_secs_f32(minutes * 60.0))
        {
            tracing::info!("passwords locked after inactivity");
            return self.lock(ui, config);
        }
        Outcome::Nothing
    }

    /// Writes an encrypted copy of the vault (Settings › Passwords). Returns a status line.
    pub fn export(&self, path: &Path) -> String {
        match &self.vault {
            Some(vault) if vault.is_unlocked() => match vault.export_encrypted(path) {
                Ok(()) => format!("Exported to {}", path.display()),
                Err(e) => {
                    tracing::error!("exporting passwords failed: {e}");
                    format!("Export failed: {e}")
                }
            },
            _ => "Unlock Passwords first (open Passwords in the sidebar).".to_owned(),
        }
    }

    /// Imports a CSV export of another browser or manager. Returns a status line.
    pub fn import_csv(&mut self, path: &Path) -> String {
        let Some(vault) = self.vault.as_mut().filter(|v| v.is_unlocked()) else {
            return "Unlock Passwords first (open Passwords in the sidebar).".to_owned();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => Zeroizing::new(text),
            Err(e) => return format!("Can't read {}: {e}", path.display()),
        };
        match vault.import_csv(&text) {
            Ok(0) => "Nothing new to import.".to_owned(),
            Ok(n) => format!("Imported {n} item{}.", if n == 1 { "" } else { "s" }),
            Err(e) => {
                tracing::error!("importing passwords failed: {e}");
                format!("Import failed: {e}")
            }
        }
    }

    /// Capture aid (throwaway profiles only): sets up the vault with a test password and
    /// clearly fake entries.
    pub fn seed_test_data(&mut self) {
        let Some(vault) = &mut self.vault else { return };
        if !vault.is_set_up() && vault.setup("test-only-pass").is_err() {
            return;
        }
        if !vault.is_unlocked() && vault.unlock("test-only-pass").is_err() {
            return;
        }
        if !vault.items().is_empty() {
            return;
        }
        let login = |site: &str, user: &str| {
            ItemData::Login(Login {
                site: site.into(),
                username: user.into(),
                password: "test-only-pass".into(),
                notes: String::new(),
            })
        };
        let items = [
            login("https://example.com", "test.user@example.com"),
            login("https://login.example.org", "tester"),
            login("http://127.0.0.1:8765", "localtest"),
            ItemData::Card(axion_vault::Card {
                label: "Test Visa".into(),
                cardholder: "Test User".into(),
                number: "4111111111111111".into(),
                expiry: "12/30".into(),
                cvc: "123".into(),
            }),
            ItemData::Address(axion_vault::Address {
                name: "Test User".into(),
                street: "1 Example Street".into(),
                city: "Testville".into(),
                zip: "12345".into(),
                country: "Exampleland".into(),
                email: "test.user@example.com".into(),
                phone: "+1 555 0100".into(),
            }),
            ItemData::Note(axion_vault::Note {
                title: "Test note".into(),
                text: "Nothing secret here.".into(),
            }),
        ];
        for item in items {
            let _ = vault.add(item);
        }
    }

    /// Capture aids: "first" shows the first item, "reveal" its secret, "edit" / "add" the
    /// editor, "generate" a password in it.
    pub fn capture_step(&mut self, ui: &mut Ui, config: &mut Config, what: &str) -> Outcome {
        let action = match what {
            "first" => {
                if let Some(id) = self.vault.as_ref().and_then(|v| v.items().first()).map(|i| i.id.clone()) {
                    self.select(ui, config, Detail::View(id));
                }
                return Outcome::Nothing;
            }
            "fill" => ActionId(PROMPT_FILL_BASE),
            // The unlock prompt, with the test vault's password typed in.
            "unlock" => {
                if let Some(field) = self.prompt.field {
                    ui.set_field_text(field, "test-only-pass");
                }
                PROMPT_UNLOCK
            }
            "reveal" => ActionId(REVEAL_BASE + 2),
            "edit" => EDIT,
            "add" => ADD,
            "generate" => GENERATE,
            "lock" => LOCK,
            _ => return Outcome::Nothing,
        };
        self.click(ui, config, &mut None, None, action)
    }

    /// Capture aid: the save prompt as after signing in with new credentials.
    pub fn show_save_prompt_for_test(&mut self, ui: &mut Ui, config: &Config, view: ViewId, url: &str) {
        let event = PageEvent::Submit {
            username: "new.tester".into(),
            password: Zeroizing::new("test-only-pass-2".into()),
        };
        self.page_event(ui, config, view, url, event);
    }
}

impl Prompt {
    fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Glass(GlassLevel::Regular))
                .with_radius(PROMPT_RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: length(PROMPT_WIDTH), height: auto() },
                padding: padding(10.0, 10.0),
                ..column(8.0)
            },
        );
        ui.set_visible(root, false);
        Self { root, state: None, field: None, error: None, appear: Animated::new(0.0, Spring::SNAPPY) }
    }

    fn hide(&mut self) {
        self.state = None;
        self.appear.target = 0.0;
    }

    fn animate(&mut self, ui: &mut Ui, dt: f32, address: Frame) {
        if !ui.is_visible(self.root) {
            return;
        }
        // Centred under the address field.
        let left = address.origin.x + (address.size.x - PROMPT_WIDTH) / 2.0;
        let top = address.origin.y + address.size.y + PROMPT_GAP;
        let mut style = ui.style(self.root);
        let inset = Rect { left: length(left.max(8.0)), right: auto(), top: length(top), bottom: auto() };
        if style.inset != inset {
            style.inset = inset;
            ui.set_style(self.root, style);
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if self.state.is_none() && k < 0.01 {
            ui.set_visible(self.root, false);
            ui.clear_children(self.root);
            return;
        }
        ui.set_opacity(self.root, k.min(1.0));
        scale_in(ui, self.root, k);
    }
}
