//! Closing a tab: the tab shrinks and fades away (its glass squashes out of the bar, the sidebar
//! list or the overview grid) while the tabs beside it slide into the gap on springs.
//!
//! The tab model drops the tab at once (page switch, media and lifecycle code never wait for an
//! animation). The chrome rebuilds its rows from the model, so the closed tab's node is *taken
//! out of the old tree before the rebuild* and lives on as a [`Dying`] ghost in a plane of its
//! own, drawn where the tab was (where it was *drawn*: a tab that was still sliding from an
//! earlier close starts from there). The ghost is outside the layout and passes the pointer
//! through, so the tabs under it stay clickable; rapid closes simply add more ghosts. The other
//! tabs slide because their drawn position follows their laid-out one on a spring ([`Slide`]),
//! seeded with the positions from before the rebuild.
//!
//! Reduce Motion and flat mode keep only a short fade (no squash, no slide in Reduce Motion).

use std::collections::HashMap;
use std::hash::Hash;

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_engine::ViewId;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::chrome::{Chrome, Surface};
use crate::drip;
use crate::favicons::{Favicons, favicon_node};

/// A ghost counts as gone below this share of its size/opacity.
const GONE: f32 = 0.03;
/// A tab closer than this to its place has arrived (logical px).
const SETTLED_PX: f32 = 0.5;

/// How a closing tab shrinks, by where it lies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A tab segment in the bar: it narrows to its centre.
    Bar,
    /// A sidebar row: it flattens to its centre line.
    Row,
    /// An overview card: it shrinks to its centre.
    Card,
}

/// The spring a closing tab shrinks on (Reduce Motion: a quick, plain fade).
pub fn close_spring(config: &Config) -> Spring {
    drip::spring(config, config.motion.tab_close)
}

/// The spring the tabs beside it slide on.
pub fn slide_spring(config: &Config) -> Spring {
    drip::spring(config, config.motion.snappy)
}

/// Whether a closing tab squashes (otherwise it only fades: Reduce Motion, flat mode).
pub fn squashes(config: &Config) -> bool {
    !config.glass.reduce_motion && !config.glass.is_flat()
}

/// Where a tab at `frame` is drawn when it has shrunk to `life` (1: whole, 0: gone), and how
/// opaque it is. Without `squash` only the opacity changes.
pub fn pose(frame: Frame, kind: Kind, life: f32, squash: bool) -> (Frame, f32) {
    let s = life.clamp(0.0, 1.0);
    if !squash {
        return (frame, s);
    }
    let scale = match kind {
        Kind::Bar => Vec2::new(s, 0.55 + 0.45 * s),
        Kind::Row => Vec2::new(0.9 + 0.1 * s, s),
        Kind::Card => Vec2::splat(0.6 + 0.4 * s),
    };
    let size = frame.size * scale;
    (Frame { origin: frame.origin + (frame.size - size) / 2.0, size }, s)
}

/// A tab that was closed and is shrinking away. Its node is not part of the layout any more.
pub struct Dying {
    pub node: NodeId,
    /// Where it was drawn when it was closed.
    frame: Frame,
    kind: Kind,
    life: Animated<f32>,
    squash: bool,
    /// An overview card's thumbnail (its view, to keep showing the page).
    pub image: Option<(NodeId, ViewId)>,
}

