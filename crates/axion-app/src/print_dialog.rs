//! The print dialog: a glass sheet with the options of "Save as PDF" (paper, orientation,
//! margins, scale, backgrounds, header and footer). The choices are kept in `[print]`.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Color, Config, PaperSize, PrintMargins};
use axion_engine::PdfOptions;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use axion_i18n::tr;

const BACKDROP: ActionId = ActionId(9300);
pub(crate) const SAVE: ActionId = ActionId(9301);
const CANCEL: ActionId = ActionId(9302);
const PAPER_BASE: u32 = 9310;
const ORIENT_BASE: u32 = 9320;
pub(crate) const ORIENT_LANDSCAPE: ActionId = ActionId(9321);
const MARGIN_BASE: u32 = 9330;
const BACKGROUNDS: ActionId = ActionId(9340);
const HEADER_FOOTER: ActionId = ActionId(9341);
const SCALE_DOWN: ActionId = ActionId(9342);
const SCALE_UP: ActionId = ActionId(9343);

const WIDTH: f32 = 460.0;
const RADIUS: f32 = 24.0;
const START_SCALE: f32 = 0.88;
const BACKDROP_DIM: f32 = 0.28;
const SCALE_STEP: u32 = 10;
const SCALE_MIN: u32 = 25;
const SCALE_MAX: u32 = 200;

const PAPERS: [PaperSize; 5] =
    [PaperSize::A4, PaperSize::A3, PaperSize::A5, PaperSize::Letter, PaperSize::Legal];
const MARGINS: [PrintMargins; 4] =
    [PrintMargins::None, PrintMargins::Narrow, PrintMargins::Default, PrintMargins::Wide];

/// A click in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintAction {
    Save,
    Cancel,
    Backdrop,
    /// An option changed (the dialog already shows it).
    Changed,
}

/// Paper size in inches (portrait).
pub fn paper_inches(paper: PaperSize) -> (f64, f64) {
    match paper {
        PaperSize::A4 => (8.27, 11.69),
        PaperSize::A3 => (11.69, 16.54),
        PaperSize::A5 => (5.83, 8.27),
        PaperSize::Letter => (8.5, 11.0),
        PaperSize::Legal => (8.5, 14.0),
    }
}

fn paper_name(paper: PaperSize) -> &'static str {
    match paper {
        PaperSize::A4 => "A4",
        PaperSize::A3 => "A3",
        PaperSize::A5 => "A5",
        PaperSize::Letter => "Letter",
        PaperSize::Legal => "Legal",
    }
}

fn margin_name(margins: PrintMargins) -> &'static str {
    match margins {
        PrintMargins::None => tr("None"),
        PrintMargins::Narrow => tr("Narrow"),
        PrintMargins::Default => tr("Default"),
        PrintMargins::Wide => tr("Wide"),
    }
}

/// The engine's options for the dialog's choices.
pub fn pdf_options(config: &Config) -> PdfOptions {
    let p = &config.print;
    let (w, h) = paper_inches(p.paper);
    let (paper_width, paper_height) = if p.landscape { (h, w) } else { (w, h) };
    let margin = f64::from(match p.margins {
        PrintMargins::None => 0.0,
        PrintMargins::Narrow => p.margin_narrow_inches,
        PrintMargins::Default => p.margin_default_inches,
        PrintMargins::Wide => p.margin_wide_inches,
    });
    PdfOptions {
        landscape: false,
        print_background: p.backgrounds,
        scale: f64::from(p.scale_percent.clamp(SCALE_MIN, SCALE_MAX)) / 100.0,
        paper_width,
        paper_height,
        margin_top: margin,
        margin_right: margin,
        margin_bottom: margin,
        margin_left: margin,
        header_footer: p.header_footer,
        page_ranges: String::new(),
    }
}

pub struct PrintDialog {
    root: NodeId,
    card: NodeId,
    open: bool,
    appear: Animated<f32>,
    dirty: bool,
    /// The page's title, shown under the heading.
    page: String,
}

pub(crate) fn gap(row: f32, column: f32) -> Size<LengthPercentage> {
    Size { width: length(row), height: length(column) }
}

