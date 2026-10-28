//! The Reader popover ("Aa"): drips out of the Reader button in the address field. Text size,
//! font, theme and column width of Reader view, and a way back to the page.

use axion_compositor::{Icon, Rect as Frame};
use axion_config::{Config, ReaderFont, ReaderTheme};
use axion_i18n::{tr, tr_mark};
use axion_reader::Look;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::popover::Popover;

const ACTION_BASE: u32 = 6800;
const WIDTH: f32 = 340.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 16.0;
const GAP: f32 = 12.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const HEADER: f32 = 36.0;
const ROW: f32 = 36.0;
const NOTE: f32 = 18.0;
const FONTS: [(&str, ReaderFont); 3] = [
    (tr_mark("Serif"), ReaderFont::Serif),
    (tr_mark("Sans"), ReaderFont::Sans),
    (tr_mark("Mono"), ReaderFont::Mono),
];
const THEMES: [(&str, ReaderTheme); 4] = [
    (tr_mark("Auto"), ReaderTheme::Auto),
    (tr_mark("Light"), ReaderTheme::Light),
    (tr_mark("Sepia"), ReaderTheme::Sepia),
    (tr_mark("Dark"), ReaderTheme::Dark),
];

/// What a click in the popover asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    SmallerText,
    LargerText,
    NarrowerColumn,
    WiderColumn,
    Font(ReaderFont),
    Theme(ReaderTheme),
    Exit,
}

pub struct ReaderPopover {
    popover: Popover,
    plate: NodeId,
}

fn id(n: u32) -> ActionId {
    ActionId(ACTION_BASE + n)
}

impl Action {
    fn id(self) -> ActionId {
        match self {
            Action::SmallerText => id(0),
            Action::LargerText => id(1),
            Action::NarrowerColumn => id(2),
            Action::WiderColumn => id(3),
            Action::Exit => id(4),
            Action::Font(f) => id(10 + FONTS.iter().position(|(_, x)| *x == f).unwrap_or(0) as u32),
            Action::Theme(t) => id(20 + THEMES.iter().position(|(_, x)| *x == t).unwrap_or(0) as u32),
        }
    }

    fn from_id(action: ActionId) -> Option<Self> {
        let n = action.0.checked_sub(ACTION_BASE)?;
        Some(match n {
            0 => Action::SmallerText,
            1 => Action::LargerText,
            2 => Action::NarrowerColumn,
            3 => Action::WiderColumn,
            4 => Action::Exit,
            10..=12 => Action::Font(FONTS[(n - 10) as usize].1),
            20..=23 => Action::Theme(THEMES[(n - 20) as usize].1),
            _ => return None,
        })
    }
}

