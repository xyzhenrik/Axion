//! Passwords: the sheet that manages the vault (search, a list grouped by kind, details with
//! copy and reveal, editing and a password generator), the unlock card shown while the vault is
//! locked (styled like the private browsing one), and the small glass prompt under the address
//! bar that offers to fill a saved login or to save a new one.
//!
//! Secrets stay out of logs; only the user's clicks put them on the clipboard or into a page.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config};
use axion_engine::ViewId;
use axion_i18n::{tr, tr_fmt, tr_mark, tr_n};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, UiEvent, Widget};
use axion_vault::autofill::PageEvent;
use axion_vault::{DeviceKey, ItemData, Kind, Login, Vault, VaultError, site};
use glam::Vec2;
use zeroize::Zeroizing;

use crate::drip::Phase;
use crate::favicons::{Favicons, SiteIcon};
use crate::morph::{Origin, SheetMorph};
use crate::popover::Popover;

/// Clear of the chrome's ranges (its other-group tab rows start at 12 000 and its panel
/// buttons at 13 000, which this range used to swallow).
const BASE: u32 = 20_000;
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
const PROMPT_PADDING: f32 = 10.0;
const PROMPT_GAP_ROWS: f32 = 8.0;
const PROMPT_MARGIN: f32 = 8.0;
/// Fixed row heights, so the drip knows the prompt's size before layout.
const PROMPT_HEADER: f32 = 44.0;
const PROMPT_ROW: f32 = 46.0;
const PROMPT_LINE: f32 = 44.0;
const PROMPT_ERROR: f32 = 18.0;
/// Most logins the fill prompt lists.
const PROMPT_LOGINS: usize = 3;
/// The icon in a list row.
const LIST_ICON: f32 = 30.0;
/// Vault secrets with this prefix belong to network connections (the rest of the name is the
/// connection's id).
const NETWORK_SECRET_PREFIX: &str = "net:";

/// The account password (the vault's master password) is at least this long.
pub const MIN_MASTER_LENGTH: usize = 8;
/// The password capture aids give throwaway test accounts.
pub const TEST_PASSWORD: &str = "test-only-pass";
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
        Kind::Login => tr("Password"),
        Kind::Card => tr("Card"),
        Kind::Address => tr("Address"),
        Kind::Note => tr("Note"),
    }
}

const LOGIN_FIELDS: &[(&str, bool)] = &[
    (tr_mark("Website"), false),
    (tr_mark("User name"), false),
    (tr_mark("Password"), true),
    (tr_mark("Notes"), false),
];

const CARD_FIELDS: &[(&str, bool)] = &[
    (tr_mark("Name"), false),
    (tr_mark("Cardholder"), false),
    (tr_mark("Card number"), true),
    (tr_mark("Expiry (MM/YY)"), false),
    (tr_mark("Security code"), true),
];

const ADDRESS_FIELDS: &[(&str, bool)] = &[
    (tr_mark("Full name"), false),
    (tr_mark("Street"), false),
    (tr_mark("City"), false),
    (tr_mark("ZIP code"), false),
    (tr_mark("Country"), false),
    (tr_mark("Email"), false),
    (tr_mark("Phone"), false),
];

const NOTE_FIELDS: &[(&str, bool)] = &[(tr_mark("Title"), false), (tr_mark("Note"), true)];

/// The fields of each kind: label and whether it is a secret (hidden until revealed).
fn fields(kind: Kind) -> &'static [(&'static str, bool)] {
    match kind {
        Kind::Login => LOGIN_FIELDS,
        Kind::Card => CARD_FIELDS,
        Kind::Address => ADDRESS_FIELDS,
        Kind::Note => NOTE_FIELDS,
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
    /// The sheet's lock button: lock the account (private tabs and Passwords) now.
    LockSession,
}

/// Why the alert asks for the account password although the session may be unlocked: these
/// always ask again (user decision); everything else (filling a login) doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reauth {
    /// Opening the overview of everything saved.
    OpenSheet,
    /// Showing or copying a card's number or security code (field index).
    Reveal(usize),
    Copy(usize),
}

/// Whether using field `secret` of an item of `kind` asks for the password every time: a
/// card's number and security code (money), nothing else.
fn asks_every_time(kind: Kind, secret: bool) -> bool {
    kind == Kind::Card && secret
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

/// The glass prompt that drips out of the address field (see [`Popover`]).
struct Prompt {
    popover: Popover,
    root: NodeId,
    state: Option<PromptState>,
    /// The master password field and its error line (unlock prompt only).
    field: Option<NodeId>,
    error: Option<NodeId>,
    /// The content's height (fixed rows, so the drip knows its target before layout).
    height: f32,
    /// The address field it flows out of (logical px), as of the last frame.
    address: Frame,
}

/// The vault, its sheet and the autofill prompt.
pub struct Passwords {
    vault: Option<Vault>,
    root: NodeId,
    sheet: NodeId,
    /// The sheet flows out of where it was opened from (once unlocked) and drains back.
    morph: SheetMorph,
    /// Where the sheet comes from, and the address field (when that is gone).
    origin: Option<(Origin, Origin)>,
    /// The sheet's laid out frame (logical px).
    sheet_frame: Frame,
    /// The toolbar, and the address field in it (logical px; the prompt's anchor).
    bar: NodeId,
    address: Frame,
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
    /// The account's name, for the unlock texts (its password is the master password).
    account: String,
    /// The alert asks again before this (see [`Reauth`]).
    asking: Option<Reauth>,
    /// The user name from the first step of a two-step sign-in, until the password step.
    step_user: Option<StepUser>,
    /// Credentials the user declined to save, remembered briefly and only for that exact
    /// (site, user name, password). Never a whole site.
    declines: Declines,
    /// The login last filled from the vault (host, user name), to tell a typo from a change.
    last_fill: Option<(String, String, Instant)>,
    /// A different password for a just-filled login, waiting to see whether the sign-in form
    /// comes back (a failed login) before "Change Password?" is offered.
    held: Option<Held>,
    /// The related-domain groups the vault was given (to apply config changes).
    related_applied: Vec<Vec<String>>,
    /// Site icons of the logins in the list, by host.
    icons: HashMap<String, SiteIcon>,
    /// Hosts whose icon was asked for since the sheet opened.
    icons_requested: HashSet<String>,
}

/// The user name typed on a sign-in's first step. Kept in memory only, never on disk.
struct StepUser {
    username: String,
    at: Instant,
}

/// A submit waiting for the page to show whether the login worked.
struct Held {
    view: ViewId,
    pending: PendingSave,
    at: Instant,
}

/// What a submitted login leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Offer {
    /// "Save password?" (no login for this site and user yet).
    Save,
    /// "Change Password?" (the site and user have a login with another password).
    Change,
}

