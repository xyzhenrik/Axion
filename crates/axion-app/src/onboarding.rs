//! The first-run introduction: a full-window Liquid Glass sheet with five steps, like the
//! Setup Assistant of macOS or Safari's intro on iPadOS. Welcome and language, appearance,
//! tabs and sidebar, glass or flat, and the account. When other browsers are installed a sixth
//! step, "Bring your bookmarks", sits before the account: importing there comes right before
//! the last step, and the account step stays the one that ends with "Start Browsing".
//!
//! Every choice is applied to the real interface behind the sheet while the sheet is open
//! (the browser writes it into the account's config through the usual settings path), so the
//! effect can be seen at once; Back keeps what was chosen. The sheet is rebuilt whenever a
//! choice or the language changes, which keeps all its texts, pictures and selection marks
//! consistent without any per-widget bookkeeping.
//!
//! Three pieces:
//! - [`Flow`]: the step state machine (next / back / skip / done), plain data;
//! - [`decide`]: whether a start shows the introduction at all (first run only);
//! - [`Onboarding`]: the widgets, the transitions between steps and the keyboard focus.

use axion_compositor::{Icon, IconWeight, Rect as Frame};
use axion_config::{ACCENT_SWATCHES, Color, Config, GlassStyle, TabBarStyle, ThemeMode, UiLanguage};
use axion_i18n::{tr, tr_fmt};
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{
    ActionId, Align, Background, CAPSULE, GlassLevel, Paint, Shadow, TextStyle, Ui, UiEvent, Widget,
};
use glam::Vec2;
use zeroize::Zeroizing;

use crate::home::Image;
use crate::passwords::MIN_MASTER_LENGTH;

// ---- the flow ----

/// The steps, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The app icon, a welcome and the interface language.
    Welcome,
    /// Light, dark or automatic, and the accent colour.
    Appearance,
    /// Compact or separate tab bar; docked or floating sidebar.
    Layout,
    /// Liquid Glass or flat, and the Reduce Transparency / Motion switches.
    Glass,
    /// Bookmarks and history of the other browsers found on this computer (only offered when
    /// there are some).
    Import,
    /// Name, picture and an optional password of the local account.
    Account,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Welcome, Step::Appearance, Step::Layout, Step::Glass, Step::Account];
    /// The steps when there is something to import.
    pub const WITH_IMPORT: [Step; 6] =
        [Step::Welcome, Step::Appearance, Step::Layout, Step::Glass, Step::Import, Step::Account];
}

/// How the introduction ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// All steps went through ("Start Browsing").
    Finished,
    /// Left out with "Skip".
    Skipped,
}

/// Where the introduction is: a step, or over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flow {
    index: usize,
    ending: Option<Ending>,
    /// Whether the import step is part of the flow.
    import: bool,
}

impl Flow {
    /// Starts at step `index` (counting from 0; clamped to the last step); `import`: the
    /// import step is part of the flow.
    pub fn at(index: usize, import: bool) -> Self {
        let mut flow = Self { index: 0, ending: None, import };
        flow.index = index.min(flow.steps().len() - 1);
        flow
    }

    /// The steps of this flow, in order.
    pub fn steps(&self) -> &'static [Step] {
        if self.import { &Step::WITH_IMPORT } else { &Step::ALL }
    }

    pub fn step(&self) -> Step {
        self.steps()[self.index]
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn is_first(&self) -> bool {
        self.index == 0
    }

    pub fn is_last(&self) -> bool {
        self.index + 1 == self.steps().len()
    }

    #[cfg(test)]
    pub fn ending(&self) -> Option<Ending> {
        self.ending
    }

    /// Continue: the next step, or the end after the last one. Returns whether anything changed.
    pub fn next(&mut self) -> bool {
        if self.ending.is_some() {
            return false;
        }
        if self.is_last() {
            self.ending = Some(Ending::Finished);
        } else {
            self.index += 1;
        }
        true
    }

    /// Back: the previous step (nothing on the first). Returns whether the step changed.
    pub fn back(&mut self) -> bool {
        if self.ending.is_some() || self.is_first() {
            return false;
        }
        self.index -= 1;
        true
    }

    /// Skip: the end, from any step.
    pub fn skip(&mut self) -> bool {
        if self.ending.is_some() {
            return false;
        }
        self.ending = Some(Ending::Skipped);
        true
    }

    /// Jumps to a step (capture aid).
    pub fn goto(&mut self, index: usize) {
        if self.ending.is_none() {
            self.index = index.min(self.steps().len() - 1);
        }
    }
}

// ---- first run ----

/// What the start of the browser knows about whether this is a first run.
#[derive(Clone, Copy, Debug, Default)]
pub struct FirstRun {
    /// `--onboarding`, or Settings › Show Welcome Tour: show it whatever else is true.
    pub forced: bool,
    /// `general.onboarding_done` in the account's config.
    pub done: bool,
    /// An earlier start began the introduction and was closed before it ended.
    pub started: bool,
    /// `accounts.toml` existed before this start.
    pub accounts_file: bool,
    /// The account's `config.toml` existed before this start.
    pub config_file: bool,
    /// The profile has history, bookmarks or saved tabs.
    pub browsing_data: bool,
    /// A `--capture` run (screenshots of other features must not show the introduction).
    pub capture: bool,
}

/// What to do with the introduction at the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    Show,
    /// Not now, and not recorded.
    Skip,
    /// An existing user: remember that the introduction is done (it was added after they began).
    MarkDone,
}

/// Decides whether this start shows the introduction. It shows once, on a fresh install; any
/// sign of earlier use (an accounts list, a settings file, history, bookmarks, saved tabs)
/// means the user is not new.
pub fn decide(first_run: &FirstRun) -> Launch {
    if first_run.forced {
        return Launch::Show;
    }
    if first_run.done {
        return Launch::Skip;
    }
    // Closed in the middle of the introduction: it comes back.
    if first_run.started {
        return if first_run.capture { Launch::Skip } else { Launch::Show };
    }
    if first_run.accounts_file || first_run.config_file || first_run.browsing_data {
        return Launch::MarkDone;
    }
    if first_run.capture { Launch::Skip } else { Launch::Show }
}

// ---- actions ----

const BASE: u32 = 11_000;
const CONTINUE: ActionId = ActionId(BASE);
const BACK: ActionId = ActionId(BASE + 1);
const SKIP: ActionId = ActionId(BASE + 2);
const CHOOSE_PHOTO: ActionId = ActionId(BASE + 3);
const REMOVE_PHOTO: ActionId = ActionId(BASE + 4);
const LANGUAGE_BASE: u32 = BASE + 10;
const THEME_BASE: u32 = BASE + 20;
const ACCENT_BASE: u32 = BASE + 30;
const TAB_BAR_BASE: u32 = BASE + 40;
const SIDEBAR_BASE: u32 = BASE + 50;
const GLASS_BASE: u32 = BASE + 60;
const IMPORT_BASE: u32 = BASE + 70;
/// Browsers listed in the import step.
pub const MAX_SOURCES: usize = 4;

const LANGUAGES: [(&str, UiLanguage); 3] =
    [("System", UiLanguage::System), ("English", UiLanguage::English), ("Deutsch", UiLanguage::German)];
const THEMES: [ThemeMode; 3] = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Auto];
const TAB_BARS: [TabBarStyle; 2] = [TabBarStyle::Compact, TabBarStyle::Separate];
const GLASS_STYLES: [GlassStyle; 2] = [GlassStyle::Liquid, GlassStyle::Flat];

/// A choice that changes the configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Language(UiLanguage),
    Theme(ThemeMode),
    /// Index into the accent swatches.
    Accent(usize),
    TabBar(TabBarStyle),
    FloatSidebar(bool),
    Glass(GlassStyle),
    ReduceTransparency(bool),
    ReduceMotion(bool),
}

impl Pick {
    /// Writes the choice into `config`; returns whether the interface language changed.
    pub fn apply(self, config: &mut Config) {
        match self {
            Pick::Language(language) => config.general.language = language,
            Pick::Theme(mode) => config.appearance.theme = mode,
            Pick::Accent(i) => config.appearance.accent = ACCENT_SWATCHES[i.min(ACCENT_SWATCHES.len() - 1)],
            Pick::TabBar(style) => config.layout.tab_bar = style,
            Pick::FloatSidebar(float) => config.layout.float_sidebar = float,
            Pick::Glass(style) => config.glass.style = style,
            Pick::ReduceTransparency(on) => config.glass.reduce_transparency = on,
            Pick::ReduceMotion(on) => config.glass.reduce_motion = on,
        }
    }
}

/// A click in the sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Click {
    Continue,
    Back,
    Skip,
    ChoosePhoto,
    RemovePhoto,
    /// Selects or deselects the browser at this index in the import step.
    Import(usize),
    Pick(Pick),
}

