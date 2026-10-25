//! The Shields popover: drips out of the shield in the address pill and shows what was
//! blocked on the page, with the site's level (Off / Standard / Aggressive).

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_shields::Level;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::drip::{Drip, Phase};
use crate::menu::place;

const ACTION_BASE: u32 = 6100;
const SETTINGS: ActionId = ActionId(ACTION_BASE + 10);
const WIDTH: f32 = 330.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 16.0;
const GAP: f32 = 12.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const PLATE_OPACITY: f32 = 0.55;
/// Fixed row heights, so the drip knows its target size before layout.
const HEADER: f32 = 40.0;
const COUNTER: f32 = 44.0;
const SEGMENTS: f32 = 36.0;
const NOTE: f32 = 18.0;
const FOOTER: f32 = 32.0;
const LEVELS: [(&str, Level); 3] =
    [("Off", Level::Off), ("Standard", Level::Standard), ("Aggressive", Level::Aggressive)];

/// What the popover shows.
pub struct ShieldsInfo {
    pub host: String,
    pub level: Level,
    pub blocked_here: u32,
    pub blocked_total: u64,
    /// Filter lists are still downloading.
    pub lists_loading: bool,
}

pub enum PopoverAction {
    SetLevel(Level),
    OpenSettings,
}

pub struct ShieldsPopover {
    parent: NodeId,
    root: NodeId,
    plate: NodeId,
    /// Labels updated in place while the page keeps loading (see `update_counts`).
    count: Option<NodeId>,
    total: Option<NodeId>,
    shown_level: Option<Level>,
    drip: Drip,
}

impl ShieldsPopover {
    /// `parent` is the toolbar, so the popover's glass merges with it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let root = ui.add(
            parent,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(RADIUS),
            Style {
                position: Position::Absolute,
                size: Size { width: length(WIDTH), height: auto() },
                padding: uniform(PADDING),
                ..Default::default()
            },
        );
        let plate = ui.add(
            root,
            Widget::container()
                .with_background(Background::Fill(Paint::Surface, PLATE_OPACITY))
                .with_radius(RADIUS - PADDING),
            Style {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                gap: Size { width: length(0.0), height: length(GAP) },
                padding: uniform(INSET),
                ..Default::default()
            },
        );
        ui.set_visible(root, false);
        Self { parent, root, plate, count: None, total: None, shown_level: None, drip: Drip::new() }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.drip.phase(), Phase::Opening | Phase::Open)
    }

    /// Opens below `anchor` (the shield button), or refreshes the content if already open.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        info: &ShieldsInfo,
    ) {
        self.fill(ui, info);
        let height = 2.0 * (PADDING + INSET) + HEADER + COUNTER + SEGMENTS + NOTE + FOOTER + 4.0 * GAP;
        let left = (anchor.origin.x - PADDING - 8.0).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        place(ui, self.root, self.parent, target);
        ui.set_visible(self.root, true);
        if self.is_open() {
            self.drip.retarget(target);
        } else {
            self.drip.open(config, anchor, target);
        }
        self.animate(ui, 0.0);
    }

    fn fill(&mut self, ui: &mut Ui, info: &ShieldsInfo) {
        ui.clear_children(self.plate);
        let active = info.level != Level::Off;

        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 12.0));
        let glyph = Widget::icon(Icon::Shield, 26.0).with_paint(if active {
            Paint::Shield
        } else {
            Paint::Secondary
        });
        ui.add(header, glyph, Style::default());
        let titles = ui.add(
            header,
            Widget::container(),
            Style { flex_direction: FlexDirection::Column, flex_grow: 1.0, ..Default::default() },
        );
        ui.add_label(titles, &info.host, TextStyle::BodyEmphasis, Style::default());
        let status = if active { "Shields are up" } else { "Shields are down for this site" };
        secondary(ui, titles, status, TextStyle::Footnote);

        let counter = ui.add(self.plate, Widget::container(), fixed_row(COUNTER, 12.0));
        let count = if active { info.blocked_here.to_string() } else { "–".into() };
        self.count = Some(ui.add_label(counter, &count, TextStyle::LargeTitle, Style::default()));
        let what =
            if info.lists_loading && active { "blocked · lists loading" } else { "blocked on this page" };
        secondary(ui, counter, what, TextStyle::Footnote);

        let segments = ui.add(self.plate, Widget::container(), fixed_row(SEGMENTS, 0.0));
        let options: Vec<(&str, ActionId)> = LEVELS
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (*name, ActionId(ACTION_BASE + i as u32)))
            .collect();
        let current = LEVELS.iter().position(|(_, l)| *l == info.level);
        crate::settings::segmented(ui, segments, &options, current);

        let note = match info.level {
            Level::Off => "Nothing is blocked on this site.",
            Level::Standard => "Blocks third-party ads, trackers and ad popups.",
            Level::Aggressive => "Also first-party ads and redirects without a click.",
        };
        let note_row = ui.add(self.plate, Widget::container(), fixed_row(NOTE, 0.0));
        secondary(ui, note_row, note, TextStyle::Footnote);

        let footer = ui.add(self.plate, Widget::container(), fixed_row(FOOTER, 8.0));
        let total = format!("{} blocked in total", group_thousands(info.blocked_total));
        let label = secondary(ui, footer, &total, TextStyle::Footnote);
        self.total = Some(label);
        self.shown_level = Some(info.level);
        let mut style = ui.style(label);
        style.flex_grow = 1.0;
        ui.set_style(label, style);
        let button = ui.add(
            footer,
            Widget::container()
                .with_action(SETTINGS)
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Label, 0.07)),
            Style {
                size: Size { width: auto(), height: length(FOOTER) },
                padding: Rect {
                    left: length(12.0),
                    right: length(8.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..row(4.0)
            },
        );
        ui.add_label(button, "Settings", TextStyle::Callout, Style::default());
        ui.add(button, Widget::icon(Icon::ChevronRight, 14.0).with_paint(Paint::Secondary), Style::default());
    }

    /// Refreshes the counters (cheap; call for every blocked request). Rebuilds the content
    /// only when the level changed.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        info: &ShieldsInfo,
    ) {
        if !self.is_open() {
            return;
        }
        if self.shown_level != Some(info.level) {
            self.open(ui, config, anchor, window_width, info);
            return;
        }
        if let Some(count) = self.count
            && info.level != Level::Off
        {
            ui.set_text(count, &info.blocked_here.to_string());
        }
        if let Some(total) = self.total {
            ui.set_text(total, &format!("{} blocked in total", group_thousands(info.blocked_total)));
        }
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.drip.close(config);
        self.animate(ui, 0.0);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        if self.drip.step(dt) == Phase::Closed {
            ui.set_visible(self.root, false);
            return;
        }
        ui.set_visual_frame(self.root, Some(self.drip.frame()));
        ui.set_opacity(self.plate, self.drip.content_opacity());
    }

    pub fn action(&self, id: ActionId) -> Option<PopoverAction> {
        if id == SETTINGS {
            return Some(PopoverAction::OpenSettings);
        }
        let i = id.0.checked_sub(ACTION_BASE)? as usize;
        LEVELS.get(i).map(|(_, l)| PopoverAction::SetLevel(*l))
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.is_open() && ui.frame(self.root).contains(p)
    }
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

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(
        parent,
        text,
        style,
        Style {
            flex_shrink: 1.0,
            min_size: Size { width: length(0.0), height: auto() },
            ..Default::default()
        },
    );
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

/// 1234567 → "1,234,567".
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }
}
