//! Dragging tabs: in the sidebar's lists (also from the normal tabs into the private ones and
//! back) and in the toolbar's tabs. A press on a tab becomes a drag once the pointer moved
//! `tabs.drag_threshold`; shorter moves stay clicks.
//!
//! While dragging, the tab follows the pointer as a lifted glass lens (on a spring, stretched
//! along its motion), its own row stays as an empty slot at the place it would drop, and the
//! other rows slide aside on springs (each row's drawn position follows its laid-out one).

use std::collections::HashMap;

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_engine::ViewId;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, TextStyle, Ui, Widget};
use glam::Vec2;

use crate::chrome::{Chrome, MoveKey, SidebarLayout, Surface, TabItem};
use crate::favicons::{Favicons, favicon_node};
use crate::tabs::Tab;

/// The lens grows this much when lifted.
const LIFT: f32 = 1.06;
/// Stretch per logical px/s of the lens's speed (up to `glass.squash`).
const STRETCH_PER_SPEED: f32 = 0.00008;
/// How far outside the sidebar (or the bar's band) a drop still counts, logical px.
const DROP_MARGIN: f32 = 48.0;
/// A row closer than this to its place stops being moved (logical px).
const SETTLED_PX: f32 = 0.5;

/// Where a dragged tab would go: its group, and its index there (among the group's other
/// tabs; `usize::MAX` for the end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub private: bool,
    pub index: usize,
}

/// A finished drag.
pub struct Dropped {
    pub item: TabItem,
    /// `None`: let go away from the tabs (nothing moves).
    pub target: Option<Target>,
}

struct Pending {
    item: TabItem,
    start: Vec2,
}

struct Active {
    item: TabItem,
    /// Pointer minus the item's top-left when the drag started.
    grab: Vec2,
    size: Vec2,
    pointer: Vec2,
    target: Option<Target>,
    sidebar: SidebarLayout,
    /// The bar's tabs (view, frame) when the drag started.
    bar: Vec<(ViewId, Frame)>,
    /// The sidebar's or bar's frame, for drops near it.
    area: Frame,
    lens: Animated<Vec2>,
}

pub struct TabDrag {
    pending: Option<Pending>,
    active: Option<Active>,
    /// The lens's plane, its glass capsule and content.
    plane: NodeId,
    lens: NodeId,
    /// Drawn positions of rows and segments, following their laid-out ones on springs.
    moving: HashMap<MoveKey, Animated<Vec2>>,
}

/// How many of `centers` lie before `pos`: the index a dragged item at `pos` goes to.
pub fn insertion_index(centers: &[f32], pos: f32) -> usize {
    centers.iter().filter(|&&c| c < pos).count()
}

/// The sidebar target for a dragged row whose centre is at `y`: the normal list above
/// "Private", the private list from it on (onto "Private" itself: the private tabs' start).
/// A folded or locked list takes the tab at its end.
pub fn sidebar_target(layout: &SidebarLayout, source: ViewId, y: f32) -> Target {
    let centers = |rows: &[(ViewId, Frame)]| -> Vec<f32> {
        rows.iter().filter(|(v, _)| *v != source).map(|(_, f)| f.center().y).collect()
    };
    let private = y >= layout.private_header.origin.y;
    let (open, rows) =
        if private { (layout.private_open, &layout.private) } else { (layout.normal_open, &layout.normal) };
    let index = if open { insertion_index(&centers(rows), y) } else { usize::MAX };
    Target { private, index }
}

/// The bar target for a dragged segment whose centre is at `x`.
pub fn bar_target(items: &[(ViewId, Frame)], source: ViewId, x: f32) -> usize {
    let centers: Vec<f32> = items.iter().filter(|(v, _)| *v != source).map(|(_, f)| f.center().x).collect();
    insertion_index(&centers, x)
}

fn spring(config: &Config) -> Spring {
    let motion = &config.motion;
    if config.glass.reduce_motion {
        Spring::new(0.15, 1.0)
    } else {
        Spring::new(motion.snappy.response * motion.speed, config.glass.damping(motion.snappy.damping))
    }
}