fn click_of(id: ActionId) -> Option<Click> {
    let n = id.0;
    let index = |base: u32, len: usize| n.checked_sub(base).map(|i| i as usize).filter(|&i| i < len);
    Some(match id {
        CONTINUE => Click::Continue,
        BACK => Click::Back,
        SKIP => Click::Skip,
        CHOOSE_PHOTO => Click::ChoosePhoto,
        REMOVE_PHOTO => Click::RemovePhoto,
        _ => {
            if let Some(i) = index(LANGUAGE_BASE, LANGUAGES.len()) {
                Click::Pick(Pick::Language(LANGUAGES[i].1))
            } else if let Some(i) = index(THEME_BASE, THEMES.len()) {
                Click::Pick(Pick::Theme(THEMES[i]))
            } else if let Some(i) = index(ACCENT_BASE, ACCENT_SWATCHES.len()) {
                Click::Pick(Pick::Accent(i))
            } else if let Some(i) = index(TAB_BAR_BASE, TAB_BARS.len()) {
                Click::Pick(Pick::TabBar(TAB_BARS[i]))
            } else if let Some(i) = index(SIDEBAR_BASE, 2) {
                Click::Pick(Pick::FloatSidebar(i == 1))
            } else if let Some(i) = index(IMPORT_BASE, MAX_SOURCES) {
                Click::Import(i)
            } else {
                let i = index(GLASS_BASE, GLASS_STYLES.len())?;
                Click::Pick(Pick::Glass(GLASS_STYLES[i]))
            }
        }
    })
}

// ---- what the browser hands in and takes out ----

/// The current account as the account step shows it.
#[derive(Clone)]
pub struct AccountView {
    pub name: String,
    pub initials: String,
    pub color: Color,
    pub picture: Option<Image>,
}

impl Default for AccountView {
    fn default() -> Self {
        Self { name: String::new(), initials: String::new(), color: ACCENT_SWATCHES[0], picture: None }
    }
}

/// What the account step collected when the introduction finishes.
pub struct AccountChoice {
    pub name: String,
    /// The password to set, if one was typed.
    pub password: Option<Zeroizing<String>>,
}

/// The result of a click, for the browser to act on.
pub enum Reaction {
    Nothing,
    /// The configuration changed (apply and save it); the language may have changed too.
    Config(Pick),
    /// The step changed; nothing else to do.
    Moved,
    ChoosePhoto,
    RemovePhoto,
    /// The import step was left with these browsers (indexes into the offered ones) selected:
    /// start importing their bookmarks and history. The step has moved on.
    Import(Vec<usize>),
    /// The introduction is over.
    End(Ending, Option<AccountChoice>),
}

// ---- constants of the design ----

const ROOT_MARGIN: f32 = 24.0;
const MIN_CARD: Vec2 = Vec2::new(360.0, 300.0);
const CARD_PAD_X: f32 = 40.0;
const CARD_PAD_TOP: f32 = 34.0;
const CARD_PAD_BOTTOM: f32 = 26.0;
const BUTTON_HEIGHT: f32 = 40.0;
const BUTTON_MIN_WIDTH: f32 = 120.0;
const TILE_RADIUS: f32 = 20.0;
const TILE_PAD: f32 = 10.0;
const ICON_SIDE: f32 = 112.0;
const AVATAR_SIDE: f32 = 84.0;
/// How far a step slides in from the side (logical px).
const SLIDE_DISTANCE: f32 = 56.0;
/// How far the focused tile grows on every side (logical px).
const FOCUS_GROW: f32 = 4.0;
const ERROR: Color = Color::rgb(0xD0362B);

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

fn margin(left: f32, right: f32, top: f32, bottom: f32) -> Rect<LengthPercentageAuto> {
    Rect { left: length(left), right: length(right), top: length(top), bottom: length(bottom) }
}

fn pad(h: f32, v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(h), right: length(h), top: length(v), bottom: length(v) }
}

fn dims(w: f32, h: f32) -> Size<Dimension> {
    Size { width: length(w), height: length(h) }
}

fn column(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        gap: Size { width: length(0.0), height: length(gap) },
        ..Default::default()
    }
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

/// A plain rounded box of a fixed colour.
fn block(ui: &mut Ui, parent: NodeId, color: Color, opacity: f32, radius: f32, style: Style) -> NodeId {
    ui.add(
        parent,
        Widget::container()
            .with_background(Background::Fill(Paint::Fixed(color), opacity))
            .with_radius(radius),
        style,
    )
}