impl ReaderPopover {
    /// `parent` is the toolbar, so the popover's glass merges with it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            RADIUS,
            Style {
                size: Size { width: length(WIDTH), height: auto() },
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(GAP) },
                padding: uniform(PADDING + INSET),
                ..Default::default()
            },
        );
        let plate = popover.root();
        Self { popover, plate }
    }

    pub fn is_open(&self) -> bool {
        self.popover.is_open()
    }

    /// Opens below `anchor`, or refreshes the content if already open.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        look: &Look,
        per_site: bool,
    ) {
        self.fill(ui, config, look, per_site);
        let height = 2.0 * (PADDING + INSET) + HEADER + 5.0 * ROW + NOTE + 6.0 * GAP;
        let left = (anchor.origin.x - PADDING - 8.0).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        self.popover.open(ui, config, anchor, target);
    }

    fn fill(&mut self, ui: &mut Ui, config: &Config, look: &Look, per_site: bool) {
        ui.clear_children(self.plate);
        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 10.0));
        ui.add(header, Widget::icon(Icon::TextSize, 24.0).with_paint(Paint::Accent), Style::default());
        let title = ui.add_label(header, tr("Reader View"), TextStyle::BodyEmphasis, Style::default());
        let mut style = ui.style(title);
        style.flex_grow = 1.0;
        ui.set_style(title, style);

        let r = &config.reader;
        let row_node = ui.add(self.plate, Widget::container(), fixed_row(ROW, 8.0));
        label(ui, row_node, tr("Text size"));
        step_button(ui, row_node, Action::SmallerText, Icon::Minus, look.size > r.min_font_size);
        let size = ui.add_label(row_node, &format!("{:.0}", look.size), TextStyle::Callout, Style::default());
        let _ = size;
        step_button(ui, row_node, Action::LargerText, Icon::Plus, look.size < r.max_font_size);

        let row_node = ui.add(self.plate, Widget::container(), fixed_row(ROW, 8.0));
        label(ui, row_node, tr("Column width"));
        step_button(ui, row_node, Action::NarrowerColumn, Icon::Minus, look.width > r.min_width);
        ui.add_label(row_node, &format!("{:.0}", look.width), TextStyle::Callout, Style::default());
        step_button(ui, row_node, Action::WiderColumn, Icon::Plus, look.width < r.max_width);

        let row_node = ui.add(self.plate, Widget::container(), fixed_row(ROW, 0.0));
        let options: Vec<(&str, ActionId)> =
            FONTS.iter().map(|(name, font)| (tr(name), Action::Font(*font).id())).collect();
        let current = FONTS.iter().position(|(_, f)| *f == look.font);
        crate::settings::segmented(ui, row_node, &options, current);

        let row_node = ui.add(self.plate, Widget::container(), fixed_row(ROW, 0.0));
        let options: Vec<(&str, ActionId)> =
            THEMES.iter().map(|(name, theme)| (tr(name), Action::Theme(*theme).id())).collect();
        let current = THEMES.iter().position(|(_, t)| *t == look.theme);
        crate::settings::segmented(ui, row_node, &options, current);

        let note = ui.add(self.plate, Widget::container(), fixed_row(NOTE, 0.0));
        let text = if per_site { tr("Remembered for this site.") } else { tr("Used on every site.") };
        let n = ui.add_label(note, text, TextStyle::Footnote, shrinking());
        if let Some(w) = ui.widget_mut(n) {
            w.set_paint(Paint::Secondary);
        }

        let footer = ui.add(self.plate, Widget::container(), fixed_row(ROW, 8.0));
        let exit = ui.add(
            footer,
            Widget::container()
                .with_action(Action::Exit.id())
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Label, 0.08)),
            Style {
                size: Size { width: auto(), height: length(ROW) },
                padding: Rect {
                    left: length(14.0),
                    right: length(16.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                flex_shrink: 0.0,
                ..row(6.0)
            },
        );
        ui.add(exit, Widget::icon(Icon::Close, 16.0), Style::default());
        ui.add_label(exit, tr("Exit Reader View"), TextStyle::Callout, Style::default());
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.popover.close(ui, config);
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }

    pub fn action(&self, action: ActionId) -> Option<Action> {
        Action::from_id(action)
    }
}

fn label(ui: &mut Ui, parent: NodeId, text: &str) {
    let node = ui.add_label(parent, text, TextStyle::Callout, shrinking());
    let mut style = ui.style(node);
    style.flex_grow = 1.0;
    ui.set_style(node, style);
}

/// A round − / + button; dimmed (no action) at its limit.
fn step_button(ui: &mut Ui, parent: NodeId, action: Action, icon: Icon, enabled: bool) {
    let mut widget =
        Widget::container().with_radius(CAPSULE).with_background(Background::Fill(Paint::Label, 0.08));
    if enabled {
        widget = widget.with_action(action.id());
    }
    let node = ui.add(
        parent,
        widget,
        Style {
            size: Size { width: length(ROW), height: length(ROW - 4.0) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            ..row(0.0)
        },
    );
    let paint = if enabled { Paint::Label } else { Paint::Secondary };
    ui.add(node, Widget::icon(icon, 16.0).with_paint(paint), Style::default());
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

fn row(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn fixed_row(height: f32, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row(gap) }
}

fn shrinking() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        let all = [
            Action::SmallerText,
            Action::LargerText,
            Action::NarrowerColumn,
            Action::WiderColumn,
            Action::Exit,
            Action::Font(ReaderFont::Mono),
            Action::Theme(ReaderTheme::Sepia),
            Action::Theme(ReaderTheme::Auto),
        ];
        for a in all {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id(ActionId(1)), None);
        assert_eq!(Action::from_id(ActionId(ACTION_BASE + 99)), None);
    }
}
