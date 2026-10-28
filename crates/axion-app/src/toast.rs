//! Toasts: a small glass notification ("Link copied") that drips out of the control that
//! caused it, stays for `toast.duration_seconds` and flows back into it. Built on
//! [`Popover`], so it uses the same liquid spring as every other glass popover (no easing
//! curves of its own) and works with the sidebar open and over the Home page alike.
//!
//! Anything can raise one with [`Toast::show`]; a new toast replaces the one on screen.
//!
//! Placement: a toast never covers a popover, menu or sheet. Everything that floats claims its
//! glass with `Ui::claim_space` when it is built (every [`Popover`] does, and the alert and
//! sheet kinds); each frame the toast asks `Ui::claimed_areas` and, by [`place_clear`], sits
//! below the lowest claimed area it would overlap. When a popover opens or closes while the
//! toast is up it glides to its new place on the same springs (and snaps with Reduce Motion).
//! The order, top to bottom: popover, toast, and the Now Playing card lifts out of the toast's
//! way (see `NowPlayingCard::set_lift`).

use axion_compositor::{Icon, Rect as Frame};
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{ActionId, Background, CAPSULE, Paint, TextStyle, Ui, Widget};

use crate::popover::Popover;

const HEIGHT: f32 = 44.0;
const PADDING: f32 = 18.0;
const ICON: f32 = 18.0;
const GAP: f32 = 8.0;
/// Rough advance of one character of the toast's text (logical px).
const CHAR_WIDTH: f32 = 8.0;
const MIN_WIDTH: f32 = 120.0;
const GAP_BELOW_ANCHOR: f32 = 14.0;
/// The click id of a toast's button.
const ACTION: ActionId = ActionId(9210);
const BUTTON_HEIGHT: f32 = 30.0;
const BUTTON_PADDING: f32 = 14.0;

/// Kept free at the window's edges.
const WINDOW_MARGIN: f32 = 8.0;

pub struct Toast {
    popover: Popover,
    icon: NodeId,
    label: NodeId,
    /// The button of a toast with an action ("Update"), hidden otherwise, and its label.
    button: NodeId,
    button_label: NodeId,
    /// Seconds until it flows back (while shown).
    remaining: Option<f32>,
    /// Where it would sit with nothing in the way, and where it sits now.
    natural: Frame,
    placed: Frame,
    /// The window's size when it was raised (logical px).
    window: glam::Vec2,
}