/// Centred text of a style and paint that may wrap onto `lines` lines, at most `max_width` wide.
fn paragraph(
    ui: &mut Ui,
    parent: NodeId,
    text: &str,
    style: TextStyle,
    paint: Paint,
    lines: usize,
    max_width: f32,
) -> NodeId {
    let label = ui.add_label(
        parent,
        text,
        style,
        Style {
            size: Size { width: percent(1.0), height: auto() },
            min_size: Size { width: length(0.0), height: auto() },
            max_size: Size { width: length(max_width), height: auto() },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    ui.set_label_lines(label, lines);
    ui.set_label_align(label, Align::Center);
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(paint);
    }
    label
}

// ---- mock-up pictures ----

/// The colours of a mini window.
#[derive(Clone, Copy)]
struct Look {
    window: Color,
    bar: Color,
    page: Color,
    sidebar: Color,
    ink: Color,
}

const LIGHT: Look = Look {
    window: Color::rgb(0xE4E4EA),
    bar: Color::rgb(0xFFFFFF),
    page: Color::rgb(0xFFFFFF),
    sidebar: Color::rgb(0xF4F4F8),
    ink: Color::rgb(0x8E8E93),
};

const DARK: Look = Look {
    window: Color::rgb(0x1A1A1C),
    bar: Color::rgb(0x3A3A3E),
    page: Color::rgb(0x2C2C2E),
    sidebar: Color::rgb(0x242426),
    ink: Color::rgb(0x8E8E93),
};

#[derive(Clone, Copy, PartialEq)]
enum SidebarKind {
    Docked,
    Floating,
}

#[derive(Clone, Copy)]
struct Mock {
    look: Look,
    tab_bar: TabBarStyle,
    sidebar: SidebarKind,
}

/// A mini browser window of `size` (the details scale with the width): the tab bar layout, a
/// sidebar and a page with a few lines of content, the open tab in the accent colour.
fn mock_window(ui: &mut Ui, parent: NodeId, size: Vec2, mock: Mock) -> NodeId {
    let s = size.x / 160.0;
    let look = mock.look;
    let window = ui.add(
        parent,
        Widget::container()
            .with_background(Background::Fill(Paint::Fixed(look.window), 1.0))
            .with_radius(9.0 * s)
            .with_shadow(Shadow { offset_y: 1.0, blur: 5.0 * s, opacity: 0.18 }),
        Style {
            size: dims(size.x, size.y),
            flex_shrink: 0.0,
            padding: pad(5.0 * s, 5.0 * s),
            gap: Size { width: length(4.0 * s), height: length(0.0) },
            ..row(4.0 * s)
        },
    );
    let lines = |ui: &mut Ui, parent: NodeId, widths: &[f32], height: f32, opacity: f32| {
        for &w in widths {
            block(
                ui,
                parent,
                look.ink,
                opacity,
                CAPSULE,
                Style {
                    size: Size { width: percent(w), height: length(height) },
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
        }
    };
    let sidebar_card = |ui: &mut Ui, parent: NodeId, style: Style| {
        let card = ui.add(
            parent,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(look.sidebar), 1.0))
                .with_radius(6.0 * s)
                .with_shadow(Shadow { offset_y: 1.0, blur: 4.0 * s, opacity: 0.16 }),
            Style {
                padding: pad(4.0 * s, 5.0 * s),
                gap: Size { width: length(0.0), height: length(3.5 * s) },
                ..style
            },
        );
        block(
            ui,
            card,
            Color::rgb(0x000000),
            0.0,
            CAPSULE,
            Style { size: dims(1.0, 0.0), ..Default::default() },
        );
        ui.add(
            card,
            Widget::container().with_background(Background::Fill(Paint::Accent, 0.9)).with_radius(CAPSULE),
            Style {
                size: Size { width: percent(0.9), height: length(3.5 * s) },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        lines(ui, card, &[0.8, 0.65, 0.75], 3.0 * s, 0.4);
    };
    if mock.sidebar == SidebarKind::Docked {
        sidebar_card(
            ui,
            window,
            Style {
                size: Size { width: length(30.0 * s), height: percent(1.0) },
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                ..Default::default()
            },
        );
    }
    let main = ui.add(
        window,
        Widget::container(),
        Style {
            flex_grow: 1.0,
            min_size: Size { width: length(0.0), height: length(0.0) },
            size: Size { width: auto(), height: percent(1.0) },
            ..column(4.0 * s)
        },
    );
    // The toolbar.
    match mock.tab_bar {
        TabBarStyle::Compact => {
            let bar = block(
                ui,
                main,
                look.bar,
                1.0,
                CAPSULE,
                Style {
                    size: Size { width: percent(1.0), height: length(11.0 * s) },
                    flex_shrink: 0.0,
                    padding: pad(4.0 * s, 0.0),
                    ..row(3.0 * s)
                },
            );
            for (grow, accent) in [(2.2, true), (1.0, false), (1.0, false)] {
                let slot = ui.add(
                    bar,
                    Widget::container(),
                    Style { flex_grow: grow, flex_basis: length(0.0), ..Default::default() },
                );
                let widget = Widget::container()
                    .with_background(if accent {
                        Background::Fill(Paint::Accent, 0.95)
                    } else {
                        Background::Fill(Paint::Fixed(look.ink), 0.35)
                    })
                    .with_radius(CAPSULE);
                ui.add(
                    slot,
                    widget,
                    Style {
                        size: Size { width: percent(1.0), height: length(4.0 * s) },
                        ..Default::default()
                    },
                );
            }
        }
        TabBarStyle::Separate => {
            let top = ui.add(
                main,
                Widget::container(),
                Style {
                    size: Size { width: percent(1.0), height: length(10.0 * s) },
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            let address = block(
                ui,
                top,
                look.bar,
                1.0,
                CAPSULE,
                Style {
                    size: Size { width: percent(0.62), height: percent(1.0) },
                    padding: pad(5.0 * s, 0.0),
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            block(
                ui,
                address,
                look.ink,
                0.45,
                CAPSULE,
                Style { size: Size { width: percent(0.7), height: length(3.0 * s) }, ..Default::default() },
            );
            let tabs = ui.add(
                main,
                Widget::container(),
                Style {
                    size: Size { width: percent(1.0), height: length(8.0 * s) },
                    flex_shrink: 0.0,
                    ..row(3.0 * s)
                },
            );
            for accent in [true, false, false] {
                let tab = ui.add(
                    tabs,
                    Widget::container()
                        .with_background(if accent {
                            Background::Fill(Paint::Accent, 0.95)
                        } else {
                            Background::Fill(Paint::Fixed(look.bar), 1.0)
                        })
                        .with_radius(CAPSULE),
                    Style {
                        flex_grow: 1.0,
                        flex_basis: length(0.0),
                        size: Size { width: auto(), height: percent(1.0) },
                        padding: pad(4.0 * s, 0.0),
                        justify_content: JustifyContent::CENTER,
                        ..row(0.0)
                    },
                );
                block(
                    ui,
                    tab,
                    if accent { Color::rgb(0xFFFFFF) } else { look.ink },
                    if accent { 0.8 } else { 0.4 },
                    CAPSULE,
                    Style {
                        size: Size { width: percent(0.6), height: length(2.5 * s) },
                        ..Default::default()
                    },
                );
            }
        }
    }
    // The page.
    let page = block(
        ui,
        main,
        look.page,
        1.0,
        6.0 * s,
        Style {
            flex_grow: 1.0,
            min_size: Size { width: length(0.0), height: length(0.0) },
            size: Size { width: percent(1.0), height: auto() },
            padding: pad(6.0 * s, 6.0 * s),
            ..column(3.5 * s)
        },
    );
    lines(ui, page, &[0.5, 0.9, 0.78], 3.5 * s, 0.38);
    ui.add(
        page,
        Widget::container().with_background(Background::Fill(Paint::Accent, 0.28)).with_radius(4.0 * s),
        Style {
            size: Size { width: percent(0.55), height: length(14.0 * s) },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    if mock.sidebar == SidebarKind::Floating {
        sidebar_card(
            ui,
            window,
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(8.0 * s), right: auto(), top: length(8.0 * s), bottom: auto() },
                size: Size { width: length(32.0 * s), height: length(size.y - 16.0 * s) },
                flex_direction: FlexDirection::Column,
                ..Default::default()
            },
        );
    }
    window
}

/// The Automatic theme's picture: a light window whose right half is dark.
fn mock_split(ui: &mut Ui, parent: NodeId, size: Vec2, tab_bar: TabBarStyle) {
    let holder = ui.add(
        parent,
        Widget::container(),
        Style { size: dims(size.x, size.y), flex_shrink: 0.0, ..Default::default() },
    );
    mock_window(ui, holder, size, Mock { look: LIGHT, tab_bar, sidebar: SidebarKind::Docked });
    let half = ui.add(
        holder,
        Widget::container().with_clip(),
        Style {
            position: Position::Absolute,
            inset: Rect { left: length(size.x / 2.0), right: auto(), top: length(0.0), bottom: auto() },
            size: dims(size.x / 2.0, size.y),
            ..Default::default()
        },
    );
    let dark = mock_window(ui, half, size, Mock { look: DARK, tab_bar, sidebar: SidebarKind::Docked });
    let mut style = ui.style(dark);
    style.position = Position::Absolute;
    style.inset = Rect { left: length(-size.x / 2.0), right: auto(), top: length(0.0), bottom: auto() };
    ui.set_style(dark, style);
}

/// A vivid backdrop with the demonstration of a glass panel (or its flat counterpart) on it.
/// `real_glass`: real refracting glass (the tile of Liquid Glass while glass is on); else a
/// translucent look-alike, so the picture stays in flat mode too.
fn glass_picture(ui: &mut Ui, parent: NodeId, size: Vec2, flat: bool, real_glass: bool) {
    let backdrop = ui.add(
        parent,
        Widget::container()
            .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x3552D6)), 1.0))
            .with_radius(14.0)
            .with_clip()
            .with_shadow(Shadow { offset_y: 1.0, blur: 6.0, opacity: 0.2 }),
        Style { size: dims(size.x, size.y), flex_shrink: 0.0, ..Default::default() },
    );
    let blob = |ui: &mut Ui, color: u32, left: f32, top: f32, side: f32| {
        block(
            ui,
            backdrop,
            Color::rgb(color),
            1.0,
            CAPSULE,
            Style {
                position: Position::Absolute,
                inset: Rect {
                    left: length(size.x * left),
                    right: auto(),
                    top: length(size.y * top),
                    bottom: auto(),
                },
                size: dims(size.y * side, size.y * side),
                ..Default::default()
            },
        );
    };
    blob(ui, 0xFF5C93, -0.04, 0.02, 0.62);
    blob(ui, 0xFFC145, 0.62, 0.40, 0.70);
    blob(ui, 0x25D0A6, 0.30, -0.18, 0.50);
    blob(ui, 0xA45CFF, 0.80, -0.12, 0.45);
    let panel_size = dims(size.x * 0.66, size.y * 0.56);
    let panel_at = |left: f32, top: f32| Rect {
        left: length(size.x * left),
        right: auto(),
        top: length(size.y * top),
        bottom: auto(),
    };
    let panel = if flat {
        ui.add(
            backdrop,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0xF2F2F7)), 1.0))
                .with_radius(12.0)
                .with_shadow(Shadow { offset_y: 1.0, blur: 3.0, opacity: 0.25 }),
            Style {
                position: Position::Absolute,
                inset: panel_at(0.17, 0.22),
                size: panel_size,
                padding: pad(12.0, 12.0),
                ..column(7.0)
            },
        )
    } else if real_glass {
        ui.add(
            backdrop,
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(18.0),
            Style {
                position: Position::Absolute,
                inset: panel_at(0.17, 0.22),
                size: panel_size,
                padding: pad(12.0, 12.0),
                ..column(7.0)
            },
        )
    } else {
        // Glass look-alike for flat mode: translucent white with a bright rim.
        let rim = block(
            ui,
            backdrop,
            Color::rgb(0xFFFFFF),
            0.55,
            18.0,
            Style {
                position: Position::Absolute,
                inset: panel_at(0.17, 0.22),
                size: panel_size,
                padding: Rect {
                    left: length(1.5),
                    right: length(1.5),
                    top: length(1.5),
                    bottom: length(1.5),
                },
                ..Default::default()
            },
        );
        ui.add(
            rim,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x6A82E8)), 0.55))
                .with_radius(16.5),
            Style {
                size: Size { width: percent(1.0), height: percent(1.0) },
                padding: pad(12.0, 12.0),
                ..column(7.0)
            },
        )
    };
    let ink = if flat { Color::rgb(0x8E8E93) } else { Color::rgb(0xFFFFFF) };
    block(
        ui,
        panel,
        ink,
        if flat { 0.55 } else { 0.9 },
        CAPSULE,
        Style {
            size: Size { width: percent(0.55), height: length(7.0) },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    block(
        ui,
        panel,
        ink,
        if flat { 0.35 } else { 0.55 },
        CAPSULE,
        Style {
            size: Size { width: percent(0.9), height: length(5.0) },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    block(
        ui,
        panel,
        ink,
        if flat { 0.35 } else { 0.55 },
        CAPSULE,
        Style {
            size: Size { width: percent(0.72), height: length(5.0) },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    // A small round button beside the panel: glass drops merge, flat ones do not.
    let knob_side = size.y * 0.26;
    let knob_style = Style {
        position: Position::Absolute,
        inset: panel_at(0.74, 0.58),
        size: dims(knob_side, knob_side),
        ..Default::default()
    };
    if flat {
        ui.add(
            backdrop,
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0xF2F2F7)), 1.0))
                .with_radius(CAPSULE)
                .with_shadow(Shadow { offset_y: 1.0, blur: 3.0, opacity: 0.25 }),
            knob_style,
        );
    } else if real_glass {
        ui.add(
            backdrop,
            Widget::container().with_background(Background::Glass(GlassLevel::Regular)).with_radius(CAPSULE),
            knob_style,
        );
    } else {
        block(ui, backdrop, Color::rgb(0xFFFFFF), 0.4, CAPSULE, knob_style);
    }
}

// ---- the sheet ----

/// What a control registers for the keyboard.
#[derive(Clone, Copy)]
enum Target {
    Action(ActionId),
    Field(NodeId),
}

/// A control Tab can reach.
#[derive(Clone, Copy)]
struct Entry {
    node: NodeId,
    target: Target,
    base: Background,
}

/// Text typed in the account step, kept while the sheet is rebuilt.
#[derive(Default)]
struct Draft {
    name: String,
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    /// Whether the name was touched (else it follows the account's current name).
    name_set: bool,
}

#[derive(Clone, Copy)]
struct Fields {
    name: NodeId,
    password: NodeId,
    confirm: NodeId,
}

/// A browser offered in the import step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportChoice {
    /// "Google Chrome · Person 1".
    pub label: String,
    /// What it holds ("Bookmarks and history").
    pub detail: String,
    /// Checked at first: the most recently used browser.
    pub selected: bool,
}