pub(crate) fn chip(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, on: bool) {
    let background = if on {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            min_size: Size { width: length(0.0), height: length(32.0) },
            padding: Rect { left: length(14.0), right: length(14.0), top: length(0.0), bottom: length(0.0) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(node, text, TextStyle::Footnote, Style::default());
    ui.set_label_lines(label, 1);
    if on && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnAccent);
    }
}

fn row(ui: &mut Ui, parent: NodeId, title: &str) -> NodeId {
    let outer = ui.add(
        parent,
        Widget::container(),
        Style { flex_direction: FlexDirection::Column, gap: gap(0.0, 6.0), ..Default::default() },
    );
    let label = ui.add_label(outer, title, TextStyle::Footnote, Style::default());
    ui.set_label_lines(label, 1);
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    ui.add(
        outer,
        Widget::container(),
        Style {
            flex_wrap: FlexWrap::Wrap,
            gap: gap(8.0, 8.0),
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    )
}

pub(crate) fn button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) {
    let background = if primary {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
    let node = ui.add(
        parent,
        Widget::container().with_action(action).with_radius(CAPSULE).with_background(background),
        Style {
            flex_grow: 1.0,
            flex_basis: length(0.0),
            min_size: Size { width: length(0.0), height: length(40.0) },
            justify_content: JustifyContent::CENTER,
            align_items: AlignItems::CENTER,
            ..Default::default()
        },
    );
    let label = ui.add_label(node, text, TextStyle::BodyEmphasis, Style::default());
    ui.set_label_lines(label, 1);
    if primary && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnAccent);
    }
}

