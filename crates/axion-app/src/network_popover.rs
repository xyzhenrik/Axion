//! The network popover: drips out of the chip in the address field and shows the route of the
//! current site. Its primary action is "Use <connection> for this session" (every tab through
//! one connection until it is switched off); below it a choice per site (the default, direct,
//! or a connection).

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_i18n::{tr, tr_fmt};
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, GlassLevel, Paint, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::network::ChipState;
use crate::popover::Popover;

/// This popover owns ids `ACTION_BASE..ACTION_BASE + 20` (the circuit popover starts at 6970).
const ACTION_BASE: u32 = 6950;
const SETTINGS: ActionId = ActionId(ACTION_BASE + 1);
/// Switches the session route off.
const SESSION_OFF: ActionId = ActionId(ACTION_BASE + 2);
/// `SESSION_BASE + i`: use the i-th offered connection for the session.
const SESSION_BASE: u32 = ACTION_BASE + 3;
/// Most connections offered for a session route.
pub const MAX_SESSION: usize = 7;
const CHOICE_BASE: u32 = ACTION_BASE + 10;
/// Most connections the popover lists.
pub const MAX_CHOICES: usize = 8;
const WIDTH: f32 = 340.0;
const PADDING: f32 = 6.0;
const INSET: f32 = 16.0;
const GAP: f32 = 10.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
const RADIUS: f32 = 24.0;
const HEADER: f32 = 40.0;
const LABEL: f32 = 18.0;
const CHOICE: f32 = 36.0;
const NOTE: f32 = 36.0;
const SESSION_NOTE: f32 = 54.0;
const FOOTER: f32 = 32.0;
const SESSION_BUTTON: f32 = 40.0;

/// What the popover shows.
pub struct NetworkInfo {
    /// The site the choice is for (a registrable domain).
    pub site: String,
    pub state: ChipState,
    /// "Connected via Work VPN".
    pub title: String,
    /// The status line under it.
    pub detail: String,
    /// The default first (no rule for the site), then direct, then the connections: label and
    /// whether it is the route now.
    pub choices: Vec<(String, bool)>,
    /// A rule for this site exists (the note says so).
    pub has_rule: bool,
    /// The connection of the active session route, if there is one.
    pub session_active: Option<String>,
    /// Connections that can be used for the session: the first is the primary action, the
    /// rest are listed under it (while a session route is on, these switch to another one).
    pub session_offers: Vec<String>,
}

pub enum PopoverAction {
    /// The choice at this index (0 = the default, 1 = direct, then the connections).
    Choose(usize),
    /// Use the i-th offered connection for the whole session.
    SessionOn(usize),
    /// Switch the session route off; the normal routes come back.
    SessionOff,
    OpenSettings,
}

pub struct NetworkPopover {
    popover: Popover,
    plate: NodeId,
}

impl NetworkPopover {
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