impl Dying {
    /// Takes `node` (laid out at `frame`, absolutely positioned there by the caller) on.
    pub fn new(ui: &mut Ui, node: NodeId, frame: Frame, kind: Kind, config: &Config) -> Self {
        let mut life = Animated::new(1.0, close_spring(config));
        life.target = 0.0;
        let mut style = ui.style(node);
        style.position = Position::Absolute;
        style.inset =
            Rect { left: length(frame.origin.x), right: auto(), top: length(frame.origin.y), bottom: auto() };
        style.size = Size { width: length(frame.size.x), height: length(frame.size.y) };
        style.margin = Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) };
        style.flex_grow = 0.0;
        style.flex_shrink = 0.0;
        style.min_size = Size { width: length(0.0), height: length(0.0) };
        ui.set_style(node, style);
        ui.set_passthrough(node, true);
        ui.set_visual_frame(node, None);
        Self { node, frame, kind, life, squash: squashes(config), image: None }
    }

    pub fn step(&mut self, ui: &mut Ui, dt: f32) {
        self.life.step(dt);
        let (visual, opacity) = pose(self.frame, self.kind, self.life.value, self.squash);
        ui.set_visual_frame(self.node, self.squash.then_some(visual));
        ui.set_opacity(self.node, opacity);
    }

    /// Gone (or as good as): the node can be removed.
    pub fn is_gone(&self) -> bool {
        self.life.value <= GONE
    }
}

/// Drawn positions of items that follow their laid-out ones on springs, so a change of layout
/// (a tab closed, a row dropped) slides instead of jumping. Entries live only while something
/// is still on its way.
pub struct Slide<K> {
    places: HashMap<K, Animated<Vec2>>,
}

impl<K> Default for Slide<K> {
    fn default() -> Self {
        Self { places: HashMap::new() }
    }
}

impl<K: Copy + Eq + Hash> Slide<K> {
    pub fn is_idle(&self) -> bool {
        self.places.is_empty()
    }

    /// Notes where items are drawn now (before a rebuild changes their layout); keys that are
    /// already sliding keep their motion.
    pub fn remember(&mut self, items: impl IntoIterator<Item = (K, Vec2)>, spring: Spring) {
        for (key, at) in items {
            self.places.entry(key).or_insert_with(|| Animated::new(at, spring));
        }
    }

    /// Starts `key` at `from`, to spring into its laid-out place.
    pub fn start(&mut self, key: K, from: Vec2, spring: Spring) {
        self.places.insert(key, Animated::new(from, spring));
    }

    /// Where `key` is drawn now, if it is on its way.
    #[cfg(test)]
    fn place(&self, key: K) -> Option<Vec2> {
        self.places.get(&key).map(|p| p.value)
    }

    /// One step for the item `key`, laid out at `target`: its drawn position, or `None` once it
    /// has arrived (or `snap`: Reduce Motion).
    fn follow(&mut self, key: K, target: Vec2, spring: Spring, snap: bool, dt: f32) -> Option<Vec2> {
        let place = self.places.entry(key).or_insert_with(|| Animated::new(target, spring));
        place.spring = spring;
        place.target = target;
        if snap {
            place.snap(target);
        }
        place.step(dt);
        if place.is_settled(SETTLED_PX) {
            place.snap(target);
            None
        } else {
            Some(place.value)
        }
    }

    /// After layout: draws every node of `items` where its spring is. `hold` keeps the entries
    /// when everything has arrived (a drag is on). Returns whether all have arrived.
    pub fn animate(
        &mut self,
        ui: &mut Ui,
        items: &[(K, NodeId)],
        spring: Spring,
        snap: bool,
        dt: f32,
    ) -> bool {
        let mut settled = true;
        for &(key, node) in items {
            let frame = ui.frame(node);
            match self.follow(key, frame.origin, spring, snap, dt) {
                None => ui.set_visual_frame(node, None),
                Some(at) => {
                    settled = false;
                    ui.set_visual_frame(node, Some(Frame { origin: at, size: frame.size }));
                }
            }
        }
        settled
    }

    /// Forgets all motion (everything snaps to its layout).
    pub fn clear(&mut self) {
        self.places.clear();
    }
}

/// A tab waiting for the next rebuild of the chrome, to be taken out of the old tree first.
struct Closing {
    view: ViewId,
    url: String,
    label: String,
}

/// The ghosts of closed tabs in the tab bar and the sidebar.
pub struct TabClose {
    plane: NodeId,
    pending: Vec<Closing>,
    dying: Vec<Dying>,
}