impl Toast {
    /// `parent` is the toolbar, so the toast's glass flows out of the bar's.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            HEIGHT / 2.0,
            Style {
                size: Size { width: auto(), height: length(HEIGHT) },
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(GAP), height: length(0.0) },
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        )
        .detached();
        let plate = popover.root();
        let icon = ui.add(
            plate,
            Widget::icon(Icon::Check, ICON).with_paint(Paint::Accent),
            Style { flex_shrink: 0.0, ..Default::default() },
        );
        let label = ui.add_label(plate, "", TextStyle::Callout, Style::default());
        let button = ui.add(
            plate,
            Widget::container()
                .with_action(ACTION)
                .with_radius(CAPSULE)
                .with_background(Background::Fill(Paint::Accent, 0.16)),
            Style {
                size: Size { width: auto(), height: length(BUTTON_HEIGHT) },
                flex_shrink: 0.0,
                padding: Rect {
                    left: length(BUTTON_PADDING),
                    right: length(BUTTON_PADDING),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                margin: Rect {
                    left: length(4.0),
                    right: length(-8.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                justify_content: JustifyContent::CENTER,
                align_items: AlignItems::CENTER,
                ..Default::default()
            },
        );
        let button_label = ui.add_label(button, "", TextStyle::Callout, Style::default());
        if let Some(w) = ui.widget_mut(button_label) {
            w.set_paint(Paint::Accent);
        }
        ui.set_visible(button, false);
        Self {
            popover,
            icon,
            label,
            button,
            button_label,
            remaining: None,
            natural: Frame::default(),
            placed: Frame::default(),
            window: glam::Vec2::ZERO,
        }
    }

    /// Drips out of `anchor` (the control that caused it) with `text` and `icon`, right-aligned
    /// with it and kept inside the window; stays for the configured time.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window: glam::Vec2,
        icon: Icon,
        text: &str,
    ) {
        self.show_with_action(ui, config, anchor, window, icon, text, None);
    }

    /// Like [`Self::show`], with a button (`action`: its label) that stays longer; a click on
    /// it arrives as `UiEvent::Clicked` with an id [`Self::owns`] recognises.
    #[allow(clippy::too_many_arguments)]
    pub fn show_with_action(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        anchor: Frame,
        window: glam::Vec2,
        icon: Icon,
        text: &str,
        action: Option<&str>,
    ) {
        ui.set_text(self.label, text);
        ui.set_icon(self.icon, icon);
        ui.set_visible(self.button, action.is_some());
        let mut width = toast_width(text);
        if let Some(action) = action {
            ui.set_text(self.button_label, action);
            width += 2.0 * BUTTON_PADDING + action.chars().count() as f32 * CHAR_WIDTH + GAP;
        }
        let right = anchor.origin.x + anchor.size.x;
        let max_left = (window.x - width - WINDOW_MARGIN).max(WINDOW_MARGIN);
        let left = (right - width).clamp(WINDOW_MARGIN, max_left);
        self.natural = Frame::new(left, anchor.origin.y + anchor.size.y + GAP_BELOW_ANCHOR, width, HEIGHT);
        self.window = window;
        self.placed = self.clear_of_popovers(ui, config);
        self.popover.open(ui, config, anchor, self.placed);
        let seconds = if action.is_some() {
            config.toast.action_duration_seconds
        } else {
            config.toast.duration_seconds
        };
        self.remaining = Some(seconds.max(0.0));
    }

    /// Where the toast sits now: its natural place, moved below whatever is open there.
    fn clear_of_popovers(&self, ui: &Ui, config: &Config) -> Frame {
        let occupied = ui.claimed_areas(&[self.popover.root()]);
        place_clear(self.natural, &occupied, config.toast.clearance.max(0.0), self.window.y - WINDOW_MARGIN)
    }

    /// The toast's glass as drawn now (`None` while it is away); the Now Playing card keeps
    /// clear of it.
    pub fn area(&self) -> Option<Frame> {
        self.popover.frame()
    }

    /// The centre of the toast's button (capture aid for clicking it).
    pub fn button_center(&self, ui: &Ui) -> glam::Vec2 {
        ui.frame(self.button).center()
    }

    /// Whether `id` is the click id of the toast's button.
    pub fn owns(&self, id: ActionId) -> bool {
        id == ACTION
    }

    /// Flows back into its anchor now.
    pub fn dismiss(&mut self, ui: &mut Ui, config: &Config) {
        self.remaining = None;
        self.popover.close(ui, config);
    }

    /// Runs the timer and advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, config: &Config, dt: f32) {
        if self.remaining.is_some() && self.popover.is_open() {
            // A popover opened or closed in its way: glide to the new place.
            let placed = self.clear_of_popovers(ui, config);
            if placed != self.placed {
                self.placed = placed;
                self.popover.retarget(ui, placed);
            }
        }
        if let Some(left) = self.remaining {
            let left = left - dt;
            if left <= 0.0 {
                self.dismiss(ui, config);
            } else {
                self.remaining = Some(left);
            }
        }
        self.popover.animate(ui, dt);
    }
}

/// Moves `natural` (the toast's frame with nothing in the way) down until it overlaps none of
/// the `occupied` frames that share its columns, keeping `clearance` below them and its
/// bottom above `limit` (the window's lower edge less a margin). Where that is impossible
/// (a popover reaching to the bottom of the window) it stays as low as the window allows.
pub fn place_clear(natural: Frame, occupied: &[Frame], clearance: f32, limit: f32) -> Frame {
    let (left, right) = (natural.origin.x, natural.origin.x + natural.size.x);
    let mut blockers: Vec<Frame> =
        occupied.iter().copied().filter(|o| o.origin.x < right && o.origin.x + o.size.x > left).collect();
    blockers.sort_by(|a, b| a.origin.y.total_cmp(&b.origin.y));
    let mut y = natural.origin.y;
    let mut x = left;
    for o in blockers {
        let (top, bottom) = (o.origin.y, o.origin.y + o.size.y);
        if top < y + natural.size.y + clearance && bottom + clearance > y {
            y = bottom + clearance;
            // Stacked under it: no further right than the popover's edge.
            x = x.min((o.origin.x + o.size.x - natural.size.x).max(o.origin.x));
        }
    }
    y = y.min(limit - natural.size.y).max(natural.origin.y);
    Frame::new(x, y, natural.size.x, natural.size.y)
}