/// Whether, and how, to offer saving a submitted login. `stored` is the saved password for the
/// same site and user name (`None`: no such login, or the vault is locked), `declined` whether
/// the user already said "Not Now" to exactly these credentials.
fn offer_for(stored: Option<&str>, submitted: &str, declined: bool) -> Option<Offer> {
    match stored {
        Some(stored) if stored == submitted => None,
        _ if declined => None,
        Some(_) => Some(Offer::Change),
        None => Some(Offer::Save),
    }
}

/// Fingerprints of declined credentials. The password is only kept as a keyed hash (random key
/// per run), in memory.
struct Declines {
    hasher: std::hash::RandomState,
    list: Vec<(String, String, u64, Instant)>,
}

impl Default for Declines {
    fn default() -> Self {
        Self { hasher: std::hash::RandomState::new(), list: Vec::new() }
    }
}

impl Declines {
    fn print(&self, host: &str, user: &str, password: &str) -> u64 {
        use std::hash::BuildHasher;
        self.hasher.hash_one((host, user, password))
    }

    fn add(&mut self, host: &str, user: &str, password: &str, now: Instant) {
        let print = self.print(host, user, password);
        self.list.retain(|(h, u, p, _)| !(h == host && u == user && *p == print));
        self.list.push((host.to_owned(), user.to_owned(), print, now));
    }

    /// Whether these exact credentials were declined within `memory`.
    fn contains(&mut self, host: &str, user: &str, password: &str, now: Instant, memory: Duration) -> bool {
        self.list.retain(|(.., at)| now.saturating_duration_since(*at) < memory);
        let print = self.print(host, user, password);
        self.list.iter().any(|(h, u, p, _)| h == host && u == user && *p == print)
    }
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
fn fixed(height: f32, padding: Rect<LengthPercentage>, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, padding, ..row(gap) }
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
    let label = painted(ui, parent, text, style, Paint::Secondary);
    ui.set_label_lines(label, 3);
    label
}

fn capsule_button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) -> NodeId {
    let background = if primary {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
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
    let paint = if primary { Paint::OnAccent } else { Paint::Label };
    painted(ui, button, text, TextStyle::BodyEmphasis, paint)
}

/// A small capsule button (accent tint, or red for destructive ones).
fn pill(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, destructive: bool) {
    let (color, on) = if destructive {
        (Paint::Fixed(ERROR), Paint::OnColor(ERROR))
    } else {
        (Paint::Accent, Paint::OnAccent)
    };
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_radius(CAPSULE)
            .with_background(Background::TintedGlass(GlassLevel::Thin, color)),
        Style { size: Size { width: auto(), height: length(34.0) }, padding: padding(16.0, 0.0), ..row(0.0) },
    );
    painted(ui, button, text, TextStyle::Callout, on);
}

/// A round icon button.
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
    ui.add(button, Widget::icon(icon, size * 0.5).with_paint(Paint::Accent), Style::default())
}

/// The kind's icon in a tinted circle.
fn badge(ui: &mut Ui, parent: NodeId, kind: Kind, size: f32) {
    badge_icon(ui, parent, kind_icon(kind), size);
}

/// `icon` in a tinted circle.
fn badge_icon(ui: &mut Ui, parent: NodeId, icon: Icon, size: f32) {
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
    ui.add(circle, Widget::icon(icon, size * 0.55).with_paint(Paint::Accent), Style::default());
}