    pub fn open(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        info: &NetworkInfo,
    ) {
        let height = self.fill(ui, info);
        let left = (anchor.origin.x - PADDING - 8.0).clamp(8.0, (window_width - WIDTH - 8.0).max(8.0));
        let target = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, WIDTH, height);
        self.popover.open(ui, config, anchor, target);
    }

    /// Refreshes the content of an open popover.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window_width: f32,
        info: &NetworkInfo,
    ) {
        if self.is_open() {
            self.open(ui, config, anchor, window_width, info);
        }
    }

    /// Builds the content; returns the popover's height.
    fn fill(&mut self, ui: &mut Ui, info: &NetworkInfo) -> f32 {
        ui.clear_children(self.plate);
        // Content heights, summed to size the popover (children are GAP apart).
        let mut heights: Vec<f32> = vec![HEADER];
        let header = ui.add(self.plate, Widget::container(), fixed_row(HEADER, 12.0));
        let (icon, paint) = match info.state {
            ChipState::Up => (Icon::Network, Paint::Shield),
            ChipState::Connecting => (Icon::Network, Paint::Accent),
            ChipState::Down => (Icon::WifiOff, Paint::Fixed(axion_config::Color::rgb(0xE2453A))),
            ChipState::Bypassed => (Icon::GlobeLock, Paint::Secondary),
        };
        ui.add(header, Widget::icon(icon, 26.0).with_paint(paint), Style::default());
        let titles = ui.add(
            header,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        let t = ui.add_label(titles, &info.title, TextStyle::BodyEmphasis, shrink());
        ui.set_label_lines(t, 1);
        secondary(ui, titles, &info.detail, TextStyle::Footnote);

        // The session route: the primary action.
        if let Some(name) = &info.session_active {
            heights.push(SESSION_NOTE);
            let note_row = ui.add(self.plate, Widget::container(), fixed_row(SESSION_NOTE, 0.0));
            let n = secondary(
                ui,
                note_row,
                &tr_fmt!(
                    "Session route: every tab goes through {name} until you switch it off. Site routes are set aside meanwhile.",
                    name = name
                ),
                TextStyle::Footnote,
            );
            ui.set_label_lines(n, 3);
            heights.push(SESSION_BUTTON);
            session_button(
                ui,
                self.plate,
                SESSION_OFF,
                &tr_fmt!("Stop using {name} for this session", name = name),
                true,
            );
        }
        for (i, name) in info.session_offers.iter().take(MAX_SESSION).enumerate() {
            let primary = i == 0 && info.session_active.is_none();
            heights.push(SESSION_BUTTON);
            session_button(
                ui,
                self.plate,
                ActionId(SESSION_BASE + i as u32),
                &tr_fmt!("Use {name} for this session", name = name),
                primary,
            );
        }

        // The route of this site, as long as no session route sets site routes aside.
        if info.session_active.is_none() {
            heights.push(LABEL);
            let label_row = ui.add(self.plate, Widget::container(), fixed_row(LABEL, 0.0));
            secondary(ui, label_row, &tr_fmt!("Route for {site}", site = info.site), TextStyle::Footnote);

            for (i, (label, selected)) in info.choices.iter().take(MAX_CHOICES).enumerate() {
                heights.push(CHOICE);
                let row = ui.add(
                    self.plate,
                    Widget::container()
                        .with_action(ActionId(CHOICE_BASE + i as u32))
                        .with_isolated()
                        .with_radius(CAPSULE)
                        .with_background(Background::Glass(GlassLevel::Thin)),
                    Style {
                        size: Size { width: auto(), height: length(CHOICE) },
                        flex_shrink: 0.0,
                        padding: Rect {
                            left: length(14.0),
                            right: length(12.0),
                            top: length(0.0),
                            bottom: length(0.0),
                        },
                        ..row_style(8.0)
                    },
                );
                let l = ui.add_label(
                    row,
                    label,
                    TextStyle::Body,
                    Style {
                        flex_grow: 1.0,
                        flex_shrink: 1.0,
                        min_size: Size { width: length(0.0), height: auto() },
                        ..Default::default()
                    },
                );
                ui.set_label_lines(l, 1);
                if *selected {
                    ui.add(row, Widget::icon(Icon::Check, 16.0).with_paint(Paint::Accent), Style::default());
                }
            }

            let note = if info.has_rule {
                tr("This site has its own route. Choose Default to remove it.")
            } else {
                tr("Applies to this site and its subdomains, and is saved in the network settings.")
            };
            heights.push(NOTE);
            let note_row = ui.add(self.plate, Widget::container(), fixed_row(NOTE, 0.0));
            let n = secondary(ui, note_row, note, TextStyle::Footnote);
            ui.set_label_lines(n, 2);
        }

        heights.push(FOOTER);
        let footer = ui.add(self.plate, Widget::container(), fixed_row(FOOTER, 8.0));
        let spacer = ui.add(footer, Widget::container(), Style { flex_grow: 1.0, ..Default::default() });
        let _ = spacer;
        let button = ui.add(
            footer,
            Widget::container()
                .with_action(SETTINGS)
                .with_isolated()
                .with_radius(CAPSULE)
                .with_background(Background::Glass(GlassLevel::Thin)),
            Style {
                size: Size { width: auto(), height: length(FOOTER) },
                padding: Rect {
                    left: length(12.0),
                    right: length(8.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..row_style(4.0)
            },
        );
        ui.add_label(button, tr("Network settings…"), TextStyle::Callout, Style::default());
        ui.add(button, Widget::icon(Icon::ChevronRight, 14.0).with_paint(Paint::Secondary), Style::default());
        content_height(&heights)
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.popover.close(ui, config);
    }

    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    pub fn action(&self, id: ActionId) -> Option<PopoverAction> {
        Self::action_of(id)
    }

    fn action_of(id: ActionId) -> Option<PopoverAction> {
        if id == SETTINGS {
            return Some(PopoverAction::OpenSettings);
        }
        if id == SESSION_OFF {
            return Some(PopoverAction::SessionOff);
        }
        if let Some(i) = id.0.checked_sub(SESSION_BASE).map(|i| i as usize)
            && i < MAX_SESSION
        {
            return Some(PopoverAction::SessionOn(i));
        }
        let i = id.0.checked_sub(CHOICE_BASE)? as usize;
        (i < MAX_CHOICES).then_some(PopoverAction::Choose(i))
    }

    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }
}

/// The popover's height for these content rows: padding, the rows and the gaps between them.
fn content_height(rows: &[f32]) -> f32 {
    let gaps = rows.len().saturating_sub(1) as f32 * GAP;
    2.0 * (PADDING + INSET) + rows.iter().sum::<f32>() + gaps
}

/// A full-width glass button: the primary one is tinted with the accent, the others are plain
/// glass (never a fill behind the glass).
fn session_button(ui: &mut Ui, parent: NodeId, action: ActionId, text: &str, primary: bool) {
    let background = if primary {
        Background::TintedGlass(GlassLevel::Thin, Paint::Accent)
    } else {
        Background::Glass(GlassLevel::Thin)
    };
    let button = ui.add(
        parent,
        Widget::container()
            .with_action(action)
            .with_isolated()
            .with_radius(CAPSULE)
            .with_background(background),
        Style {
            size: Size { width: auto(), height: length(SESSION_BUTTON) },
            flex_shrink: 0.0,
            justify_content: JustifyContent::CENTER,
            padding: Rect { left: length(14.0), right: length(14.0), top: length(0.0), bottom: length(0.0) },
            ..row_style(8.0)
        },
    );
    let label = ui.add_label(
        button,
        text,
        if primary { TextStyle::BodyEmphasis } else { TextStyle::Body },
        Style {
            flex_shrink: 1.0,
            min_size: Size { width: length(0.0), height: auto() },
            ..Default::default()
        },
    );
    ui.set_label_lines(label, 1);
    if primary && let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::OnAccent);
    }
}