impl TabClose {
    /// Builds the ghosts' plane (call after the chrome and the overview, so it lies above them).
    pub fn build(ui: &mut Ui) -> Self {
        let plane = ui.add(
            ui.root(),
            Widget::container(),
            Style {
                position: Position::Absolute,
                inset: Rect { left: length(0.0), right: length(0.0), top: length(0.0), bottom: length(0.0) },
                ..Default::default()
            },
        );
        ui.set_passthrough(plane, true);
        Self { plane, pending: Vec::new(), dying: Vec::new() }
    }

    /// A tab is about to be closed (call before the model drops it).
    pub fn begin(&mut self, view: ViewId, url: &str, label: String) {
        self.pending.push(Closing { view, url: url.to_owned(), label });
    }

    /// Forgets the tabs waiting (animation off, or the bars are not what is shown).
    pub fn discard_pending(&mut self) {
        self.pending.clear();
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Before the chrome rebuilds its tabs: takes the closed tabs' nodes out of the old tree and
    /// turns them into ghosts where they were drawn.
    pub fn adopt(
        &mut self,
        ui: &mut Ui,
        chrome: &mut Chrome,
        favicons: &Favicons,
        config: &Config,
        window: Vec2,
    ) {
        let screen = Frame { origin: Vec2::ZERO, size: window };
        for closing in std::mem::take(&mut self.pending) {
            for taken in chrome.detach_tab(ui, closing.view) {
                let kind = match taken.surface {
                    Surface::Bar => Kind::Bar,
                    Surface::Sidebar => Kind::Row,
                };
                if !fully_inside(taken.frame, screen) {
                    if let Some(node) = taken.node {
                        ui.remove(node);
                    }
                    continue;
                }
                let node = match taken.node {
                    Some(node) => {
                        let mut children = ui.children(self.plane);
                        children.push(node);
                        ui.set_children(self.plane, &children);
                        node
                    }
                    // The compact bar's open segment stays (it is the address); a look-alike
                    // of it leaves.
                    None => self.lookalike(ui, &closing, favicons, taken.frame),
                };
                self.dying.push(Dying::new(ui, node, taken.frame, kind, config));
            }
        }
    }

    /// A glass capsule with the tab's icon and title, as the open segment looked.
    fn lookalike(&self, ui: &mut Ui, closing: &Closing, favicons: &Favicons, frame: Frame) -> NodeId {
        let node = ui.add(
            self.plane,
            Widget::container().with_radius(CAPSULE).with_background(Background::Glass(GlassLevel::Thin)),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(9.0), height: length(0.0) },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                size: Size { width: length(frame.size.x), height: length(frame.size.y) },
                ..Default::default()
            },
        );
        favicon_node(ui, node, &closing.url, 16.0, favicons);
        ui.add_label(
            node,
            &closing.label,
            TextStyle::BodyEmphasis,
            Style {
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        node
    }

    /// Every frame: the ghosts shrink; gone ones are removed.
    pub fn animate(&mut self, ui: &mut Ui, dt: f32) {
        for ghost in &mut self.dying {
            ghost.step(ui, dt);
        }
        self.dying.retain(|ghost| {
            let gone = ghost.is_gone();
            if gone {
                ui.remove(ghost.node);
            }
            !gone
        });
    }
}

fn fully_inside(inner: Frame, outer: Frame) -> bool {
    let (a, b) = (inner.origin, inner.origin + inner.size);
    let (c, d) = (outer.origin, outer.origin + outer.size);
    inner.size.x > 0.0
        && inner.size.y > 0.0
        && a.x >= c.x - 0.5
        && a.y >= c.y - 0.5
        && b.x <= d.x + 0.5
        && b.y <= d.y + 0.5
}

/// The width a tab gets while the bar holds the widths of the tabs it had before a close (the
/// next close button stays under the pointer): the width it would get anyway, but never wider
/// than the held one.
pub fn held_width(wanted: f32, held: Option<f32>) -> f32 {
    held.map_or(wanted, |h| wanted.min(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame::new(100.0, 10.0, 200.0, 36.0)
    }

    #[test]
    fn a_whole_tab_is_drawn_as_it_is() {
        for kind in [Kind::Bar, Kind::Row, Kind::Card] {
            let (f, a) = pose(frame(), kind, 1.0, true);
            assert_eq!(f, frame());
            assert_eq!(a, 1.0);
        }
    }

    #[test]
    fn shrinking_keeps_the_centre() {
        for kind in [Kind::Bar, Kind::Row, Kind::Card] {
            let (f, a) = pose(frame(), kind, 0.4, true);
            assert!((f.center() - frame().center()).length() < 1e-3);
            assert!(f.size.x < 200.0 && f.size.y < 36.0 + 1e-3);
            assert!((a - 0.4).abs() < 1e-6);
        }
        // Bars narrow, rows flatten.
        let (bar, _) = pose(frame(), Kind::Bar, 0.0, true);
        assert_eq!(bar.size.x, 0.0);
        let (row, _) = pose(frame(), Kind::Row, 0.0, true);
        assert_eq!(row.size.y, 0.0);
    }

    #[test]
    fn plain_fade_keeps_the_frame() {
        let (f, a) = pose(frame(), Kind::Bar, 0.25, false);
        assert_eq!(f, frame());
        assert_eq!(a, 0.25);
        // Overshoot of the spring never draws a negative size or opacity.
        let (f, a) = pose(frame(), Kind::Card, -0.2, true);
        assert!(f.size.x > 0.0 && a == 0.0);
    }

    #[test]
    fn held_widths_never_grow_the_tabs() {
        assert_eq!(held_width(240.0, None), 240.0);
        assert_eq!(held_width(240.0, Some(180.0)), 180.0);
        // Not enough room even for the held width: the tabs shrink as usual.
        assert_eq!(held_width(120.0, Some(180.0)), 120.0);
    }

    #[test]
    fn neighbours_slide_into_the_gap() {
        let spring = Spring::new(0.25, 0.9);
        let mut slide: Slide<u32> = Slide::default();
        // Tab 2 was drawn at x = 300 and is now laid out at x = 100.
        slide.remember([(2, Vec2::new(300.0, 0.0))], spring);
        let mut last = 300.0;
        let mut frames = 0;
        loop {
            frames += 1;
            let at = slide.follow(2, Vec2::new(100.0, 0.0), spring, false, 1.0 / 120.0);
            let Some(at) = at else { break };
            assert!(at.x < 300.0 + 1.0 && at.x > 0.0, "x = {}", at.x);
            last = at.x;
            assert!(frames < 600, "never arrived");
        }
        assert!(last < 130.0, "should be nearly there before it snaps: {last}");
        // Remembering a tab that is already sliding keeps its motion.
        slide.remember([(2, Vec2::new(999.0, 0.0))], spring);
        assert_eq!(slide.place(2), Some(Vec2::new(100.0, 0.0)));
    }

    #[test]
    fn reduce_motion_snaps() {
        let spring = Spring::new(0.25, 1.0);
        let mut slide: Slide<u32> = Slide::default();
        slide.remember([(1, Vec2::new(300.0, 0.0))], spring);
        assert_eq!(slide.follow(1, Vec2::new(100.0, 0.0), spring, true, 1.0 / 120.0), None);
    }

    #[test]
    fn a_new_tab_does_not_slide() {
        let spring = Spring::new(0.25, 0.9);
        let mut slide: Slide<u32> = Slide::default();
        assert_eq!(slide.follow(7, Vec2::new(50.0, 0.0), spring, false, 1.0 / 120.0), None);
    }
}