pub struct Onboarding {
    root: NodeId,
    card: NodeId,
    body: Option<NodeId>,
    flow: Flow,
    open: bool,
    window: Vec2,
    appear: Animated<f32>,
    /// 1 → 0 while a step slides in from the right, -1 → 0 from the left.
    slide: Animated<f32>,
    /// The language generation the sheet was built for.
    generation: u32,
    entries: Vec<Entry>,
    focus: Option<usize>,
    icon: Option<Image>,
    account: AccountView,
    draft: Draft,
    fields: Option<Fields>,
    reduce_transparency: Option<NodeId>,
    reduce_motion: Option<NodeId>,
    /// What the account step says is wrong.
    error: Option<String>,
    /// Whether the account has a password already (the step then leaves it alone).
    has_password: bool,
    max_size: Vec2,
    dim: f32,
    /// Space between neighbouring glass controls: wider than `glass.merge_distance`, so they
    /// never bridge at rest (only a press merges them).
    apart: f32,
    /// The other browsers found (the import step exists when there are any) and whether the
    /// import of the current picks was started already.
    sources: Vec<ImportChoice>,
    import_started: bool,
}

impl Onboarding {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container().with_background(Background::Fill(Paint::Ground, 0.3)).without_feedback(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let card = ui.add(
            root,
            Widget::container(),
            Style { flex_direction: FlexDirection::Column, ..Default::default() },
        );
        ui.set_visible(root, false);
        Self {
            root,
            card,
            body: None,
            flow: Flow::at(0, false),
            open: false,
            window: Vec2::new(1280.0, 800.0),
            appear: Animated::new(0.0, Spring::SNAPPY),
            slide: Animated::new(0.0, Spring::SNAPPY),
            generation: axion_i18n::generation(),
            entries: Vec::new(),
            focus: None,
            icon: None,
            account: AccountView::default(),
            draft: Draft::default(),
            fields: None,
            reduce_transparency: None,
            reduce_motion: None,
            error: None,
            has_password: false,
            max_size: Vec2::new(880.0, 640.0),
            dim: 0.3,
            apart: 28.0,
            sources: Vec::new(),
            import_started: false,
        }
    }

    /// The browsers whose data can be imported. With none, the introduction has no import
    /// step; set before [`Self::open`].
    pub fn set_sources(&mut self, sources: Vec<ImportChoice>) {
        self.sources = sources;
        self.import_started = false;
    }

    /// Indexes of the browsers that are checked (and shown).
    fn picked(&self) -> Vec<usize> {
        self.sources
            .iter()
            .enumerate()
            .take(MAX_SOURCES)
            .filter(|(_, s)| s.selected)
            .map(|(i, _)| i)
            .collect()
    }

    /// The steps this introduction goes through.
    #[cfg(test)]
    pub fn steps(&self) -> &'static [Step] {
        self.flow.steps()
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The app icon shown on the first step.
    pub fn set_icon(&mut self, icon: Option<Image>) {
        self.icon = icon;
    }

    /// The account the account step is about (call again when its picture changed).
    pub fn set_account(&mut self, account: AccountView, has_password: bool) {
        if !self.draft.name_set {
            self.draft.name.clone_from(&account.name);
        }
        self.account = account;
        self.has_password = has_password;
    }

    /// Shows the introduction from the first step (or `step`, a capture aid).
    pub fn open(&mut self, ui: &mut Ui, config: &Config, window: Vec2, step: usize) -> Vec<UiEvent> {
        self.open = true;
        self.flow = Flow::at(step, !self.sources.is_empty());
        self.draft = Draft { name: self.account.name.clone(), ..Draft::default() };
        self.error = None;
        self.focus = None;
        self.window = window;
        self.appear.spring = spring(config);
        self.appear.snap(0.0);
        self.appear.target = 1.0;
        self.slide.snap(0.0);
        ui.set_visible(self.root, true);
        self.rebuild(ui, config);
        Vec::new()
    }

    /// Closes the sheet (it fades out).
    pub fn close(&mut self, ui: &mut Ui) {
        self.open = false;
        self.appear.target = 0.0;
        let _ = ui.blur();
    }

    /// The window changed size.
    pub fn resize(&mut self, ui: &mut Ui, config: &Config, window: Vec2) {
        if self.window != window {
            self.window = window;
            if self.open {
                self.rebuild(ui, config);
            }
        }
    }

    pub fn click(&self, id: ActionId) -> Option<Click> {
        click_of(id)
    }