impl TabDrag {
    /// Builds the lens's plane (call last, so it lies above the chrome).
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
        let lens = ui.add(
            plane,
            Widget::container().with_background(Background::Glass(GlassLevel::Thin)).with_radius(CAPSULE),
            Style {
                position: Position::Absolute,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::CENTER,
                gap: Size { width: length(10.0), height: length(0.0) },
                padding: Rect {
                    left: length(14.0),
                    right: length(14.0),
                    top: length(0.0),
                    bottom: length(0.0),
                },
                ..Default::default()
            },
        );
        ui.set_visible(plane, false);
        Self { pending: None, active: None, plane, lens, moving: HashMap::new() }
    }

    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }

    /// A press on a tab: it may become a drag.
    pub fn press(&mut self, item: TabItem, at: Vec2) {
        self.pending = Some(Pending { item, start: at });
    }

    /// The pointer went up without dragging (a click).
    pub fn release_press(&mut self) {
        self.pending = None;
    }

    /// The pointer moved with the button down. Returns true when a drag starts now (the UI's
    /// press must then be cancelled, so no click follows).
    pub fn pointer_moved(
        &mut self,
        ui: &mut Ui,
        chrome: &Chrome,
        config: &Config,
        tab: Option<&Tab>,
        favicons: &Favicons,
        p: Vec2,
    ) -> bool {
        if let Some(active) = &mut self.active {
            active.pointer = p;
            return false;
        }
        let Some(pending) = &self.pending else { return false };
        if p.distance(pending.start) < config.tabs.drag_threshold.max(0.0) {
            return false;
        }
        let Some(pending) = self.pending.take() else { return false };
        let item = pending.item;
        let frame = ui.frame(item.node);
        let area = match item.surface {
            Surface::Sidebar => ui.frame(chrome.sidebar_node()),
            Surface::Bar => ui.parent(item.node).map(|n| ui.frame(n)).unwrap_or(frame),
        };
        let bar = chrome.tab_items(Surface::Bar).iter().map(|i| (i.view, ui.frame(i.node))).collect();
        let mut lens = Animated::new(frame.origin, spring(config));
        lens.target = p - (pending.start - frame.origin);
        self.active = Some(Active {
            item,
            grab: pending.start - frame.origin,
            size: frame.size,
            pointer: p,
            target: None,
            sidebar: chrome.sidebar_layout(ui),
            bar,
            area,
            lens,
        });
        // The lens: the tab's icon and title on lifted glass.
        ui.clear_children(self.lens);
        if let Some(tab) = tab {
            favicon_node(ui, self.lens, &tab.url, 18.0, favicons);
            ui.add_label(
                self.lens,
                &tab.label(),
                TextStyle::BodyEmphasis,
                Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Default::default()
                },
            );
        }
        ui.set_visible(self.plane, true);
        tracing::debug!(view = ?item.view, surface = ?item.surface, "tab drag started");
        true
    }

    /// Before layout, every frame: the drop place follows the pointer, the dragged tab's slot
    /// moves there, the lens follows on its spring.
    pub fn update(&mut self, ui: &mut Ui, chrome: &mut Chrome, config: &Config, dt: f32) {
        let Some(active) = &mut self.active else { return };
        let centre = active.pointer - active.grab + active.size / 2.0;
        let near = Frame {
            origin: active.area.origin - Vec2::splat(DROP_MARGIN),
            size: active.area.size + Vec2::splat(2.0 * DROP_MARGIN),
        }
        .contains(active.pointer);
        active.target = near.then(|| match active.item.surface {
            Surface::Sidebar => sidebar_target(&active.sidebar, active.item.view, centre.y),
            Surface::Bar => Target {
                private: active.item.private,
                index: bar_target(&active.bar, active.item.view, centre.x),
            },
        });
        // Between the groups the index counts the target group's own rows only.
        let preview = active.target.map(|t| (t.private, t.index));
        chrome.preview_drag(ui, active.item.view, active.item.surface, preview);

        active.lens.spring = spring(config);
        active.lens.target = active.pointer - active.grab;
        // Reduce Motion and flat mode: the tab follows the pointer plainly, without a spring.
        if config.glass.reduce_motion || config.glass.is_flat() {
            active.lens.snap(active.lens.target);
        }
        active.lens.step(dt);
        let speed = active.lens.velocity;
        let stretch = (speed.length() * STRETCH_PER_SPEED).min(config.glass.squash());
        let dir = if speed.length() > 0.0 { (speed / speed.length()).abs() } else { Vec2::ZERO };
        // Longer along the motion, a little narrower across it.
        let lift = if config.glass.is_flat() { 1.0 } else { LIFT };
        let scale = Vec2::splat(lift) * (Vec2::ONE + dir * stretch)
            / (Vec2::ONE + Vec2::new(dir.y, dir.x) * stretch * 0.5);
        let mut style = ui.style(self.lens);
        let origin = active.lens.value;
        style.inset = Rect { left: length(origin.x), right: auto(), top: length(origin.y), bottom: auto() };
        style.size = Size { width: length(active.size.x), height: length(active.size.y) };
        ui.set_style(self.lens, style);
        let size = active.size * scale;
        let visual = Frame { origin: origin + (active.size - size) / 2.0, size };
        ui.set_visual_frame(self.lens, Some(visual));
    }

    /// After layout, every frame: rows and segments are drawn where their springs are, on the
    /// way to where the layout put them. Only while dragging and until they settled after it.
    pub fn animate_rows(&mut self, ui: &mut Ui, chrome: &Chrome, config: &Config, dt: f32) {
        if self.active.is_none() && self.moving.is_empty() {
            return;
        }
        let spring = spring(config);
        let nodes = chrome.moving_nodes(ui);
        let mut settled = true;
        for (key, node) in &nodes {
            let frame = ui.frame(*node);
            let place = self.moving.entry(*key).or_insert_with(|| Animated::new(frame.origin, spring));
            place.spring = spring;
            place.target = frame.origin;
            if config.glass.reduce_motion {
                place.snap(frame.origin);
            }
            place.step(dt);
            if place.is_settled(SETTLED_PX) {
                place.snap(frame.origin);
                ui.set_visual_frame(*node, None);
            } else {
                settled = false;
                ui.set_visual_frame(*node, Some(Frame { origin: place.value, size: frame.size }));
            }
        }
        if self.active.is_none() && settled {
            self.moving.clear();
        }
    }

    /// The pointer went up during a drag: the lens goes, and the dropped tab flies from the
    /// lens into its new row (`landing`: its key after the drop, e.g. a reopened tab's).
    pub fn finish(&mut self, ui: &mut Ui) -> Option<Dropped> {
        self.pending = None;
        let active = self.active.take()?;
        ui.set_visible(self.plane, false);
        ui.set_visual_frame(self.lens, None);
        self.land(MoveKey::Tab(active.item.view, active.item.surface), active.lens.value);
        Some(Dropped { item: active.item, target: active.target })
    }

    /// Starts `key`'s row at `from` (logical px), to spring into its place.
    pub fn land(&mut self, key: MoveKey, from: Vec2) {
        let mut place = Animated::new(from, Spring::SNAPPY);
        place.value = from;
        self.moving.insert(key, place);
    }

    /// Where the lens is (logical px), while dragging.
    pub fn lens_origin(&self) -> Option<Vec2> {
        self.active.as_ref().map(|a| a.lens.value)
    }

    /// Drops the drag without moving anything (Escape, or the tab went away).
    pub fn cancel(&mut self, ui: &mut Ui) {
        self.pending = None;
        if self.active.take().is_some() {
            ui.set_visible(self.plane, false);
            ui.set_visual_frame(self.lens, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(views: &[u32], top: f32) -> Vec<(ViewId, Frame)> {
        views
            .iter()
            .enumerate()
            .map(|(i, &v)| (ViewId(v), Frame::new(0.0, top + i as f32 * 44.0, 200.0, 42.0)))
            .collect()
    }

    fn layout() -> SidebarLayout {
        SidebarLayout {
            normal: rows(&[1, 2, 3], 44.0),
            private_header: Frame::new(0.0, 176.0, 200.0, 42.0),
            private: rows(&[7, 8], 220.0),
            normal_open: true,
            private_open: true,
        }
    }

    #[test]
    fn index_counts_the_items_before() {
        assert_eq!(insertion_index(&[10.0, 30.0, 50.0], 0.0), 0);
        assert_eq!(insertion_index(&[10.0, 30.0, 50.0], 31.0), 2);
        assert_eq!(insertion_index(&[10.0, 30.0, 50.0], 99.0), 3);
    }

    #[test]
    fn reordering_within_the_normal_list() {
        let l = layout();
        // Row 1 (first) dragged below row 2's centre: it goes after it.
        assert_eq!(sidebar_target(&l, ViewId(1), 44.0 + 44.0 + 22.0), Target { private: false, index: 1 });
        // Above everything: the top.
        assert_eq!(sidebar_target(&l, ViewId(3), 10.0), Target { private: false, index: 0 });
        // Just above "Private": the normal list's end.
        assert_eq!(sidebar_target(&l, ViewId(1), 170.0), Target { private: false, index: 2 });
    }

    #[test]
    fn dropping_into_the_private_list() {
        let mut l = layout();
        // Onto "Private": the private tabs' start; between its rows: in between.
        assert_eq!(sidebar_target(&l, ViewId(2), 190.0), Target { private: true, index: 0 });
        assert_eq!(sidebar_target(&l, ViewId(2), 250.0), Target { private: true, index: 1 });
        // Locked (or folded) private tabs take it at their end.
        l.private_open = false;
        assert_eq!(sidebar_target(&l, ViewId(2), 250.0), Target { private: true, index: usize::MAX });
    }

    #[test]
    fn bar_segments_reorder_by_centre() {
        let items: Vec<(ViewId, Frame)> =
            (0..3).map(|i| (ViewId(i), Frame::new(i as f32 * 100.0, 0.0, 96.0, 36.0))).collect();
        assert_eq!(bar_target(&items, ViewId(0), 160.0), 1);
        assert_eq!(bar_target(&items, ViewId(2), 10.0), 0);
        assert_eq!(bar_target(&items, ViewId(1), 260.0), 2);
    }
}