/// A grey rounded box holding a text field; returns the field.
/// Makes a field of `field_box` a password field with the shared eye.
fn add_eye(ui: &mut Ui, field: NodeId) {
    if let Some(boxed) = ui.parent(field) {
        crate::eye::add(ui, boxed, field);
    }
}

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
        Spring::new(
            config.motion.snappy.response * config.motion.speed,
            config.glass.damping(config.motion.snappy.damping),
        )
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
    /// Builds the sheet and prompt and opens the vault at `path` (locked), bound to this
    /// device's key. The vault's master password is the password of the account named
    /// `account`.
    /// The prompt's glass lives in `bar` (the toolbar), so it flows out of the address field.
    pub fn build(ui: &mut Ui, bar: NodeId, path: &Path, device: Arc<dyn DeviceKey>, account: &str) -> Self {
        let vault =
            Vault::open(path, device).inspect_err(|e| tracing::error!("passwords unavailable: {e}")).ok();
        // Sign-in forms are checked against the vault's site index with the device key: have
        // it loaded (the TPM takes most of a second) before the first one, off this thread.
        if let Some(vault) = vault.as_ref().filter(|v| v.is_set_up()) {
            vault.load_device_key_in_background();
        }
        let prompt = Prompt::build(ui, bar);
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
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
        ui.add_label(header, tr("Passwords"), TextStyle::Title1, grow());
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
            tr("Search"),
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

        // The unlock / set-up card, centred over the dimmed browser: its own plane, so it also
        // lies above the sheet and its content when it asks again over it (card details).
        let alert_layer = ui.add(
            ui.root(),
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
        let alert_title =
            ui.add_label(alert, tr("Unlock Passwords"), TextStyle::BodyEmphasis, Style::default());
        let alert_text = secondary(ui, alert, "", TextStyle::Footnote);
        let alert_note = secondary(ui, alert, "", TextStyle::Footnote);
        let field_style = Style {
            size: Size { width: percent(1.0), height: length(40.0) },
            margin: Rect { left: length(0.0), right: length(0.0), top: length(6.0), bottom: length(0.0) },
            ..row(0.0)
        };
        let alert_field = field_box(ui, alert, tr("Account password"), field_style.clone());
        add_eye(ui, alert_field);
        let confirm_field = field_box(ui, alert, tr("Repeat password"), field_style);
        add_eye(ui, confirm_field);
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
        capsule_button(ui, buttons, CANCEL_UNLOCK, tr("Cancel"), false);
        let alert_button = capsule_button(ui, buttons, UNLOCK, tr("Unlock"), true);
        ui.set_visible(root, false);
        ui.set_visible(alert_layer, false);
        let morph = SheetMorph::build(sheet, root, SHEET_RADIUS, BACKDROP_DIM);
        Self {
            vault,
            root,
            sheet,
            morph,
            origin: None,
            sheet_frame: Frame::default(),
            bar,
            address: Frame::default(),
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
            account: account.to_owned(),
            asking: None,
            step_user: None,
            declines: Declines::default(),
            last_fill: None,
            held: None,
            related_applied: Vec::new(),
            icons: HashMap::new(),
            icons_requested: HashSet::new(),
        }
    }

    /// Hands the vault the configured related-domain groups (when they changed).
    fn sync_related(&mut self, config: &Config) {
        let groups = &config.passwords.related_domains;
        if *groups == self.related_applied {
            return;
        }
        if let Some(vault) = &mut self.vault {
            vault.set_related(site::Related::new(groups));
        }
        self.related_applied.clone_from(groups);
    }

    /// The user name remembered from a sign-in's first step, if it is recent enough.
    fn recent_step_user(&self, config: &Config) -> Option<&str> {
        let step = self.step_user.as_ref()?;
        let limit = Duration::from_secs_f32(config.passwords.two_step_memory_seconds.max(0.0));
        (step.at.elapsed() <= limit).then_some(step.username.as_str())
    }

    /// Looks up the icons of the logins in the list (the browser calls this while the sheet is
    /// open). Returns the hosts to fetch icons for: saved sites with no cached icon, up to
    /// `passwords.icon_fetch_limit` per opening.
    pub fn sync_icons(&mut self, ui: &mut Ui, config: &Config, favicons: &Favicons) -> Vec<String> {
        let mut missing = Vec::new();
        if !self.open {
            return missing;
        }
        let Some(vault) = self.vault.as_ref().filter(|v| v.is_unlocked()) else { return missing };
        let sites: Vec<String> = vault
            .items()
            .iter()
            .filter_map(|i| match &i.data {
                ItemData::Login(l) => Some(l.site.clone()),
                _ => None,
            })
            .collect();
        let limit = config.passwords.icon_fetch_limit as usize;
        let mut changed = false;
        for site in sites {
            let host = site::display_host(&site);
            if self.icons.contains_key(&host) {
                continue;
            }
            if let Some(icon) = favicons.get(&site) {
                self.icons.insert(host, icon);
                changed = true;
            } else if config.passwords.fetch_missing_icons
                && missing.len() < limit
                && self.icons_requested.insert(host.clone())
            {
                missing.push(host);
            }
        }
        if changed {
            self.rebuild_list(ui);
        }
        missing
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_unlocked(&self) -> bool {
        self.vault.as_ref().is_some_and(Vault::is_unlocked)
    }

    /// The secrets of the network connections (connection id, stored text), or `None` while
    /// the vault is locked. They are kept apart from the items (see `Vault::set_secret`).
    pub fn network_secrets(&self) -> Option<Vec<(String, String)>> {
        let vault = self.vault.as_ref().filter(|v| v.is_unlocked())?;
        Some(
            vault
                .secret_names(NETWORK_SECRET_PREFIX)
                .into_iter()
                .filter_map(|name| {
                    let text = vault.secret(&name)?.to_owned();
                    Some((name.strip_prefix(NETWORK_SECRET_PREFIX)?.to_owned(), text))
                })
                .collect(),
        )
    }

    /// Stores the secrets of a connection (needs the unlocked vault).
    pub fn set_network_secret(&mut self, id: &str, text: &str) -> Result<(), VaultError> {
        self.vault
            .as_mut()
            .ok_or(VaultError::NotSetUp)?
            .set_secret(&format!("{NETWORK_SECRET_PREFIX}{id}"), text)
    }

    /// Forgets the secrets of a connection (needs the unlocked vault).
    pub fn remove_network_secret(&mut self, id: &str) -> Result<(), VaultError> {
        self.vault
            .as_mut()
            .ok_or(VaultError::NotSetUp)?
            .remove_secret(&format!("{NETWORK_SECRET_PREFIX}{id}"))
    }

    /// Whether the account has a password (the vault is set up).
    pub fn has_password(&self) -> bool {
        self.vault.as_ref().is_some_and(Vault::is_set_up)
    }

    /// The account was renamed.
    pub fn set_account_name(&mut self, name: &str) {
        name.clone_into(&mut self.account);
    }

    /// Unlocks with the account password from elsewhere (the private tabs' lock screen): the
    /// sheet, a waiting unlock prompt and a pending save carry on as if unlocked here.
    pub fn unlock_with(&mut self, ui: &mut Ui, config: &Config, password: &str) -> Result<(), VaultError> {
        self.sync_related(config);
        let vault = self.vault.as_mut().ok_or(VaultError::NotSetUp)?;
        vault.unlock(password)?;
        tracing::info!("account unlocked");
        self.after_unlock(ui, config);
        Ok(())
    }

    /// Creates the account password (sets up the vault, which leaves it unlocked).
    pub fn set_up_password(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        password: &str,
    ) -> Result<(), VaultError> {
        let vault = self.vault.as_mut().ok_or(VaultError::NotSetUp)?;
        vault.setup(password)?;
        tracing::info!("account password set");
        self.after_unlock(ui, config);
        Ok(())
    }

    /// Changes the account password: the vault is re-encrypted with it (and left unlocked).
    pub fn change_password(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        old: &str,
        new: &str,
    ) -> Result<(), VaultError> {
        let vault = self.vault.as_mut().ok_or(VaultError::NotSetUp)?;
        vault.change_password(old, new)?;
        tracing::info!("account password changed");
        self.after_unlock(ui, config);
        Ok(())
    }

    fn after_unlock(&mut self, ui: &mut Ui, config: &Config) {
        if let Some(pending) = self.pending.take() {
            self.save_login(pending);
        }
        if let Some(PromptState::Unlock { view, url }) = &self.prompt.state {
            let (view, url) = (*view, url.clone());
            if !self.show_fill(ui, config, view, &url) {
                self.prompt.hide();
            }
        }
        let _ = self.refresh(ui, config);
    }

    /// Whether `p` (logical px) is on the sheet or the unlock card (the backdrop closes).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        (ui.is_visible(self.sheet) && ui.frame(self.sheet).contains(p))
            || (ui.is_visible(self.alert_layer) && ui.frame(self.alert).contains(p))
    }

    /// Whether a click with `action` belongs to the passwords UI.
    pub fn owns_action(action: ActionId) -> bool {
        (BASE..LAST).contains(&action.0)
    }

    /// Whether this event belongs to the passwords UI.
    pub fn wants(&self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Clicked { action, .. } => Self::owns_action(*action),
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

    /// Opens the sheet. With an account password it is asked for first, every time (also
    /// while the session is unlocked; then it is only checked).
    /// Opens out of the glass at `origin` (or `fallback`, the address field). The sheet flows
    /// out once the vault is unlocked.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        window: Vec2,
        origin: Origin,
        fallback: Origin,
    ) -> Outcome {
        self.sync_related(config);
        self.origin = Some((origin, fallback));
        self.open = true;
        self.icons_requested.clear();
        self.prompt.hide();
        self.asking = self.has_password().then_some(Reauth::OpenSheet);
        ui.set_visible(self.root, true);
        self.resize(ui, window);
        self.refresh(ui, config)
    }

    /// Closes: a shown sheet drains back into where it came from (or `fallback`).
    pub fn close(&mut self, ui: &mut Ui, config: &Config, fallback: Origin) {
        self.open = false;
        self.asking = None;
        // What was revealed is hidden again next time.
        self.revealed.clear();
        self.confirm_delete = false;
        // Edits in progress are dropped; what's typed doesn't stay in hidden fields.
        if matches!(self.shown, Detail::Edit { .. }) {
            self.shown = Detail::Empty;
        }
        ui.set_field_text(self.alert_field, "");
        ui.set_field_text(self.confirm_field, "");
        crate::eye::hide(ui, self.alert_field);
        crate::eye::hide(ui, self.confirm_field);
        ui.set_visible(self.alert_layer, false);
        let draining = ui.is_visible(self.root) && ui.is_visible(self.sheet) && self.morph.is_shown();
        self.morph.close(ui, config, fallback);
        if !draining {
            self.finish_close(ui);
        }
    }

    /// The address field: where the sheet comes from when the prompt opens it, and where it
    /// drains back into when its own origin is gone.
    fn address_origin(&self) -> Origin {
        Origin { plane: self.bar, point: self.address.center(), control: self.address }
    }

    /// Once the sheet has drained (or right away, without one): its details go, it hides.
    fn finish_close(&mut self, ui: &mut Ui) {
        self.clear_detail(ui);
        ui.set_visible(self.root, false);
    }

    pub fn resize(&mut self, ui: &mut Ui, window: Vec2) {
        let size = (window - Vec2::splat(2.0 * SHEET_MARGIN)).min(SHEET_MAX).max(Vec2::new(480.0, 360.0));
        let origin = ((window - size) / 2.0).max(Vec2::ZERO);
        let mut style = ui.style(self.sheet);
        style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
        style.size = Size { width: length(size.x), height: length(size.y) };
        ui.set_style(self.sheet, style);
        self.sheet_frame = Frame { origin, size };
        self.morph.retarget(self.sheet_frame);
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
        if let Some(reason) = self.asking {
            return self.ask_again(ui, config, reason);
        }
        if let Some(w) = ui.widget_mut(self.alert_layer) {
            w.background = Background::None;
        }
        if self.is_unlocked() {
            // After the unlock alert the window is dimmed already.
            let dimmed = ui.is_visible(self.alert_layer);
            ui.set_visible(self.alert_layer, false);
            ui.set_visible(self.sheet, true);
            if let Some((origin, fallback)) = self.origin {
                self.morph.open(ui, config, origin, fallback, self.sheet_frame, !dimmed);
            }
            self.rebuild_list(ui);
            self.show_detail(ui, config);
            return Outcome::Nothing;
        }
        ui.set_visible(self.sheet, false);
        ui.set_visible(self.alert_layer, true);
        ui.set_visible(self.alert_error, false);
        let set_up = self.vault.as_ref().is_some_and(Vault::is_set_up);
        let account = &self.account;
        let (title, text, note, button) = match &self.vault {
            None => (
                tr("Passwords Unavailable"),
                tr("The password vault couldn't be opened.").to_owned(),
                "",
                tr("Close"),
            ),
            Some(_) if set_up => (
                tr("Unlock Passwords"),
                tr_fmt!("Enter the password of “{name}” to continue.", name = account),
                "",
                tr("Unlock"),
            ),
            Some(_) => (
                tr("Set Up Passwords"),
                tr_fmt!("Choose a password for “{name}”.", name = account),
                tr("It encrypts Passwords and locks private tabs."),
                tr("Create"),
            ),
        };
        ui.set_text(self.alert_title, title);
        ui.set_text(self.alert_text, &text);
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

    /// The alert asking for the password before `reason`. Opening the sheet hides it until
    /// then; for a card's details the sheet stays and the alert lies on top.
    fn ask_again(&mut self, ui: &mut Ui, config: &Config, reason: Reauth) -> Outcome {
        let over_sheet = reason != Reauth::OpenSheet && self.is_unlocked();
        ui.set_visible(self.sheet, over_sheet);
        ui.set_visible(self.alert_layer, true);
        ui.set_visible(self.alert_error, false);
        // Over the sheet the alert's layer takes the clicks (nothing behind it reacts).
        if let Some(w) = ui.widget_mut(self.alert_layer) {
            w.background = if over_sheet {
                Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM / 2.0)
            } else {
                Background::None
            };
        }
        let (title, note) = match reason {
            Reauth::OpenSheet => (tr("Passwords"), ""),
            Reauth::Reveal(_) => {
                (tr("Show Card Details"), tr("Card details ask for the password every time."))
            }
            Reauth::Copy(_) => (tr("Copy Card Details"), tr("Card details ask for the password every time.")),
        };
        ui.set_text(self.alert_title, title);
        let text = tr_fmt!("Enter the password of “{name}” to continue.", name = self.account);
        ui.set_text(self.alert_text, &text);
        ui.set_text(self.alert_note, note);
        ui.set_visible(self.alert_note, !note.is_empty());
        ui.set_text(self.alert_button, if self.is_unlocked() { tr("Continue") } else { tr("Unlock") });
        ui.set_visible(ui.parent(self.alert_field).unwrap_or(self.alert_field), true);
        ui.set_visible(self.confirm_box, false);
        self.alert_appear.spring = spring(config);
        self.alert_appear.snap(0.0);
        self.alert_appear.target = 1.0;
        Outcome::Events(ui.focus(self.alert_field))
    }

    /// The password asked for by [`Self::ask_again`]: checked (unlocking the vault, and so the
    /// session, if it was locked), then what it was asked for happens.
    fn answer_again(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        clipboard: &mut Option<arboard::Clipboard>,
        reason: Reauth,
    ) -> Outcome {
        let Some(vault) = &mut self.vault else {
            self.close(ui, config, self.address_origin());
            return Outcome::Nothing;
        };
        let password = Zeroizing::new(ui.field_text(self.alert_field).unwrap_or_default().to_owned());
        let result = if vault.is_unlocked() { vault.verify(&password) } else { vault.unlock(&password) };
        match result {
            Ok(()) => {
                ui.set_field_text(self.alert_field, "");
                self.asking = None;
                if let Some(pending) = self.pending.take() {
                    self.save_login(pending);
                }
                match reason {
                    Reauth::OpenSheet => {}
                    Reauth::Reveal(i) => self.set_revealed(ui, i, true),
                    Reauth::Copy(i) => self.copy_field(config, clipboard, i),
                }
                let outcome = self.refresh(ui, config);
                // The alert's field had the keyboard; nothing else in the sheet takes it.
                let _ = ui.blur();
                outcome
            }
            Err(VaultError::WrongPassword) => self.alert_failed(ui, tr("Wrong password")),
            Err(e) => {
                tracing::error!("checking the account password failed: {e}");
                self.alert_failed(ui, &describe(&e))
            }
        }
    }

    fn try_unlock(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        clipboard: &mut Option<arboard::Clipboard>,
    ) -> Outcome {
        if let Some(reason) = self.asking {
            return self.answer_again(ui, config, clipboard, reason);
        }
        let Some(vault) = &mut self.vault else {
            self.close(ui, config, self.address_origin());
            return Outcome::Nothing;
        };
        let password = Zeroizing::new(ui.field_text(self.alert_field).unwrap_or_default().to_owned());
        let result = if vault.is_set_up() {
            vault.unlock(&password)
        } else {
            let confirm = Zeroizing::new(ui.field_text(self.confirm_field).unwrap_or_default().to_owned());
            if password.chars().count() < MIN_MASTER_LENGTH {
                return self.alert_failed(
                    ui,
                    &tr_fmt!("Use at least {count} characters.", count = MIN_MASTER_LENGTH),
                );
            }
            if *password != *confirm {
                return self.alert_failed(ui, tr("The passwords don't match."));
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
            Err(VaultError::WrongPassword) => self.alert_failed(ui, tr("Wrong password")),
            Err(e) => {
                tracing::error!("unlocking passwords failed: {e}");
                self.alert_failed(ui, &describe(&e))
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
        self.step_user = None;
        self.icons.clear();
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
                let icon = match &item.data {
                    ItemData::Login(l) => self.icons.get(&site::display_host(&l.site)),
                    _ => None,
                };
                match icon {
                    Some(image) => {
                        let node = ui.add(
                            row_node,
                            Widget::image().with_radius(LIST_ICON * 0.22),
                            Style {
                                size: Size { width: length(LIST_ICON), height: length(LIST_ICON) },
                                flex_shrink: 0.0,
                                ..Default::default()
                            },
                        );
                        ui.set_image(node, Some(image.clone()));
                    }
                    None if kind == Kind::Login => badge_icon(ui, row_node, Icon::Globe, LIST_ICON),
                    None => badge(ui, row_node, kind, LIST_ICON),
                }
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
            let text =
                if self.query.trim().is_empty() { tr("Nothing saved yet.") } else { tr("No matches.") };
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
                let text = if none {
                    tr("Click + to save a password, card or address.")
                } else {
                    tr("Select an item.")
                };
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
            .map(|t| tr_fmt!("Edited {date}", date = t.strftime("%Y-%m-%d")))
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
            secondary(ui, texts, tr(label), TextStyle::Footnote);
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
        pill(ui, buttons, EDIT, tr("Edit"), false);
        pill(
            ui,
            buttons,
            DELETE,
            if self.confirm_delete { tr("Click again to delete") } else { tr("Delete") },
            true,
        );
    }

    fn build_edit(&mut self, ui: &mut Ui, config: &Config, id: Option<&str>, kind: Kind) {
        let item = id.and_then(|id| self.vault.as_ref()?.get(id)).cloned();
        let data = item.as_ref().map_or_else(|| ItemData::empty(kind), |i| i.data.clone());
        let title = if item.is_some() {
            tr_fmt!("Edit {kind}", kind = kind_name(kind))
        } else {
            tr_fmt!("New {kind}", kind = kind_name(kind))
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
                tr(label),
                TextStyle::Body,
                Style {
                    size: Size { width: length(EDIT_LABEL_WIDTH), height: auto() },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            let field_node = field_box(ui, row_node, tr(label), grow());
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
        pill(ui, buttons, SAVE, tr("Save"), false);
        pill(ui, buttons, CANCEL_EDIT, tr("Cancel"), false);
    }

    /// The password generator under a login's fields: length, symbols, Generate.
    fn generator(&mut self, ui: &mut Ui, config: &Config) {
        let label = secondary(ui, self.detail, tr("PASSWORD GENERATOR"), TextStyle::Caption);
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
        ui.add_label(row_node, tr("Length"), TextStyle::Body, Style::default());
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
        ui.add_label(row_node, tr("Symbols"), TextStyle::Body, Style::default());
        self.generator_symbols =
            Some(ui.add(row_node, Widget::toggle(config.passwords.generator_symbols), Style::default()));
        ui.add(row_node, Widget::container(), grow());
        pill(ui, row_node, GENERATE, tr("Generate"), false);
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
                Some(tr("Add a website and a password."))
            }
            ItemData::Card(c) if c.number.is_empty() => Some(tr("Add the card number.")),
            ItemData::Address(_) | ItemData::Note(_)
                if (0..fields(kind).len()).all(|i| field(&data, i).is_empty()) =>
            {
                Some(tr("Fill in at least one field."))
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
        self.sync_related(config);
        if let Some(vault) = &mut self.vault {
            vault.touch();
        }
        match event {
            UiEvent::Submitted { node, .. } if node == self.alert_field => {
                if ui.is_visible(self.confirm_box) {
                    return Outcome::Events(ui.focus(self.confirm_field));
                }
                self.try_unlock(ui, config, clipboard)
            }
            UiEvent::Submitted { node, .. } if node == self.confirm_field => {
                self.try_unlock(ui, config, clipboard)
            }
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
            // Not showing a card's details after all: the sheet stays.
            CANCEL_UNLOCK if matches!(self.asking, Some(Reauth::Reveal(_) | Reauth::Copy(_))) => {
                self.asking = None;
                ui.set_field_text(self.alert_field, "");
                return self.refresh(ui, config);
            }
            CLOSE | CANCEL_UNLOCK => self.close(ui, config, self.address_origin()),
            UNLOCK => return self.try_unlock(ui, config, clipboard),
            // Locks the whole account (private tabs too), like the account menu's Lock.
            LOCK => return Outcome::LockSession,
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
                match &self.prompt.state {
                    Some(PromptState::Unlock { url, .. }) => {
                        self.unlock_dismissed.insert(site::display_host(url));
                    }
                    // Only these exact credentials: another password prompts again.
                    Some(PromptState::Save(p)) => {
                        self.declines.add(
                            &site::display_host(&p.url),
                            &p.username,
                            &p.password,
                            Instant::now(),
                        );
                    }
                    _ => {}
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
                        let address = self.address_origin();
                        return self.open(ui, config, window, address, address);
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
                if on && self.asks_every_time(i) {
                    self.asking = Some(Reauth::Reveal(i));
                    return self.refresh(ui, config);
                }
                if matches!(self.shown, Detail::Edit { .. }) {
                    self.set_revealed(ui, i, on);
                } else {
                    self.set_revealed(ui, i, on);
                    self.show_detail(ui, config);
                }
            }
            ActionId(n) if (COPY_BASE..COPY_BASE + MAX_FIELDS).contains(&n) => {
                let i = (n - COPY_BASE) as usize;
                if self.asks_every_time(i) {
                    self.asking = Some(Reauth::Copy(i));
                    return self.refresh(ui, config);
                }
                self.copy_field(config, clipboard, i);
                self.show_detail(ui, config);
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

    /// Whether field `i` of the item shown (or edited) asks for the password every time.
    fn asks_every_time(&self, i: usize) -> bool {
        let kind = match &self.shown {
            Detail::View(id) => self.vault.as_ref().and_then(|v| v.get(id)).map(|item| item.data.kind()),
            Detail::Edit { kind, .. } => Some(*kind),
            Detail::Empty => None,
        };
        kind.is_some_and(|kind| fields(kind).get(i).is_some_and(|&(_, secret)| asks_every_time(kind, secret)))
    }

    /// Copies field `i` of the item shown.
    fn copy_field(&mut self, config: &Config, clipboard: &mut Option<arboard::Clipboard>, i: usize) {
        let Detail::View(id) = &self.shown else { return };
        let value = self
            .vault
            .as_ref()
            .and_then(|v| v.get(id))
            .map(|item| Zeroizing::new(field(&item.data, i).to_owned()));
        if let Some(value) = value {
            self.copy(config, clipboard, &value);
            self.copied_field = Some(i);
        }
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
        self.last_fill = Some((site::display_host(&url), login.username.clone(), Instant::now()));
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
        self.sync_related(config);
        match event {
            PageEvent::Form | PageEvent::IdentifierForm => {
                // The sign-in form is back after a submit: the login most likely failed.
                if self.held.as_ref().is_some_and(|h| h.view == view) {
                    self.held = None;
                }
                if !config.passwords.autofill || matches!(self.prompt.state, Some(PromptState::Save(_))) {
                    return;
                }
                let Some(vault) = &self.vault else { return };
                if !vault.is_unlocked() {
                    // Locked: only ask when the vault's keyed site index says this site has
                    // logins (or has no index yet, a vault from before it: the first unlock
                    // builds it). Sites without saved logins never prompt, unless the user
                    // said "Not Now" for this site.
                    if vault.is_set_up() && !vault.device_ready() {
                        // Not loaded yet (or the TPM failed): no prompt now rather than the UI
                        // waiting for the TPM; the next form asks again.
                        vault.load_device_key_in_background();
                    } else if vault.is_set_up()
                        && vault.site_known(url) != Some(false)
                        && !self.unlock_dismissed.contains(&site::display_host(url))
                    {
                        self.prompt.state = Some(PromptState::Unlock { view, url: url.to_owned() });
                        self.build_prompt(ui, config);
                    } else if matches!(self.prompt.state, Some(PromptState::Unlock { .. })) {
                        self.prompt.hide();
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
            PageEvent::Identifier { username } => {
                // First step of a two-step sign-in: the password comes on the next page.
                self.step_user = Some(StepUser { username, at: Instant::now() });
            }
            PageEvent::Submit { mut username, password } => {
                // A password-only page after a user-name-only one: the user name is the one
                // typed before (if recent), unless this page asked for one itself.
                let remembered = self.step_user.take();
                if username.is_empty()
                    && let Some(step) = remembered
                    && step.at.elapsed()
                        <= Duration::from_secs_f32(config.passwords.two_step_memory_seconds.max(0.0))
                {
                    username = step.username;
                }
                if !config.passwords.offer_to_save || self.vault.is_none() {
                    return;
                }
                self.held = None;
                let host = site::display_host(url);
                let stored = self
                    .vault
                    .as_ref()
                    .filter(|v| v.is_unlocked())
                    .and_then(|v| v.login_for(url, &username))
                    .and_then(|i| match &i.data {
                        ItemData::Login(l) => Some(l.password.to_string()),
                        _ => None,
                    });
                let memory = Duration::from_secs_f32(config.passwords.decline_memory_seconds.max(0.0));
                let declined = self.declines.contains(&host, &username, &password, Instant::now(), memory);
                let Some(offer) = offer_for(stored.as_deref(), &password, declined) else { return };
                let pending = PendingSave { url: url.to_owned(), username, password };
                let fill_window = Duration::from_secs_f32(config.passwords.fill_recent_seconds.max(0.0));
                let just_filled = self.last_fill.as_ref().is_some_and(|(h, u, at)| {
                    *h == host && *u == pending.username && at.elapsed() <= fill_window
                });
                if offer == Offer::Change && just_filled {
                    // The filled password was replaced by another one: a typo until the page
                    // shows otherwise (see `release_held`).
                    self.held = Some(Held { view, pending, at: Instant::now() });
                    return;
                }
                self.prompt.state = Some(PromptState::Save(pending));
                self.build_prompt(ui, config);
            }
        }
    }

    /// A held submit whose page did not bring the sign-in form back in time worked: offer it.
    fn release_held(&mut self, ui: &mut Ui, config: &Config) {
        let wait = Duration::from_secs_f32(config.passwords.login_check_seconds.max(0.0));
        if self.held.as_ref().is_some_and(|h| h.at.elapsed() >= wait)
            && let Some(held) = self.held.take()
        {
            if self.open || matches!(self.prompt.state, Some(PromptState::Save(_))) {
                return;
            }
            self.prompt.state = Some(PromptState::Save(held.pending));
            self.build_prompt(ui, config);
        }
    }

    /// Offers the logins saved for `url`. Returns false when there are none.
    fn show_fill(&mut self, ui: &mut Ui, config: &Config, view: ViewId, url: &str) -> bool {
        let ids: Vec<String> = match &self.vault {
            Some(vault) if vault.is_unlocked() => {
                let logins = vault.logins_for(url);
                // After a two-step sign-in's first step: the login of the user name just typed.
                let typed = self.recent_step_user(config).map(str::to_lowercase);
                let chosen: Vec<_> = typed
                    .map(|name| {
                        logins
                            .iter()
                            .filter(|i| {
                                matches!(&i.data, ItemData::Login(l) if l.username.to_lowercase() == name)
                            })
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default();
                let list = if chosen.is_empty() { logins } else { chosen };
                list.iter().take(PROMPT_LOGINS).map(|i| i.id.clone()).collect()
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
                self.prompt.set_error(ui, config, Some(&describe(&e)));
                Outcome::Events(ui.focus(field))
            }
        }
    }

    fn build_prompt(&mut self, ui: &mut Ui, config: &Config) {
        let root = self.prompt.root;
        ui.clear_children(root);
        self.prompt.field = None;
        self.prompt.error = None;
        let header = ui.add(root, Widget::container(), fixed(PROMPT_HEADER, padding(6.0, 0.0), 10.0));
        let mut height = PROMPT_HEADER;
        ui.add(header, Widget::icon(Icon::Key, 20.0).with_paint(Paint::Accent), Style::default());
        let titles = ui.add(header, Widget::container(), Style { ..column(0.0) });
        let mut style = ui.style(titles);
        style.flex_grow = 1.0;
        style.min_size = Size { width: length(0.0), height: auto() };
        ui.set_style(titles, style);
        match &self.prompt.state {
            Some(PromptState::Fill { url, ids, .. }) => {
                ui.add_label(titles, tr("Fill with Axion"), TextStyle::BodyEmphasis, Style::default());
                secondary(ui, titles, &site::display_host(url), TextStyle::Footnote);
                icon_button(ui, header, PROMPT_CLOSE, Icon::Close, 28.0);
                for (i, id) in ids.iter().enumerate() {
                    let Some(ItemData::Login(login)) =
                        self.vault.as_ref().and_then(|v| v.get(id)).map(|i| &i.data)
                    else {
                        continue;
                    };
                    // Rows directly on the glass; the hovered one gets a capsule highlight.
                    let row_node = ui.add(
                        root,
                        Widget::container()
                            .with_action(ActionId(PROMPT_FILL_BASE + i as u32))
                            .with_radius(CAPSULE),
                        fixed(PROMPT_ROW, padding(12.0, 0.0), 10.0),
                    );
                    height += PROMPT_GAP_ROWS + PROMPT_ROW;
                    ui.add(
                        row_node,
                        Widget::icon(Icon::Key, 16.0).with_paint(Paint::Secondary),
                        Style::default(),
                    );
                    let name =
                        if login.username.is_empty() { tr("No user name") } else { login.username.as_str() };
                    ui.add_label(row_node, name, TextStyle::Body, grow());
                    secondary(ui, row_node, HIDDEN, TextStyle::Footnote);
                }
            }
            Some(PromptState::Unlock { url, .. }) => {
                ui.add_label(titles, tr("Unlock to fill"), TextStyle::BodyEmphasis, Style::default());
                secondary(ui, titles, &site::display_host(url), TextStyle::Footnote);
                icon_button(ui, header, PROMPT_CLOSE, Icon::Close, 28.0);
                let line = ui.add(root, Widget::container(), fixed(PROMPT_LINE, padding(2.0, 0.0), 8.0));
                height += PROMPT_GAP_ROWS + PROMPT_LINE;
                let field = field_box(ui, line, tr("Account password"), Style { flex_grow: 1.0, ..row(0.0) });
                add_eye(ui, field);
                // The field takes the room; the button keeps its natural width.
                let label = capsule_button(ui, line, PROMPT_UNLOCK, tr("Unlock"), true);
                if let Some(button) = ui.parent(label) {
                    let mut style = ui.style(button);
                    style.flex_grow = 0.0;
                    ui.set_style(button, style);
                }
                let error = painted(ui, root, "", TextStyle::Footnote, Paint::Fixed(ERROR));
                ui.set_style(error, fixed(PROMPT_ERROR, padding(6.0, 0.0), 0.0));
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
                let title = if update { tr("Change Password?") } else { tr("Save password?") };
                ui.add_label(titles, title, TextStyle::BodyEmphasis, Style::default());
                let host = site::display_host(&pending.url);
                let who = if pending.username.is_empty() {
                    host
                } else {
                    format!("{} · {host}", pending.username)
                };
                secondary(ui, titles, &who, TextStyle::Footnote);
                let buttons = ui.add(root, Widget::container(), fixed(PROMPT_LINE, padding(2.0, 0.0), 8.0));
                height += PROMPT_GAP_ROWS + PROMPT_LINE;
                capsule_button(ui, buttons, PROMPT_CLOSE, tr("Not Now"), false);
                let save = if self.is_unlocked() {
                    if update { tr("Update") } else { tr("Save") }
                } else {
                    tr("Save…")
                };
                capsule_button(ui, buttons, PROMPT_SAVE, save, true);
            }
            None => return,
        }
        self.prompt.height = height;
        self.prompt.show(ui, config);
    }

    /// Hides the prompt (another tab was selected, or the page navigated away).
    pub fn hide_prompt(&mut self, view: Option<ViewId>, keep_save: bool) {
        if !keep_save {
            self.held = None;
        }
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
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32, address: Frame) {
        self.address = address;
        let phase = self.morph.animate(ui, dt);
        if !self.open && phase == Phase::Closed && ui.is_visible(self.root) {
            self.finish_close(ui);
        }
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
        self.release_held(ui, config);
        self.prompt.animate(ui, config, dt, address);
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
        // No auto-lock: an unlock lasts for the session (only Lock ends it).
        let _ = (ui, config);
        Outcome::Nothing
    }

    /// Writes a portable encrypted export (Settings › Passwords); `password` must be the
    /// account password. Returns a status line.
    pub fn export(&self, path: &Path, password: &str) -> String {
        match &self.vault {
            Some(vault) if vault.is_unlocked() => match vault.export_encrypted(path, password) {
                Ok(()) => tr_fmt!("Exported to {path}", path = path.display()),
                Err(VaultError::WrongPassword) => {
                    tr("Export failed: that isn't the account password.").to_owned()
                }
                Err(e) => {
                    tracing::error!("exporting passwords failed: {e}");
                    tr_fmt!("Export failed: {reason}", reason = describe(&e))
                }
            },
            _ => tr("Unlock Passwords first (open Passwords in the sidebar).").to_owned(),
        }
    }

    /// Restores an Axion export made with `password` (Settings › Passwords): into the unlocked
    /// vault, or, while the account has no password yet, as a new vault whose password (and so
    /// the account's) becomes the export's. Returns a status line and whether the account
    /// got its password just now.
    pub fn restore(&mut self, ui: &mut Ui, config: &Config, path: &Path, password: &str) -> (String, bool) {
        let Some(vault) = self.vault.as_mut() else {
            return (tr("Passwords are unavailable.").to_owned(), false);
        };
        let set_up = vault.is_set_up();
        if set_up && !vault.is_unlocked() {
            return (tr("Unlock Passwords first (open Passwords in the sidebar).").to_owned(), false);
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(e) => {
                return (tr_fmt!("Can't read {path}: {error}", path = path.display(), error = e), false);
            }
        };
        let result = if set_up {
            vault.import_export(&bytes, password)
        } else {
            vault.setup_from_export(&bytes, password)
        };
        match result {
            Ok(n) => {
                tracing::info!(items = n, "passwords restored from an export");
                if !set_up {
                    self.after_unlock(ui, config);
                }
                let mut status = match n {
                    0 => tr("Nothing new to restore.").to_owned(),
                    n => tr_n("Restored {n} item.", "Restored {n} items.", n),
                };
                if !set_up {
                    status.push(' ');
                    status.push_str(tr("Its password is now the account password."));
                }
                (status, !set_up)
            }
            Err(VaultError::WrongPassword) => {
                (tr("Restore failed: wrong password for this export.").to_owned(), false)
            }
            Err(e) => {
                tracing::error!("restoring passwords failed: {e}");
                (tr_fmt!("Restore failed: {reason}", reason = describe(&e)), false)
            }
        }
    }

    /// Imports a CSV export of another browser or manager. Returns a status line.
    pub fn import_csv(&mut self, path: &Path) -> String {
        let Some(vault) = self.vault.as_mut().filter(|v| v.is_unlocked()) else {
            return tr("Unlock Passwords first (open Passwords in the sidebar).").to_owned();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => Zeroizing::new(text),
            Err(e) => return tr_fmt!("Can't read {path}: {error}", path = path.display(), error = e),
        };
        match vault.import_csv(&text) {
            Ok(0) => tr("Nothing new to import.").to_owned(),
            Ok(n) => tr_n("Imported {n} item.", "Imported {n} items.", n),
            Err(e) => {
                tracing::error!("importing passwords failed: {e}");
                tr_fmt!("Import failed: {reason}", reason = e)
            }
        }
    }

    /// Capture aid (throwaway profiles only): sets up the vault with a test password and
    /// clearly fake entries.
    pub fn seed_test_data(&mut self) {
        let Some(vault) = &mut self.vault else { return };
        if !vault.is_set_up() && vault.setup(TEST_PASSWORD).is_err() {
            return;
        }
        if !vault.is_unlocked() && vault.unlock(TEST_PASSWORD).is_err() {
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
            // The first card, and asking for its number (the password is asked again).
            "card" => {
                let card =
                    self.vault.as_ref().and_then(|v| v.items().iter().find(|i| i.data.kind() == Kind::Card));
                if let Some(id) = card.map(|i| i.id.clone()) {
                    self.select(ui, config, Detail::View(id));
                }
                return Outcome::Nothing;
            }
            "reveal-card" => ActionId(REVEAL_BASE + 2),
            // The sheet's alert, answered with the test vault's password.
            "answer" => {
                ui.set_field_text(self.alert_field, TEST_PASSWORD);
                UNLOCK
            }
            "fill" => ActionId(PROMPT_FILL_BASE),
            // The unlock prompt, with the test vault's password typed in.
            "unlock" => {
                if let Some(field) = self.prompt.field {
                    ui.set_field_text(field, TEST_PASSWORD);
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
    fn build(ui: &mut Ui, bar: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            bar,
            PROMPT_RADIUS,
            Style {
                size: Size { width: length(PROMPT_WIDTH), height: auto() },
                padding: padding(PROMPT_PADDING, PROMPT_PADDING),
                ..column(PROMPT_GAP_ROWS)
            },
        );
        let root = popover.root();
        Self { popover, root, state: None, field: None, error: None, height: 0.0, address: Frame::default() }
    }

    fn hide(&mut self) {
        self.state = None;
    }

    /// Centred under the address field, flowing out of it (or gliding to a new size).
    fn show(&mut self, ui: &mut Ui, config: &Config) {
        let a = self.address;
        if a.size.x <= 0.0 {
            return;
        }
        let height = self.height + 2.0 * PROMPT_PADDING;
        let left = (a.origin.x + (a.size.x - PROMPT_WIDTH) / 2.0).max(PROMPT_MARGIN);
        let target = Frame::new(left, a.origin.y + a.size.y + PROMPT_GAP, PROMPT_WIDTH, height);
        self.popover.open(ui, config, a, target);
    }

    /// Shows or hides the error line under the password field (the prompt grows or shrinks).
    fn set_error(&mut self, ui: &mut Ui, config: &Config, text: Option<&str>) {
        let Some(error) = self.error else { return };
        let shown = ui.is_visible(error);
        if let Some(text) = text {
            ui.set_text(error, text);
        }
        if shown != text.is_some() {
            ui.set_visible(error, text.is_some());
            let delta = PROMPT_GAP_ROWS + PROMPT_ERROR;
            self.height += if text.is_some() { delta } else { -delta };
            self.show(ui, config);
        }
    }

    fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32, address: Frame) {
        let moved = address != self.address;
        self.address = address;
        if self.state.is_none() {
            if self.popover.is_open() {
                self.popover.close(ui, config);
            }
        } else if moved && self.popover.is_open() {
            self.show(ui, config);
        }
        if self.popover.animate(ui, dt) == Phase::Closed && self.state.is_none() {
            ui.clear_children(self.root);
            self.field = None;
            self.error = None;
        }
    }
}

/// What to tell the user about a vault error.
pub fn describe(e: &VaultError) -> String {
    match e {
        VaultError::WrongPassword => tr("Wrong password").to_owned(),
        VaultError::OtherDevice => {
            tr("These passwords belong to another device. Restore them from an Axion \
                                    export in Settings › Passwords.")
            .to_owned()
        }
        e => {
            let text = e.to_string();
            let mut chars = text.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_card_secrets_ask_every_time() {
        for (i, &(label, secret)) in fields(Kind::Card).iter().enumerate() {
            let expected = matches!(label, "Card number" | "Security code");
            assert_eq!(asks_every_time(Kind::Card, secret), expected, "card field {i} ({label})");
        }
        // A login's password fills without asking again; notes and addresses don't either.
        for kind in [Kind::Login, Kind::Address, Kind::Note] {
            for &(_, secret) in fields(kind) {
                assert!(!asks_every_time(kind, secret));
            }
        }
    }

    #[test]
    fn offer_decisions() {
        // Same credentials as saved: nothing. New login: save. Other password: change.
        assert_eq!(offer_for(Some("pw"), "pw", false), None);
        assert_eq!(offer_for(None, "pw", false), Some(Offer::Save));
        assert_eq!(offer_for(Some("old"), "new", false), Some(Offer::Change));
        // A declined exact credential stays quiet, for a save and for a change alike.
        assert_eq!(offer_for(None, "pw", true), None);
        assert_eq!(offer_for(Some("old"), "new", true), None);
    }

    #[test]
    fn decline_is_remembered_for_the_exact_credential_only() {
        let (t0, memory) = (Instant::now(), Duration::from_secs(60));
        let mut d = Declines::default();
        d.add("a.com", "me", "wrong", t0);
        assert!(d.contains("a.com", "me", "wrong", t0, memory));
        // The correct password after the typo, another user, another site: all prompt again.
        assert!(!d.contains("a.com", "me", "right", t0, memory));
        assert!(!d.contains("a.com", "you", "wrong", t0, memory));
        assert!(!d.contains("b.com", "me", "wrong", t0, memory));
        assert_eq!(
            offer_for(None, "right", d.contains("a.com", "me", "right", t0, memory)),
            Some(Offer::Save)
        );
        // Short-lived.
        assert!(!d.contains("a.com", "me", "wrong", t0 + Duration::from_secs(61), memory));
    }
}