fn uniform(v: f32) -> Rect<LengthPercentage> {
    Rect { left: length(v), right: length(v), top: length(v), bottom: length(v) }
}

fn row_style(gap: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER,
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }
}

fn fixed_row(height: f32, gap: f32) -> Style {
    Style { size: Size { width: auto(), height: length(height) }, flex_shrink: 0.0, ..row_style(gap) }
}

fn shrink() -> Style {
    Style { flex_shrink: 1.0, min_size: Size { width: length(0.0), height: auto() }, ..Default::default() }
}

fn secondary(ui: &mut Ui, parent: NodeId, text: &str, style: TextStyle) -> NodeId {
    let label = ui.add_label(parent, text, style, shrink());
    if let Some(w) = ui.widget_mut(label) {
        w.set_paint(Paint::Secondary);
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every id the popover can produce.
    fn all_ids() -> Vec<u32> {
        let mut ids = vec![SETTINGS.0, SESSION_OFF.0];
        ids.extend((0..MAX_SESSION as u32).map(|i| SESSION_BASE + i));
        ids.extend((0..MAX_CHOICES as u32).map(|i| CHOICE_BASE + i));
        ids
    }

    #[test]
    fn action_ids_are_unique_and_stay_in_this_popovers_range() {
        let ids = all_ids();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "two actions share an id: {ids:?}");
        // The translate popover owns 6900.., the circuit popover 6970..: stay in between.
        assert!(ids.iter().all(|id| (6950..6970).contains(id)), "{ids:?}");
        assert!(SESSION_BASE + MAX_SESSION as u32 <= CHOICE_BASE, "session ids must not reach the choices");
    }

    #[test]
    fn ids_map_back_to_their_actions() {
        let p = NetworkPopover::action_of;
        assert!(matches!(p(SETTINGS), Some(PopoverAction::OpenSettings)));
        assert!(matches!(p(SESSION_OFF), Some(PopoverAction::SessionOff)));
        assert!(matches!(p(ActionId(SESSION_BASE + 2)), Some(PopoverAction::SessionOn(2))));
        assert!(matches!(p(ActionId(CHOICE_BASE + 3)), Some(PopoverAction::Choose(3))));
        assert!(p(ActionId(ACTION_BASE)).is_none());
        assert!(p(ActionId(CHOICE_BASE + MAX_CHOICES as u32)).is_none());
    }

    #[test]
    fn height_counts_padding_rows_and_gaps() {
        assert_eq!(content_height(&[]), 2.0 * (PADDING + INSET));
        assert_eq!(content_height(&[10.0, 20.0]), 2.0 * (PADDING + INSET) + 30.0 + GAP);
    }
}