    /// Whether `event` belongs to the sheet.
    pub fn wants(&self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Clicked { action, .. } => click_of(*action).is_some(),
            UiEvent::ValueChanged { node, .. } => {
                Some(*node) == self.reduce_transparency || Some(*node) == self.reduce_motion
            }
            UiEvent::Submitted { node, .. } | UiEvent::Cancelled { node } => {
                self.fields.is_some_and(|f| [f.name, f.password, f.confirm].contains(node))
            }
            _ => false,
        }
    }

    /// A toggle changed: the pick it stands for.
    pub fn toggled(&self, node: NodeId, value: f32) -> Option<Pick> {
        let on = value > 0.5;
        if Some(node) == self.reduce_transparency {
            Some(Pick::ReduceTransparency(on))
        } else if Some(node) == self.reduce_motion {
            Some(Pick::ReduceMotion(on))
        } else {
            None
        }
    }

    /// Enter in a field of the account step: on to the next field, then finish.
    pub fn submitted(&mut self, ui: &mut Ui, config: &Config, node: NodeId) -> Reaction {
        let Some(f) = self.fields else { return Reaction::Nothing };
        let next = if node == f.name {
            f.password
        } else if node == f.password {
            f.confirm
        } else {
            return self.press(ui, config, Click::Continue);
        };
        let _ = ui.focus(next);
        Reaction::Nothing
    }

    /// Takes a click; the browser applies the configuration picks.
    pub fn press(&mut self, ui: &mut Ui, config: &Config, click: Click) -> Reaction {
        match click {
            Click::Continue => {
                self.sync_draft(ui);
                if self.flow.is_last() {
                    return match self.validate() {
                        Ok(choice) => {
                            self.flow.next();
                            Reaction::End(Ending::Finished, Some(choice))
                        }
                        Err(problem) => {
                            self.error = Some(problem);
                            self.rebuild(ui, config);
                            Reaction::Nothing
                        }
                    };
                }
                let picks = if self.flow.step() == Step::Import && !self.import_started {
                    self.picked()
                } else {
                    Vec::new()
                };
                let reaction = self.go(ui, config, 1);
                if picks.is_empty() {
                    return reaction;
                }
                self.import_started = true;
                Reaction::Import(picks)
            }
            Click::Back => {
                self.sync_draft(ui);
                self.go(ui, config, -1)
            }
            Click::Skip => {
                self.flow.skip();
                Reaction::End(Ending::Skipped, None)
            }
            Click::ChoosePhoto => {
                self.sync_draft(ui);
                Reaction::ChoosePhoto
            }
            Click::RemovePhoto => {
                self.sync_draft(ui);
                Reaction::RemovePhoto
            }
            Click::Import(i) => {
                if let Some(source) = self.sources.get_mut(i) {
                    source.selected = !source.selected;
                    self.import_started = false;
                }
                self.rebuild(ui, config);
                Reaction::Moved
            }
            Click::Pick(pick) => {
                self.sync_draft(ui);
                Reaction::Config(pick)
            }
        }
    }

    fn go(&mut self, ui: &mut Ui, config: &Config, direction: i32) -> Reaction {
        let changed = if direction > 0 { self.flow.next() } else { self.flow.back() };
        if !changed {
            return Reaction::Nothing;
        }
        self.error = None;
        self.focus = None;
        self.slide.spring = spring(config);
        self.slide.snap(direction as f32);
        self.slide.target = 0.0;
        self.rebuild(ui, config);
        Reaction::Moved
    }

    /// Capture aid: jumps to a step.
    pub fn goto(&mut self, ui: &mut Ui, config: &Config, step: usize) {
        self.sync_draft(ui);
        self.flow.goto(step);
        self.error = None;
        self.focus = None;
        self.slide.snap(0.0);
        self.rebuild(ui, config);
    }

    /// Says what went wrong with the chosen picture.
    pub fn show_error(&mut self, ui: &mut Ui, config: &Config, text: &str) {
        self.error = Some(text.to_owned());
        self.rebuild(ui, config);
    }

    /// The account step's name and password, checked.
    fn validate(&self) -> Result<AccountChoice, String> {
        let name = self.draft.name.trim().to_owned();
        if name.is_empty() {
            return Err(tr("Enter a name.").to_owned());
        }
        let password = (!self.draft.password.is_empty()).then(|| self.draft.password.clone());
        if let Some(password) = &password {
            if password.chars().count() < MIN_MASTER_LENGTH {
                return Err(tr_fmt!("Use at least {count} characters.", count = MIN_MASTER_LENGTH));
            }
            if **password != *self.draft.confirm {
                return Err(tr("The new passwords don't match.").to_owned());
            }
        }
        Ok(AccountChoice { name, password })
    }

    /// Copies what is typed in the fields into the draft.
    fn sync_draft(&mut self, ui: &Ui) {
        let Some(fields) = self.fields else { return };
        if let Some(name) = ui.field_text(fields.name) {
            if name != self.draft.name {
                self.draft.name_set = true;
            }
            self.draft.name = name.to_owned();
        }
        self.draft.password = Zeroizing::new(ui.field_text(fields.password).unwrap_or_default().to_owned());
        self.draft.confirm = Zeroizing::new(ui.field_text(fields.confirm).unwrap_or_default().to_owned());
    }

    /// Capture aid: types a name into the account step.
    pub fn set_name(&mut self, ui: &mut Ui, text: &str) {
        self.draft.name = text.to_owned();
        self.draft.name_set = true;
        if let Some(fields) = self.fields {
            ui.set_field_text(fields.name, text);
        }
    }

    /// Capture aid: types a password (and its confirmation) into the account step.
    pub fn set_password(&mut self, ui: &mut Ui, text: &str) {
        self.draft.password = Zeroizing::new(text.to_owned());
        self.draft.confirm = Zeroizing::new(text.to_owned());
        if let Some(fields) = self.fields {
            ui.set_field_text(fields.password, text);
            ui.set_field_text(fields.confirm, text);
        }
    }

    // ---- keyboard ----

    /// Tab / Shift+Tab / arrows: moves the focus over the step's controls and fields.
    pub fn move_focus(&mut self, ui: &mut Ui, forward: bool) {
        if self.entries.is_empty() {
            return;
        }
        // Leaving a field keeps what was typed.
        self.sync_draft(ui);
        let _ = ui.blur();
        let n = self.entries.len();
        let next = match (self.focus, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        self.focus = Some(next);
        if let Target::Field(field) = self.entries[next].target {
            let _ = ui.focus(field);
        }
        self.apply_focus(ui);
    }

    /// The action of the focused control (Enter / Space on it).
    pub fn focused_action(&self) -> Option<ActionId> {
        let entry = self.entries.get(self.focus?)?;
        match entry.target {
            Target::Action(id) => Some(id),
            Target::Field(_) => None,
        }
    }

    pub fn has_focus(&self) -> bool {
        self.focus.is_some()
    }

    /// Whether a field of the sheet has the keyboard.
    pub fn editing(&self, ui: &Ui) -> bool {
        self.fields
            .is_some_and(|f| ui.focused().is_some_and(|n| [f.name, f.password, f.confirm].contains(&n)))
    }

    /// Marks the focused control: it grows a little and thickens its glass.
    fn apply_focus(&mut self, ui: &mut Ui) {
        for (i, entry) in self.entries.iter().enumerate() {
            let focused = self.focus == Some(i) && matches!(entry.target, Target::Action(_));
            if let Some(w) = ui.widget_mut(entry.node) {
                w.background = if focused {
                    match entry.base {
                        Background::Glass(_) => Background::Glass(GlassLevel::Regular),
                        Background::TintedGlass(_, p) => Background::TintedGlass(GlassLevel::Regular, p),
                        other => other,
                    }
                } else {
                    entry.base
                };
            }
            let frame = ui.frame(entry.node);
            let grown = focused.then(|| Frame {
                origin: frame.origin - Vec2::splat(FOCUS_GROW),
                size: frame.size + Vec2::splat(FOCUS_GROW * 2.0),
            });
            ui.set_visual_frame(entry.node, grown);
        }
    }

    // ---- animation ----

    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
        }
        // The language changed (the first step's pick): every text is rebuilt.
        if self.open && self.generation != axion_i18n::generation() {
            self.sync_draft(ui);
            self.rebuild(ui, config);
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.0);
        if !self.open && k < 0.01 {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_opacity(self.root, k);
        self.slide.step(dt);
        if let Some(body) = self.body {
            let s = self.slide.value.clamp(-1.5, 1.5);
            ui.set_opacity(body, (1.0 - s.abs()).clamp(0.0, 1.0));
            let mut style = ui.style(body);
            let offset = if config.glass.reduce_motion { 0.0 } else { s * SLIDE_DISTANCE };
            style.inset.left = length(offset);
            ui.set_style(body, style);
        }
        if self.focus.is_some() {
            self.apply_focus(ui);
        }
    }

    // ---- building ----

    /// Builds the whole sheet again for the current step, choices and language.
    pub fn rebuild(&mut self, ui: &mut Ui, config: &Config) {
        self.generation = axion_i18n::generation();
        self.max_size = Vec2::new(config.ui.onboarding_max_width, config.ui.onboarding_max_height);
        self.dim = config.ui.onboarding_dim;
        self.apart = config.glass.merge_distance + 3.0;
        let card_size =
            (self.window - Vec2::splat(ROOT_MARGIN * 2.0)).clamp(MIN_CARD, self.max_size.max(MIN_CARD));
        if let Some(w) = ui.widget_mut(self.root) {
            w.background = Background::Fill(Paint::Ground, self.dim);
        }
        ui.set_style(self.card, {
            let mut style = ui.style(self.card);
            style.size = dims(card_size.x, card_size.y);
            style.flex_direction = FlexDirection::Column;
            style.padding = Rect {
                left: length(CARD_PAD_X),
                right: length(CARD_PAD_X),
                top: length(CARD_PAD_TOP),
                bottom: length(CARD_PAD_BOTTOM),
            };
            style.gap = Size { width: length(0.0), height: length(14.0) };
            style
        });
        ui.clear_children(self.card);
        self.entries.clear();
        self.fields = None;
        self.reduce_transparency = None;
        self.reduce_motion = None;
        let inner = card_size.x - CARD_PAD_X * 2.0;

        let body = ui.add(
            self.card,
            Widget::container(),
            Style {
                position: Position::Relative,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: length(0.0) },
                size: Size { width: percent(1.0), height: auto() },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..column(14.0)
            },
        );
        self.body = Some(body);
        match self.flow.step() {
            Step::Welcome => self.welcome(ui, body, config, inner),
            Step::Appearance => self.appearance(ui, body, config, inner),
            Step::Layout => self.layout(ui, body, config, inner),
            Step::Glass => self.glass(ui, body, config, inner),
            Step::Import => self.import_step(ui, body, config, inner),
            Step::Account => self.account_step(ui, body, config, inner),
        }
        self.footer(ui, config);
        // Skip sits in the corner, over everything (a full-width row, pushed to the right).
        let corner = ui.add(
            self.card,
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: auto(), top: length(18.0), bottom: auto() },
                size: dims(card_size.x, 32.0),
                padding: pad(20.0, 0.0),
                justify_content: JustifyContent::FLEX_END,
                ..row(0.0)
            },
        );
        self.pill(ui, corner, SKIP, tr("Skip"), PillKind::Small);
        ui.layout();
        if let Some(i) = self.focus {
            if i >= self.entries.len() {
                self.focus = None;
            } else if let Target::Field(field) = self.entries[i].target {
                let _ = ui.focus(field);
            }
        }
        self.apply_focus(ui);
    }

    fn register(&mut self, ui: &Ui, node: NodeId, target: Target) {
        let base = ui.widget(node).map_or(Background::None, |w| w.background);
        self.entries.push(Entry { node, target, base });
    }

    fn title(&self, ui: &mut Ui, parent: NodeId, title: &str, subtitle: &str, inner: f32) {
        paragraph(ui, parent, title, TextStyle::LargeTitle, Paint::Label, 2, inner);
        paragraph(ui, parent, subtitle, TextStyle::Body, Paint::Secondary, 3, (inner * 0.8).min(560.0));
    }

    /// A capsule button.
    fn pill(&mut self, ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, kind: PillKind) -> NodeId {
        let (background, label_paint) = match kind {
            PillKind::Primary | PillKind::Selected => {
                (Background::TintedGlass(GlassLevel::Thin, Paint::Accent), Some(Paint::OnAccent))
            }
            PillKind::Plain | PillKind::Small => (Background::Glass(GlassLevel::Thin), None),
        };
        let height = if kind == PillKind::Small { 32.0 } else { BUTTON_HEIGHT };
        let node = ui.add(
            parent,
            Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
            Style {
                size: Size { width: auto(), height: length(height) },
                min_size: Size {
                    width: length(if matches!(kind, PillKind::Primary) { BUTTON_MIN_WIDTH } else { 0.0 }),
                    height: auto(),
                },
                flex_shrink: 0.0,
                padding: pad(if kind == PillKind::Small { 16.0 } else { 24.0 }, 0.0),
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let label = ui.add_label(
            node,
            text,
            if kind == PillKind::Small { TextStyle::Footnote } else { TextStyle::BodyEmphasis },
            Style::default(),
        );
        if let Some(paint) = label_paint
            && let Some(w) = ui.widget_mut(label)
        {
            w.set_paint(paint);
        }
        self.register(ui, node, Target::Action(action));
        node
    }

    fn footer(&mut self, ui: &mut Ui, config: &Config) {
        let _ = config;
        let footer = ui.add(
            self.card,
            Widget::container(),
            Style {
                size: Size { width: percent(1.0), height: length(BUTTON_HEIGHT + 4.0) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::SPACE_BETWEEN,
                ..row(0.0)
            },
        );
        let side = |ui: &mut Ui, align: JustifyContent| {
            ui.add(
                footer,
                Widget::container(),
                Style {
                    size: Size { width: length(150.0), height: percent(1.0) },
                    flex_shrink: 0.0,
                    justify_content: align,
                    align_items: AlignItems::CENTER,
                    ..Default::default()
                },
            )
        };
        let left = side(ui, JustifyContent::FLEX_START);
        if !self.flow.is_first() {
            self.pill(ui, left, BACK, tr("Back"), PillKind::Plain);
        }
        // The step dots.
        let dots = ui.add(footer, Widget::container(), row(8.0));
        for i in 0..self.flow.steps().len() {
            let current = i == self.flow.index();
            ui.add(
                dots,
                Widget::container()
                    .with_background(if current {
                        Background::Fill(Paint::Accent, 1.0)
                    } else {
                        Background::Fill(Paint::Label, 0.22)
                    })
                    .with_radius(CAPSULE),
                Style {
                    size: dims(if current { 22.0 } else { 8.0 }, 8.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
        }
        let right = side(ui, JustifyContent::FLEX_END);
        let text = if self.flow.is_last() { tr("Start Browsing") } else { tr("Continue") };
        self.pill(ui, right, CONTINUE, text, PillKind::Primary);
    }

    /// A choice tile: a picture, a name and a selection mark.
    #[allow(clippy::too_many_arguments)]
    fn tile(
        &mut self,
        ui: &mut Ui,
        parent: NodeId,
        action: ActionId,
        selected: bool,
        label: &str,
        detail: Option<&str>,
        picture: impl FnOnce(&mut Ui, NodeId),
    ) -> NodeId {
        let node = ui.add(
            parent,
            Widget::container().with_action(action).with_radius(TILE_RADIUS).with_background(if selected {
                Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
            } else {
                Background::Glass(GlassLevel::Thin)
            }),
            Style {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::CENTER,
                flex_shrink: 0.0,
                padding: pad(TILE_PAD, TILE_PAD),
                gap: Size { width: length(0.0), height: length(6.0) },
                ..Default::default()
            },
        );
        picture(ui, node);
        let name = ui.add_label(node, label, TextStyle::Callout, Style::default());
        if selected && let Some(w) = ui.widget_mut(name) {
            w.set_paint(Paint::OnAccent);
        }
        if let Some(detail) = detail {
            let note = ui.add_label(node, detail, TextStyle::Footnote, Style::default());
            if let Some(w) = ui.widget_mut(note) {
                w.set_paint(if selected { Paint::OnAccent } else { Paint::Secondary });
            }
        }
        let mark = ui.add(
            node,
            Widget::container().with_radius(CAPSULE).with_background(if selected {
                Background::Fill(Paint::Fixed(Color::rgb(0xFFFFFF)), 1.0)
            } else {
                Background::Fill(Paint::Label, 0.14)
            }),
            Style {
                size: dims(18.0, 18.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        if selected {
            ui.add(
                mark,
                Widget::icon(Icon::Check, 12.0).with_paint(Paint::Accent).with_icon_weight(IconWeight::Bold),
                Style::default(),
            );
        }
        self.register(ui, node, Target::Action(action));
        node
    }

    /// Width of a mock-up picture in a row of `per_row` tiles that share `inner` px.
    fn mock_width(&self, inner: f32, per_row: usize, gap: f32, max: f32) -> f32 {
        let tile = (inner - gap * (per_row as f32 - 1.0)) / per_row as f32;
        (tile - TILE_PAD * 2.0).clamp(80.0, max)
    }

    /// Whether the window is short: pictures shrink so that all fits.
    fn compact(&self) -> bool {
        self.window.y < 700.0
    }

    // ---- the steps ----

    fn welcome(&mut self, ui: &mut Ui, body: NodeId, config: &Config, inner: f32) {
        let side = if self.compact() { ICON_SIDE * 0.82 } else { ICON_SIDE };
        // The app icon is a rounded glass tile with its own soft edge: shown as is, never
        // clipped or shadowed again (the fallback tile below is).
        let holder = ui.add(
            body,
            if self.icon.is_some() {
                Widget::container()
            } else {
                Widget::container()
                    .with_shadow(Shadow { offset_y: 6.0, blur: 22.0, opacity: 0.25 })
                    .with_radius(side * 0.23)
            },
            Style { size: dims(side, side), flex_shrink: 0.0, ..Default::default() },
        );
        if let Some(icon) = &self.icon {
            let image =
                ui.add(holder, Widget::image(), Style { size: dims(side, side), ..Default::default() });
            ui.set_image(image, Some(icon.clone()));
        } else {
            ui.add(
                holder,
                Widget::container()
                    .with_background(Background::Fill(Paint::Accent, 1.0))
                    .with_radius(side * 0.23),
                Style { size: dims(side, side), justify_content: JustifyContent::CENTER, ..row(0.0) },
            );
        }
        self.title(
            ui,
            body,
            tr("Welcome to Axion"),
            tr("A fast, private browser made of Liquid Glass. A few quick choices and you're ready."),
            inner,
        );
        let caption =
            ui.add(body, Widget::container(), Style { margin: margin(0.0, 0.0, 6.0, 0.0), ..row(6.0) });
        ui.add(caption, Widget::icon(Icon::Globe, 16.0).with_paint(Paint::Secondary), Style::default());
        let label = ui.add_label(caption, tr("Language"), TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
        let languages = ui.add(body, Widget::container(), row(self.apart));
        for (i, &(name, language)) in LANGUAGES.iter().enumerate() {
            let selected = config.general.language == language;
            self.pill(
                ui,
                languages,
                ActionId(LANGUAGE_BASE + i as u32),
                name,
                if selected { PillKind::Selected } else { PillKind::Plain },
            );
        }
    }

    fn appearance(&mut self, ui: &mut Ui, body: NodeId, config: &Config, inner: f32) {
        self.title(
            ui,
            body,
            tr("Choose your look"),
            tr("Pick a theme and an accent colour. You can change both any time in Settings."),
            inner,
        );
        let gap = self.apart;
        let width = self.mock_width(inner, 3, gap, 190.0);
        let picture = Vec2::new(width, width * 0.66);
        let cards =
            ui.add(body, Widget::container(), Style { margin: margin(0.0, 0.0, 4.0, 0.0), ..row(gap) });
        let names = [tr("Light"), tr("Dark"), tr("Automatic")];
        for (i, &mode) in THEMES.iter().enumerate() {
            let tab_bar = config.layout.tab_bar;
            self.tile(
                ui,
                cards,
                ActionId(THEME_BASE + i as u32),
                config.appearance.theme == mode,
                names[i],
                None,
                move |ui, parent| match mode {
                    ThemeMode::Light => {
                        mock_window(
                            ui,
                            parent,
                            picture,
                            Mock { look: LIGHT, tab_bar, sidebar: SidebarKind::Docked },
                        );
                    }
                    ThemeMode::Dark => {
                        mock_window(
                            ui,
                            parent,
                            picture,
                            Mock { look: DARK, tab_bar, sidebar: SidebarKind::Docked },
                        );
                    }
                    ThemeMode::Auto => mock_split(ui, parent, picture, tab_bar),
                },
            );
        }
        let accent =
            ui.add(body, Widget::container(), Style { margin: margin(0.0, 0.0, 6.0, 0.0), ..row(14.0) });
        let label = ui.add_label(accent, tr("Accent colour"), TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(label) {
            w.set_paint(Paint::Secondary);
        }
        let swatches = ui.add(accent, Widget::container(), row(6.0));
        for (i, &color) in ACCENT_SWATCHES.iter().enumerate() {
            let selected = config.appearance.accent == color;
            let ring = ui.add(
                swatches,
                Widget::container()
                    .with_action(ActionId(ACCENT_BASE + i as u32))
                    .with_radius(CAPSULE)
                    .with_background(if selected {
                        Background::Fill(Paint::Label, 0.9)
                    } else {
                        Background::None
                    }),
                Style {
                    size: dims(32.0, 32.0),
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::CENTER,
                    ..row(0.0)
                },
            );
            block(ui, ring, color, 1.0, CAPSULE, Style { size: dims(26.0, 26.0), ..Default::default() });
            // The ring is a plain fill (not glass), so focus shows as a white inner dot.
            self.entries.push(Entry {
                node: ring,
                target: Target::Action(ActionId(ACCENT_BASE + i as u32)),
                base: Background::None,
            });
        }
    }

    fn layout(&mut self, ui: &mut Ui, body: NodeId, config: &Config, inner: f32) {
        self.title(
            ui,
            body,
            tr("Tabs and sidebar"),
            tr("Choose where your tabs live and how the sidebar sits next to the page."),
            inner,
        );
        let group_gap = self.apart + 12.0;
        let tile_gap = self.apart;
        let group_width = (inner - group_gap) / 2.0;
        let width = self.mock_width(group_width, 2, tile_gap, 150.0);
        let picture = Vec2::new(width, width * 0.66);
        let groups = ui.add(
            body,
            Widget::container(),
            Style {
                align_items: AlignItems::FLEX_START,
                margin: margin(0.0, 0.0, 6.0, 0.0),
                ..row(group_gap)
            },
        );
        let float = config.layout.float_sidebar;
        let tab_bar = config.layout.tab_bar;

        let left =
            ui.add(groups, Widget::container(), Style { align_items: AlignItems::CENTER, ..column(8.0) });
        let caption = ui.add_label(left, tr("Tab bar"), TextStyle::Caption, Style::default());
        if let Some(w) = ui.widget_mut(caption) {
            w.set_paint(Paint::Secondary);
        }
        let tiles = ui.add(left, Widget::container(), row(tile_gap));
        let names = [tr("Compact"), tr("Separate")];
        let details = [tr("Tabs in the address bar"), tr("Tabs in their own row")];
        for (i, &style) in TAB_BARS.iter().enumerate() {
            self.tile(
                ui,
                tiles,
                ActionId(TAB_BAR_BASE + i as u32),
                tab_bar == style,
                names[i],
                Some(details[i]),
                move |ui, parent| {
                    let sidebar = if float { SidebarKind::Floating } else { SidebarKind::Docked };
                    mock_window(ui, parent, picture, Mock { look: LIGHT, tab_bar: style, sidebar });
                },
            );
        }

        let right =
            ui.add(groups, Widget::container(), Style { align_items: AlignItems::CENTER, ..column(8.0) });
        let caption = ui.add_label(right, tr("Sidebar"), TextStyle::Caption, Style::default());
        if let Some(w) = ui.widget_mut(caption) {
            w.set_paint(Paint::Secondary);
        }
        let tiles = ui.add(right, Widget::container(), row(tile_gap));
        let names = [tr("Docked"), tr("Floating")];
        let details = [tr("Beside the page"), tr("Over the page")];
        for (i, floating) in [false, true].into_iter().enumerate() {
            self.tile(
                ui,
                tiles,
                ActionId(SIDEBAR_BASE + i as u32),
                float == floating,
                names[i],
                Some(details[i]),
                move |ui, parent| {
                    let sidebar = if floating { SidebarKind::Floating } else { SidebarKind::Docked };
                    mock_window(ui, parent, picture, Mock { look: LIGHT, tab_bar, sidebar });
                },
            );
        }
    }

    fn glass(&mut self, ui: &mut Ui, body: NodeId, config: &Config, inner: f32) {
        self.title(
            ui,
            body,
            tr("Glass or flat"),
            tr("Liquid Glass bends and blurs what is behind it. Flat is plainer and easier on weaker graphics hardware."),
            inner,
        );
        let gap = self.apart;
        let width = self.mock_width(inner, 2, gap, 250.0);
        let picture = Vec2::new(width, width * if self.compact() { 0.5 } else { 0.58 });
        let tiles =
            ui.add(body, Widget::container(), Style { margin: margin(0.0, 0.0, 2.0, 0.0), ..row(gap) });
        let names = [tr("Liquid Glass"), tr("Flat")];
        let flat_now = config.glass.is_flat();
        for (i, &style) in GLASS_STYLES.iter().enumerate() {
            self.tile(
                ui,
                tiles,
                ActionId(GLASS_BASE + i as u32),
                config.glass.style == style,
                names[i],
                None,
                move |ui, parent| glass_picture(ui, parent, picture, style == GlassStyle::Flat, !flat_now),
            );
        }
        let switches =
            ui.add(body, Widget::container(), Style { margin: margin(0.0, 0.0, 4.0, 0.0), ..row(28.0) });
        for (text, on, transparency) in [
            (tr("Reduce transparency"), config.glass.reduce_transparency, true),
            (tr("Reduce motion"), config.glass.reduce_motion, false),
        ] {
            let item = ui.add(switches, Widget::container(), row(10.0));
            let label = ui.add_label(item, text, TextStyle::Callout, Style::default());
            if let Some(w) = ui.widget_mut(label) {
                w.set_paint(Paint::Secondary);
            }
            let toggle = ui.add(item, Widget::toggle(on), Style::default());
            if transparency {
                self.reduce_transparency = Some(toggle);
            } else {
                self.reduce_motion = Some(toggle);
            }
        }
    }

    fn import_step(&mut self, ui: &mut Ui, body: NodeId, config: &Config, inner: f32) {
        let _ = config;
        self.title(
            ui,
            body,
            tr("Bring your bookmarks"),
            tr("Import bookmarks and history from the browsers on this computer. They are only read, never changed."),
            inner,
        );
        let width = (inner * 0.8).clamp(300.0, 520.0);
        let shown = if self.compact() { MAX_SOURCES - 1 } else { MAX_SOURCES };
        let list = ui.add(
            body,
            Widget::container(),
            Style {
                align_items: AlignItems::CENTER,
                margin: margin(0.0, 0.0, 6.0, 0.0),
                ..column(self.apart)
            },
        );
        let sources = self.sources.clone();
        for (i, source) in sources.iter().take(shown).enumerate() {
            self.source_row(ui, list, i, source, width);
        }
        if sources.len() > shown {
            let note = ui.add_label(
                body,
                &tr_fmt!(
                    "{count} more can be imported later in Settings › Data & Downloads.",
                    count = sources.len() - shown
                ),
                TextStyle::Footnote,
                Style::default(),
            );
            if let Some(w) = ui.widget_mut(note) {
                w.set_paint(Paint::Secondary);
            }
        } else {
            let note = ui.add_label(
                body,
                tr("You can import more later in Settings › Data & Downloads."),
                TextStyle::Footnote,
                Style::default(),
            );
            if let Some(w) = ui.widget_mut(note) {
                w.set_paint(Paint::Secondary);
            }
        }
    }

    /// A browser to import from: a wide piece of glass with its name, what it holds and a
    /// selection mark.
    fn source_row(&mut self, ui: &mut Ui, parent: NodeId, index: usize, source: &ImportChoice, width: f32) {
        let action = ActionId(IMPORT_BASE + index as u32);
        let selected = source.selected;
        let node = ui.add(
            parent,
            Widget::container().with_action(action).with_radius(TILE_RADIUS).with_background(if selected {
                Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
            } else {
                Background::Glass(GlassLevel::Thin)
            }),
            Style {
                size: Size { width: length(width), height: length(56.0) },
                flex_shrink: 0.0,
                padding: pad(16.0, 8.0),
                ..row(12.0)
            },
        );
        ui.add(
            node,
            Widget::icon(Icon::Globe, 22.0).with_paint(if selected {
                Paint::OnAccent
            } else {
                Paint::Secondary
            }),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        let text = ui.add(
            node,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        let name = ui.add_label(text, &source.label, TextStyle::Callout, Style::default());
        if selected && let Some(w) = ui.widget_mut(name) {
            w.set_paint(Paint::OnAccent);
        }
        let detail = ui.add_label(text, &source.detail, TextStyle::Footnote, Style::default());
        if let Some(w) = ui.widget_mut(detail) {
            w.set_paint(if selected { Paint::OnAccent } else { Paint::Secondary });
        }
        let mark = ui.add(
            node,
            Widget::container().with_radius(CAPSULE).with_background(if selected {
                Background::Fill(Paint::Fixed(Color::rgb(0xFFFFFF)), 1.0)
            } else {
                Background::Fill(Paint::Label, 0.14)
            }),
            Style {
                size: dims(20.0, 20.0),
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        if selected {
            ui.add(
                mark,
                Widget::icon(Icon::Check, 13.0).with_paint(Paint::Accent).with_icon_weight(IconWeight::Bold),
                Style::default(),
            );
        }
        self.register(ui, node, Target::Action(action));
    }

    fn account_step(&mut self, ui: &mut Ui, body: NodeId, config: &Config, inner: f32) {
        let _ = config;
        self.title(
            ui,
            body,
            tr("Your account"),
            tr("Tabs, history and passwords stay on this device, in an account of their own."),
            inner,
        );
        let form = ui.add(
            body,
            Widget::container(),
            Style { align_items: AlignItems::FLEX_START, margin: margin(0.0, 0.0, 4.0, 0.0), ..row(26.0) },
        );
        // The picture.
        let picture =
            ui.add(form, Widget::container(), Style { align_items: AlignItems::CENTER, ..column(10.0) });
        let initials = self.account.initials.clone();
        crate::settings::avatar(
            ui,
            picture,
            &initials,
            self.account.color,
            AVATAR_SIDE,
            self.account.picture.as_ref(),
        );
        if self.account.picture.is_some() {
            self.pill(ui, picture, REMOVE_PHOTO, tr("Remove Photo"), PillKind::Small);
        } else {
            self.pill(ui, picture, CHOOSE_PHOTO, tr("Choose Photo…"), PillKind::Small);
        }
        // The fields.
        let width = (inner * 0.5).clamp(230.0, 330.0);
        let fields = ui.add(form, Widget::container(), column(8.0));
        let name = self.field(ui, fields, tr("Name"), width, false);
        let note = ui.add_label(
            fields,
            tr("Optional password: locks private tabs and protects your saved passwords."),
            TextStyle::Footnote,
            Style {
                size: Size { width: length(width), height: auto() },
                min_size: Size { width: length(0.0), height: auto() },
                margin: margin(0.0, 0.0, 6.0, 0.0),
                ..Default::default()
            },
        );
        ui.set_label_lines(note, 3);
        if let Some(w) = ui.widget_mut(note) {
            w.set_paint(Paint::Secondary);
        }
        let (password, confirm) = if self.has_password {
            (name, name)
        } else {
            (
                self.field(ui, fields, tr("Password"), width, true),
                self.field(ui, fields, tr("Repeat password"), width, true),
            )
        };
        if self.has_password {
            let note = ui.add_label(
                fields,
                tr("This account already has a password."),
                TextStyle::Footnote,
                Style::default(),
            );
            if let Some(w) = ui.widget_mut(note) {
                w.set_paint(Paint::Secondary);
            }
        }
        ui.set_field_text(name, &self.draft.name);
        if !self.has_password {
            ui.set_field_text(password, &self.draft.password);
            ui.set_field_text(confirm, &self.draft.confirm);
        }
        self.fields = Some(Fields { name, password, confirm });
        if let Some(problem) = self.error.clone() {
            let error = paragraph(ui, body, &problem, TextStyle::Footnote, Paint::Fixed(ERROR), 2, inner);
            let _ = error;
        }
    }

    /// A labelled text field in a rounded box; registered for Tab.
    fn field(&mut self, ui: &mut Ui, parent: NodeId, placeholder: &str, width: f32, secure: bool) -> NodeId {
        let holder = ui.add(
            parent,
            Widget::container().with_background(Background::Fill(Paint::Label, 0.07)).with_radius(12.0),
            Style {
                size: Size { width: length(width), height: length(40.0) },
                flex_shrink: 0.0,
                padding: pad(12.0, 0.0),
                ..row(0.0)
            },
        );
        let field = ui.add_text_field(
            holder,
            placeholder,
            TextStyle::Body,
            Style {
                flex_grow: 1.0,
                size: Size { width: auto(), height: length(22.0) },
                ..Default::default()
            },
        );
        if secure {
            crate::eye::add(ui, holder, field);
        }
        self.register(ui, holder, Target::Field(field));
        field
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PillKind {
    Plain,
    Primary,
    /// A chosen option among pills.
    Selected,
    Small,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_walks_through_the_steps_and_ends_finished() {
        let mut flow = Flow::at(0, false);
        assert_eq!(flow.step(), Step::Welcome);
        for expected in [Step::Appearance, Step::Layout, Step::Glass, Step::Account] {
            assert!(flow.next());
            assert_eq!(flow.step(), expected);
            assert_eq!(flow.ending(), None);
        }
        assert!(flow.is_last());
        assert!(flow.next());
        assert_eq!(flow.ending(), Some(Ending::Finished));
        // Over: nothing moves any more.
        assert!(!flow.next());
        assert!(!flow.back());
        assert!(!flow.skip());
        assert_eq!(flow.ending(), Some(Ending::Finished));
    }

    #[test]
    fn back_goes_to_the_previous_step_and_stops_at_the_first() {
        let mut flow = Flow::at(0, false);
        assert!(!flow.back());
        flow.next();
        flow.next();
        assert_eq!(flow.index(), 2);
        assert!(flow.back());
        assert_eq!(flow.step(), Step::Appearance);
        assert!(flow.back());
        assert!(flow.is_first());
        assert!(!flow.back());
    }

    #[test]
    fn skip_ends_from_any_step() {
        for start in 0..Step::ALL.len() {
            let mut flow = Flow::at(start, false);
            assert!(flow.skip());
            assert_eq!(flow.ending(), Some(Ending::Skipped));
            assert!(!flow.next());
        }
    }

    #[test]
    fn a_start_beyond_the_last_step_is_clamped() {
        assert_eq!(Flow::at(99, false).step(), Step::Account);
        let mut flow = Flow::at(1, false);
        flow.goto(42);
        assert_eq!(flow.step(), Step::Account);
    }

    #[test]
    fn the_import_step_sits_before_the_account_and_only_when_offered() {
        assert_eq!(Flow::at(0, false).steps(), Step::ALL);
        assert!(!Flow::at(0, false).steps().contains(&Step::Import));
        let mut flow = Flow::at(0, true);
        let mut seen = vec![flow.step()];
        while !flow.is_last() {
            flow.next();
            seen.push(flow.step());
        }
        assert_eq!(
            seen,
            [Step::Welcome, Step::Appearance, Step::Layout, Step::Glass, Step::Import, Step::Account]
        );
        // "Start Browsing" still ends on the account step; a start beyond the end is clamped.
        assert_eq!(Flow::at(99, true).step(), Step::Account);
        assert_eq!(Flow::at(4, true).step(), Step::Import);
        assert_eq!(Flow::at(4, false).step(), Step::Account);
    }

    fn sheet(sources: Vec<ImportChoice>) -> (Ui, Onboarding) {
        let config = Config::default();
        let mut ui = Ui::new(axion_ui::Theme::new(&config, false), 1.0);
        let mut onboarding = Onboarding::build(&mut ui);
        onboarding.set_sources(sources);
        onboarding.open(&mut ui, &config, Vec2::new(1280.0, 800.0), 0);
        (ui, onboarding)
    }

    fn choice(label: &str, selected: bool) -> ImportChoice {
        ImportChoice { label: label.into(), detail: "Bookmarks and history".into(), selected }
    }

    #[test]
    fn the_step_is_hidden_without_other_browsers() {
        let (_ui, onboarding) = sheet(Vec::new());
        assert_eq!(onboarding.steps(), Step::ALL);
        let (_ui, onboarding) = sheet(vec![choice("Firefox", true)]);
        assert_eq!(onboarding.steps(), Step::WITH_IMPORT);
    }

    #[test]
    fn continuing_from_the_import_step_imports_the_checked_browsers_once() {
        let config = Config::default();
        let (mut ui, mut onboarding) =
            sheet(vec![choice("Firefox", true), choice("Brave", false), choice("Chrome", false)]);
        onboarding.goto(&mut ui, &config, 4);
        assert_eq!(onboarding.flow.step(), Step::Import);
        // Checking Brave too, then continuing, starts both; the account step follows.
        assert!(matches!(onboarding.press(&mut ui, &config, Click::Import(1)), Reaction::Moved));
        match onboarding.press(&mut ui, &config, Click::Continue) {
            Reaction::Import(picks) => assert_eq!(picks, [0, 1]),
            _ => panic!("expected an import"),
        }
        assert_eq!(onboarding.flow.step(), Step::Account);
        // Back and on again with the same picks does not import a second time.
        onboarding.press(&mut ui, &config, Click::Back);
        assert!(matches!(onboarding.press(&mut ui, &config, Click::Continue), Reaction::Moved));
        // Changing the picks does.
        onboarding.press(&mut ui, &config, Click::Back);
        onboarding.press(&mut ui, &config, Click::Import(1));
        match onboarding.press(&mut ui, &config, Click::Continue) {
            Reaction::Import(picks) => assert_eq!(picks, [0]),
            _ => panic!("expected an import"),
        }
    }

    #[test]
    fn nothing_checked_imports_nothing() {
        let config = Config::default();
        let (mut ui, mut onboarding) = sheet(vec![choice("Firefox", false)]);
        onboarding.goto(&mut ui, &config, 4);
        assert!(matches!(onboarding.press(&mut ui, &config, Click::Continue), Reaction::Moved));
    }

    #[test]
    fn import_clicks_have_their_own_ids() {
        assert_eq!(click_of(ActionId(IMPORT_BASE)), Some(Click::Import(0)));
        assert_eq!(
            click_of(ActionId(IMPORT_BASE + MAX_SOURCES as u32 - 1)),
            Some(Click::Import(MAX_SOURCES - 1))
        );
        assert_eq!(click_of(ActionId(IMPORT_BASE + MAX_SOURCES as u32)), None);
    }

    fn fresh() -> FirstRun {
        FirstRun::default()
    }

    #[test]
    fn a_fresh_install_shows_the_introduction() {
        assert_eq!(decide(&fresh()), Launch::Show);
    }

    #[test]
    fn done_or_known_users_do_not_see_it() {
        assert_eq!(decide(&FirstRun { done: true, ..fresh() }), Launch::Skip);
        for known in [
            FirstRun { accounts_file: true, ..fresh() },
            FirstRun { config_file: true, ..fresh() },
            FirstRun { browsing_data: true, ..fresh() },
        ] {
            assert_eq!(decide(&known), Launch::MarkDone);
        }
    }

    #[test]
    fn an_unfinished_introduction_comes_back() {
        // The first start created accounts.toml and the config; the marker says why.
        let again = FirstRun { started: true, accounts_file: true, config_file: true, ..fresh() };
        assert_eq!(decide(&again), Launch::Show);
    }

    #[test]
    fn forcing_wins_and_captures_stay_quiet() {
        assert_eq!(
            decide(&FirstRun { forced: true, done: true, accounts_file: true, ..fresh() }),
            Launch::Show
        );
        assert_eq!(decide(&FirstRun { capture: true, ..fresh() }), Launch::Skip);
        assert_eq!(decide(&FirstRun { forced: true, capture: true, ..fresh() }), Launch::Show);
    }

    #[test]
    fn picks_write_the_configuration() {
        let mut config = Config::default();
        Pick::Theme(ThemeMode::Dark).apply(&mut config);
        Pick::Accent(2).apply(&mut config);
        Pick::TabBar(TabBarStyle::Separate).apply(&mut config);
        Pick::FloatSidebar(true).apply(&mut config);
        Pick::Glass(GlassStyle::Flat).apply(&mut config);
        Pick::Language(UiLanguage::German).apply(&mut config);
        Pick::ReduceMotion(true).apply(&mut config);
        assert_eq!(config.appearance.theme, ThemeMode::Dark);
        assert_eq!(config.appearance.accent, ACCENT_SWATCHES[2]);
        assert_eq!(config.layout.tab_bar, TabBarStyle::Separate);
        assert!(config.layout.float_sidebar);
        assert!(config.glass.is_flat());
        assert_eq!(config.general.language, UiLanguage::German);
        assert!(config.glass.reduce_motion);
    }

    #[test]
    fn every_action_id_maps_to_one_click() {
        assert_eq!(click_of(CONTINUE), Some(Click::Continue));
        assert_eq!(click_of(ActionId(THEME_BASE + 1)), Some(Click::Pick(Pick::Theme(ThemeMode::Dark))));
        assert_eq!(click_of(ActionId(SIDEBAR_BASE + 1)), Some(Click::Pick(Pick::FloatSidebar(true))));
        assert_eq!(click_of(ActionId(ACCENT_BASE + 5)), Some(Click::Pick(Pick::Accent(5))));
        assert_eq!(click_of(ActionId(ACCENT_BASE + 6)), None);
        assert_eq!(click_of(ActionId(BASE - 1)), None);
    }
}
