//! The card an extension's toolbar popup (`action.default_popup`) is shown in: glass that drips
//! out of the icon that was clicked, holding the popup page. The page is its own off-screen
//! browser (see `browser/ext_popup.rs`); its frames are drawn into the card's picture, the card
//! only follows the size the page wants. Built on [`Popover`] like the other popovers, so it
//! merges with the address field's glass and comes and goes the same way.

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_ui::taffy::prelude::*;
use axion_ui::{Ui, Widget};
use glam::Vec2;

use crate::home::Image;
use crate::popover::Popover;

/// Glass around the page.
const PADDING: f32 = 6.0;
const RADIUS: f32 = 22.0;
/// The page's own corners, inside the card.
const PAGE_RADIUS: f32 = 16.0;
/// Below the anchor at least this far, and beyond the glass merge distance: the card is
/// never pulled into the toolbar's glass.
const MIN_GAP: f32 = 14.0;
/// The card stays this far from the window's edges.
const EDGE: f32 = 8.0;
/// Space kept between the card and the extensions list.
const CLEARANCE: f32 = 16.0;

pub struct ExtensionPopupCard {
    popover: Popover,
    picture: NodeId,
    /// Where the card was last sent (logical px).
    placed: Frame,
}

/// Moves `natural` (the card with nothing in the way) clear of `list` (the extensions list,
/// which lies above it): below it when that fits in the window, else beside it on the left,
/// else where it was (the list then covers part of it).
pub fn place_clear(natural: Frame, list: Option<Frame>, window: Vec2) -> Frame {
    let Some(list) = list else { return natural };
    let overlaps = natural.origin.x < list.origin.x + list.size.x + CLEARANCE
        && natural.origin.x + natural.size.x + CLEARANCE > list.origin.x
        && natural.origin.y < list.origin.y + list.size.y + CLEARANCE
        && natural.origin.y + natural.size.y + CLEARANCE > list.origin.y;
    if !overlaps {
        return natural;
    }
    let below = list.origin.y + list.size.y + CLEARANCE;
    if below + natural.size.y <= window.y - EDGE {
        return Frame::new(natural.origin.x, below, natural.size.x, natural.size.y);
    }
    let left = list.origin.x - CLEARANCE - natural.size.x;
    if left >= EDGE {
        return Frame::new(left, natural.origin.y, natural.size.x, natural.size.y);
    }
    natural
}

impl ExtensionPopupCard {
    /// `parent` is the toolbar, so the card's glass merges with it.
    pub fn build(ui: &mut Ui, parent: NodeId) -> Self {
        let popover = Popover::build(
            ui,
            parent,
            RADIUS,
            Style {
                flex_direction: FlexDirection::Column,
                padding: Rect {
                    left: length(PADDING),
                    right: length(PADDING),
                    top: length(PADDING),
                    bottom: length(PADDING),
                },
                ..Default::default()
            },
        );
        let picture = ui.add(
            popover.root(),
            Widget::image().with_radius(PAGE_RADIUS),
            Style {
                size: Size { width: length(1.0), height: length(1.0) },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        Self { popover: popover.unjoined(), picture, placed: Frame::default() }
    }

    /// How far below its anchor the card sits.
    pub fn gap(config: &Config) -> f32 {
        MIN_GAP.max(config.glass.merge_distance() + 6.0)
    }

    /// The largest page that fits under `anchor` in a window of `window` (logical px).
    pub fn room(anchor: Frame, window: Vec2, gap: f32) -> Vec2 {
        let top = anchor.origin.y + anchor.size.y + gap;
        Vec2::new(
            (window.x - 2.0 * (EDGE + PADDING)).max(1.0),
            (window.y - top - EDGE - 2.0 * PADDING).max(1.0),
        )
    }

    /// Where the card goes for a page of `content` px: right-aligned with the anchor, inside
    /// the window.
    pub fn natural(anchor: Frame, window: Vec2, content: Vec2, gap: f32) -> Frame {
        let size = content + Vec2::splat(2.0 * PADDING);
        let right = anchor.origin.x + anchor.size.x + PADDING;
        let left = (right - size.x).clamp(EDGE, (window.x - size.x - EDGE).max(EDGE));
        Frame::new(left, anchor.origin.y + anchor.size.y + gap, size.x, size.y)
    }

    /// Drips out of `anchor` for a page of `content` px; an open card glides to the new size.
    pub fn show(&mut self, ui: &mut Ui, config: &Config, anchor: Frame, window: Vec2, content: Vec2) {
        let mut style = ui.style(self.picture);
        style.size = Size { width: length(content.x), height: length(content.y) };
        ui.set_style(self.picture, style);
        let target = Self::natural(anchor, window, content, Self::gap(config));
        self.placed = target;
        if self.popover.is_open() {
            self.popover.retarget(ui, target);
        } else {
            self.popover.open(ui, config, anchor, target);
        }
    }

    /// Moves an open card to `target` (it glides), e.g. out of the extensions list's way.
    pub fn follow(&mut self, ui: &mut Ui, target: Frame) {
        if (target.origin - self.placed.origin).abs().max_element() > 0.5
            || (target.size - self.placed.size).abs().max_element() > 0.5
        {
            self.placed = target;
            self.popover.retarget(ui, target);
        }
    }

    /// The page's latest frame (`None`: nothing yet). Only the part the card shows is drawn
    /// while the page is resized to a new size.
    pub fn set_picture(&mut self, ui: &mut Ui, image: Option<Image>, shown_px: Vec2) {
        let crop = image.as_ref().map(|(_, (w, h))| {
            let tex = Vec2::new(*w as f32, *h as f32).max(Vec2::ONE);
            Frame { origin: Vec2::ZERO, size: (shown_px / tex).min(Vec2::ONE) }
        });
        ui.set_image(self.picture, image);
        ui.set_image_crop(self.picture, crop);
    }

    /// Where the page is on screen (logical px).
    pub fn page_frame(&self, ui: &Ui) -> Frame {
        ui.frame(self.picture)
    }

    pub fn close(&mut self, ui: &mut Ui, config: &Config) {
        self.popover.close(ui, config);
    }

    /// Advances the drip; call every frame.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        self.popover.animate(ui, dt);
    }

    /// Whether `p` is on the card (its page or its glass).
    pub fn contains(&self, ui: &Ui, p: Vec2) -> bool {
        self.popover.contains(ui, p)
    }
}