/// How far (logical px) a card whose resting frame is `card` must be lifted to clear `blocker`
/// (a toast above it): 0 when they do not overlap.
pub fn lift_over(card: Frame, blocker: Frame, clearance: f32) -> f32 {
    let (cl, cr) = (card.origin.x, card.origin.x + card.size.x);
    let (bl, br) = (blocker.origin.x, blocker.origin.x + blocker.size.x);
    let (ctop, bbottom) = (card.origin.y, blocker.origin.y + blocker.size.y);
    let (bt, cb) = (blocker.origin.y, card.origin.y + card.size.y);
    if cl < br && cr > bl && ctop < bbottom + clearance && cb > bt { cb - bt + clearance } else { 0.0 }
}

/// The toast's width for `text` (laid out before the drip knows its target).
fn toast_width(text: &str) -> f32 {
    (2.0 * PADDING + ICON + GAP + text.chars().count() as f32 * CHAR_WIDTH).max(MIN_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f32, y: f32, w: f32, h: f32) -> Frame {
        Frame::new(x, y, w, h)
    }

    #[test]
    fn toast_stays_put_when_nothing_is_in_the_way() {
        let natural = f(900.0, 60.0, 200.0, 44.0);
        assert_eq!(place_clear(natural, &[], 10.0, 1000.0), natural);
        // A popover far to the left, or one already above it, is no obstacle.
        let left = f(100.0, 50.0, 300.0, 400.0);
        let above = f(850.0, 0.0, 300.0, 40.0);
        assert_eq!(place_clear(natural, &[left, above], 10.0, 1000.0), natural);
    }

    #[test]
    fn toast_goes_below_a_popover_in_its_columns() {
        let natural = f(900.0, 60.0, 200.0, 44.0);
        let popover = f(820.0, 56.0, 300.0, 380.0);
        let placed = place_clear(natural, &[popover], 10.0, 1000.0);
        assert_eq!((placed.origin.x, placed.size), (900.0, natural.size));
        assert_eq!(placed.origin.y, 56.0 + 380.0 + 10.0);
    }

    #[test]
    fn toast_lines_up_with_the_popover_edge_it_stacks_under() {
        // The toast hangs out past the popover's right edge: it tucks in to it.
        let natural = f(900.0, 60.0, 200.0, 44.0);
        let popover = f(700.0, 56.0, 300.0, 120.0);
        let placed = place_clear(natural, &[popover], 12.0, 1000.0);
        assert_eq!(placed.origin.x + placed.size.x, 1000.0);
        assert_eq!(placed.origin.y, 188.0);
    }

    #[test]
    fn toast_stacks_below_the_lowest_of_several() {
        let natural = f(900.0, 60.0, 200.0, 44.0);
        let a = f(820.0, 56.0, 300.0, 100.0);
        // A second one right under the first: the toast clears both, in any order.
        let b = f(820.0, 160.0, 300.0, 100.0);
        for list in [[a, b], [b, a]] {
            assert_eq!(place_clear(natural, &list, 10.0, 1000.0).origin.y, 270.0);
        }
        // A gap too small for the toast does not let it squeeze in.
        let c = f(820.0, 170.0, 300.0, 100.0);
        assert_eq!(place_clear(natural, &[a, c], 10.0, 1000.0).origin.y, 280.0);
    }

    #[test]
    fn toast_stays_inside_the_window() {
        let natural = f(900.0, 60.0, 200.0, 44.0);
        let tall = f(820.0, 56.0, 300.0, 900.0);
        let placed = place_clear(natural, &[tall], 10.0, 800.0);
        assert_eq!(placed.origin.y + placed.size.y, 800.0);
    }

    #[test]
    fn card_lifts_clear_of_a_toast_only_when_they_overlap() {
        let card = f(10.0, 700.0, 240.0, 80.0);
        assert_eq!(lift_over(card, f(900.0, 700.0, 200.0, 44.0), 10.0), 0.0);
        assert_eq!(lift_over(card, f(100.0, 60.0, 200.0, 44.0), 10.0), 0.0);
        // A toast pushed down into the card's place: the card rises over its top.
        let toast = f(100.0, 720.0, 200.0, 44.0);
        assert_eq!(lift_over(card, toast, 10.0), 780.0 - 720.0 + 10.0);
    }

    #[test]
    fn width_grows_with_text_but_has_a_floor() {
        assert_eq!(toast_width("Hi"), MIN_WIDTH);
        assert!(toast_width("A much longer notification") > toast_width("Link copied"));
    }
}