impl PrintDialog {
    pub fn build(ui: &mut Ui) -> Self {
        let root = ui.add(
            ui.root(),
            Widget::container()
                .with_background(Background::Fill(Paint::Fixed(Color::rgb(0x000000)), BACKDROP_DIM))
                .without_feedback()
                .with_action(BACKDROP),
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
            Widget::container().with_background(Background::Glass(GlassLevel::Thick)).with_radius(RADIUS),
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                max_size: Size { width: percent(0.94), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: gap(0.0, 14.0),
                padding: Rect {
                    left: length(22.0),
                    right: length(22.0),
                    top: length(22.0),
                    bottom: length(20.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        Self {
            root,
            card,
            open: false,
            appear: Animated::new(0.0, Spring::SNAPPY),
            dirty: false,
            page: String::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        ui.frame(self.card).contains(p)
    }

    /// Opens the dialog for the page called `page`.
    pub fn show(&mut self, ui: &mut Ui, config: &Config, page: &str) {
        self.page = page.to_owned();
        self.dirty = true;
        if self.open {
            return;
        }
        self.open = true;
        self.appear.spring = if config.glass.reduce_motion {
            Spring::new(0.15, 1.0)
        } else {
            Spring::new(
                config.motion.snappy.response * config.motion.speed,
                config.glass.damping(config.motion.snappy.damping),
            )
        };
        self.appear.snap(0.0);
        self.appear.target = 1.0;
        ui.set_visible(self.root, true);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.appear.target = 0.0;
    }

    /// What a click on `id` means; option clicks are applied to `config` right here.
    pub fn action(&mut self, id: ActionId, config: &mut Config) -> Option<PrintAction> {
        let p = &mut config.print;
        let n = id.0;
        match id {
            SAVE => return Some(PrintAction::Save),
            CANCEL => return Some(PrintAction::Cancel),
            BACKDROP => return Some(PrintAction::Backdrop),
            BACKGROUNDS => p.backgrounds = !p.backgrounds,
            HEADER_FOOTER => p.header_footer = !p.header_footer,
            SCALE_DOWN => p.scale_percent = p.scale_percent.saturating_sub(SCALE_STEP).max(SCALE_MIN),
            SCALE_UP => p.scale_percent = (p.scale_percent + SCALE_STEP).min(SCALE_MAX),
            _ if (PAPER_BASE..PAPER_BASE + 5).contains(&n) => p.paper = PAPERS[(n - PAPER_BASE) as usize],
            _ if (ORIENT_BASE..ORIENT_BASE + 2).contains(&n) => p.landscape = n == ORIENT_BASE + 1,
            _ if (MARGIN_BASE..MARGIN_BASE + 4).contains(&n) => {
                p.margins = MARGINS[(n - MARGIN_BASE) as usize];
            }
            _ => return None,
        }
        self.dirty = true;
        Some(PrintAction::Changed)
    }

    fn rebuild(&mut self, ui: &mut Ui, config: &Config) {
        self.dirty = false;
        let p = &config.print;
        let card = self.card;
        ui.clear_children(card);
        let head = ui.add(
            card,
            Widget::container(),
            Style { align_items: AlignItems::CENTER, gap: gap(12.0, 0.0), ..Default::default() },
        );
        ui.add(head, Widget::icon(Icon::Download, 26.0).with_paint(Paint::Accent), Style::default());
        let titles = ui.add(
            head,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        let title = ui.add_label(titles, tr("Save as PDF"), TextStyle::BodyEmphasis, Style::default());
        ui.set_label_lines(title, 1);
        let page = ui.add_label(titles, &self.page, TextStyle::Footnote, Style::default());
        ui.set_label_lines(page, 1);
        if let Some(w) = ui.widget_mut(page) {
            w.set_paint(Paint::Secondary);
        }

        let paper = row(ui, card, tr("Paper"));
        for (i, size) in PAPERS.iter().enumerate() {
            chip(ui, paper, ActionId(PAPER_BASE + i as u32), paper_name(*size), p.paper == *size);
        }
        let orientation = row(ui, card, tr("Orientation"));
        chip(ui, orientation, ActionId(ORIENT_BASE), tr("Portrait"), !p.landscape);
        chip(ui, orientation, ActionId(ORIENT_BASE + 1), tr("Landscape"), p.landscape);
        let margins = row(ui, card, tr("Margins"));
        for (i, m) in MARGINS.iter().enumerate() {
            chip(ui, margins, ActionId(MARGIN_BASE + i as u32), margin_name(*m), p.margins == *m);
        }
        let scale = row(ui, card, tr("Scale"));
        chip(ui, scale, SCALE_DOWN, "\u{2212}", false);
        let value = ui.add_label(scale, &format!("{}%", p.scale_percent), TextStyle::Body, Style::default());
        ui.set_label_lines(value, 1);
        chip(ui, scale, SCALE_UP, "+", false);
        let options = row(ui, card, tr("Options"));
        chip(ui, options, BACKGROUNDS, tr("Backgrounds"), p.backgrounds);
        chip(ui, options, HEADER_FOOTER, tr("Header and footer"), p.header_footer);

        let buttons = ui.add(
            card,
            Widget::container(),
            Style {
                gap: gap(10.0, 0.0),
                margin: Rect { left: length(0.0), right: length(0.0), top: length(6.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        button(ui, buttons, CANCEL, tr("Cancel"), false);
        button(ui, buttons, SAVE, tr("Save as PDF…"), true);
    }

    /// Rebuilds the content after a change and advances the appear animation. Call every frame.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32) {
        if !ui.is_visible(self.root) {
            return;
        }
        if self.dirty {
            self.rebuild(ui, config);
        }
        self.appear.step(dt);
        let k = self.appear.value.clamp(0.0, 1.2);
        if !self.open && k < 0.01 {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_opacity(self.root, k.min(1.0));
        let frame = ui.frame(self.card);
        if frame.size.x > 0.0 {
            let scale = START_SCALE + (1.0 - START_SCALE) * k;
            let size = frame.size * scale;
            let origin = frame.origin + (frame.size - size) / 2.0;
            ui.set_visual_frame(self.card, Some(Frame { origin, size: size.max(Vec2::ONE) }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landscape_swaps_the_paper_and_margins_follow_the_preset() {
        let mut config = Config::default();
        config.print.paper = PaperSize::Letter;
        config.print.landscape = true;
        config.print.margins = PrintMargins::Wide;
        let o = pdf_options(&config);
        assert_eq!((o.paper_width, o.paper_height), (11.0, 8.5));
        assert_eq!(o.margin_left, 1.0);
        config.print.margins = PrintMargins::None;
        assert_eq!(pdf_options(&config).margin_top, 0.0);
    }

    #[test]
    fn scale_stays_in_bounds() {
        let mut config = Config::default();
        config.print.scale_percent = 9999;
        assert_eq!(pdf_options(&config).scale, 2.0);
    }
}
