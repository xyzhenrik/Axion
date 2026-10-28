//! Dragging tabs: in the sidebar's lists (also from the normal tabs into the private ones and
//! back) and in the toolbar's tabs. A press on a tab becomes a drag once the pointer moved
//! `tabs.drag_threshold`; shorter moves stay clicks.
//!
//! While dragging, the tab follows the pointer as a lifted glass lens (on a spring, stretched
//! along its motion), its own row stays as an empty slot at the place it would drop, and the
//! other rows slide aside on springs (each row's drawn position follows its laid-out one).

use axion_compositor::Rect as Frame;
use axion_config::Config;
use axion_engine::ViewId;
use axion_physics::{Animated, Spring};
use axion_ui::taffy::prelude::*;
use axion_ui::{Background, CAPSULE, GlassLevel, TextStyle, Ui, Widget};
use glam::Vec2;

#[cfg(test)]
use crate::chrome::SidebarHeader;
use crate::chrome::{Chrome, MoveKey, SidebarLayout, SidebarRow, Surface, TabItem};
use crate::favicons::{Favicons, favicon_node};
use crate::tabclose::{Slide, slide_spring};
use crate::tabs::Tab;

/// The lens grows this much when lifted.
const LIFT: f32 = 1.06;
/// Stretch per logical px/s of the lens's speed (up to `glass.squash`).
const STRETCH_PER_SPEED: f32 = 0.00008;
/// How far outside the sidebar (or the bar's band) a drop still counts, logical px.
const DROP_MARGIN: f32 = 48.0;

/// Where a dragged tab would go: its group (normal or private), its index there (among the
/// list's other tabs; `usize::MAX` for the end) and its tab group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub private: bool,
    pub index: usize,
    /// The tab group it joins (`None`: it is ungrouped there).
    pub tab_group: Option<u32>,
    /// Dropped onto the tab group's header (it becomes the group's first tab).
    pub on_header: bool,
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
    /// The bar's tabs when the drag started.
    bar: Vec<SidebarRow>,
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
    moving: Slide<MoveKey>,
}

/// A row's index among the list's other tabs once the dragged tab (at `source`, when it is
/// in the list) is out of it.
fn rank(row: &SidebarRow, source: Option<usize>) -> usize {
    if source.is_some_and(|s| s < row.index) { row.index - 1 } else { row.index }
}

/// The index and tab group of a drop before the `k`th of `rows`, the list's rows without the
/// dragged one. `stays` says whether a drop right behind the last tab of a group's run keeps
/// the tab in that group.
fn drop_among(
    rows: &[&SidebarRow],
    source: Option<usize>,
    k: usize,
    stays: impl Fn(&SidebarRow) -> bool,
) -> (usize, Option<u32>) {
    let index = match rows.get(k) {
        Some(next) => rank(next, source),
        None => rows.last().map_or(0, |last| rank(last, source) + 1),
    };
    let prev = k.checked_sub(1).and_then(|i| rows.get(i));
    let next = rows.get(k);
    let group = match (prev.and_then(|p| p.group), next.and_then(|n| n.group)) {
        (Some(a), Some(b)) if a == b => Some(a),
        (Some(a), _) if prev.is_some_and(|p| stays(p)) => Some(a),
        _ => None,
    };
    (index, group)
}

/// The sidebar target for a dragged row whose centre is at `y`: the normal list above
/// "Private", the private list from it on (onto "Private" itself: the private tabs' start).
/// A folded or locked list takes the tab at its end. In the normal list, a tab group's
/// header takes the tab as the group's first one, and rows in the middle of a group (or
/// over the last row of one) put it into the group.
pub fn sidebar_target(layout: &SidebarLayout, source: ViewId, y: f32) -> Target {
    let private = y >= layout.private_header.origin.y;
    let (open, rows) =
        if private { (layout.private_open, &layout.private) } else { (layout.normal_open, &layout.normal) };
    let plain = |index| Target { private, index, tab_group: None, on_header: false };
    if !open {
        return plain(usize::MAX);
    }
    let source_index = rows.iter().find(|r| r.view == source).map(|r| r.index);
    if !private
        && let Some(h) =
            layout.headers.iter().find(|h| y >= h.frame.origin.y && y < h.frame.origin.y + h.frame.size.y)
    {
        let index = if source_index.is_some_and(|s| s < h.first) { h.first - 1 } else { h.first };
        return Target { private, index, tab_group: Some(h.group), on_header: true };
    }
    let others: Vec<&SidebarRow> = rows.iter().filter(|r| r.view != source).collect();
    let k = others.iter().filter(|r| r.frame.center().y < y).count();
    if private {
        return plain(drop_among(&others, source_index, k, |_| false).0);
    }
    // Over the lower half of a group's last row the tab stays in the group; further down it
    // leaves.
    let (index, tab_group) = drop_among(&others, source_index, k, |p| y <= p.frame.origin.y + p.frame.size.y);
    Target { private, index, tab_group, on_header: false }
}

/// The bar target for a dragged segment whose centre is at `x`: its index among the other
/// tabs and its tab group. `group` is the dragged tab's own: reordering inside it keeps it
/// there.
pub fn bar_target(items: &[SidebarRow], source: ViewId, group: Option<u32>, x: f32) -> (usize, Option<u32>) {
    let source_index = items.iter().find(|r| r.view == source).map(|r| r.index);
    let others: Vec<&SidebarRow> = items.iter().filter(|r| r.view != source).collect();
    let k = others.iter().filter(|r| r.frame.center().x < x).count();
    drop_among(&others, source_index, k, |p| group.is_some() && p.group == group)
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
        Self { pending: None, active: None, plane, lens, moving: Slide::default() }
    }

    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }

    /// The tab being dragged, while dragging.
    pub fn dragged(&self) -> Option<TabItem> {
        self.active.as_ref().map(|a| a.item)
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
        // The tabs were rebuilt since the press (their nodes are gone): there is nothing to drag.
        if ui.widget(item.node).is_none() {
            return false;
        }
        let frame = ui.frame(item.node);
        let area = match item.surface {
            Surface::Sidebar => ui.frame(chrome.sidebar_node()),
            Surface::Bar => ui.parent(item.node).map(|n| ui.frame(n)).unwrap_or(frame),
        };
        let bar = chrome
            .tab_items(Surface::Bar)
            .iter()
            .map(|i| SidebarRow { view: i.view, frame: ui.frame(i.node), index: i.index, group: i.group })
            .collect();
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
            Surface::Bar => {
                let (index, tab_group) =
                    bar_target(&active.bar, active.item.view, active.item.group, centre.x);
                Target { private: active.item.private, index, tab_group, on_header: false }
            }
        });
        // Between the groups the index counts the target group's own rows only.
        let preview =
            active.target.map(|t| (t.private, t.index, t.on_header.then_some(t.tab_group).flatten()));
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
    /// way to where the layout put them. While dragging, and until they settled after it or
    /// after a tab was closed (see `remember_rows`).
    pub fn animate_rows(&mut self, ui: &mut Ui, chrome: &Chrome, config: &Config, dt: f32) {
        if self.active.is_none() && self.moving.is_idle() {
            return;
        }
        let nodes = chrome.moving_nodes(ui);
        let settled = self.moving.animate(ui, &nodes, spring(config), config.glass.reduce_motion, dt);
        if self.active.is_none() && settled {
            self.moving.clear();
        }
    }

    /// Before the chrome rebuilds its tabs for a change of their layout (a tab closed): notes
    /// where every row and segment is drawn now, so they slide to their new places instead of
    /// jumping.
    pub fn remember_rows(&mut self, ui: &Ui, chrome: &Chrome, config: &Config) {
        let items = chrome.moving_nodes(ui).into_iter().map(|(key, node)| {
            let drawn = ui.widget(node).and_then(|w| w.visual_frame).unwrap_or_else(|| ui.frame(node));
            (key, drawn.origin)
        });
        self.moving.remember(items, slide_spring(config));
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
        self.moving.start(key, from, Spring::SNAPPY);
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

    fn rows(views: &[u32], top: f32) -> Vec<SidebarRow> {
        views
            .iter()
            .enumerate()
            .map(|(i, &v)| SidebarRow {
                view: ViewId(v),
                frame: Frame::new(0.0, top + i as f32 * 44.0, 200.0, 42.0),
                index: i,
                group: None,
            })
            .collect()
    }

    fn layout() -> SidebarLayout {
        SidebarLayout {
            normal: rows(&[1, 2, 3], 44.0),
            private_header: Frame::new(0.0, 176.0, 200.0, 42.0),
            private: rows(&[7, 8], 220.0),
            headers: Vec::new(),
            normal_open: true,
            private_open: true,
        }
    }

    fn at(l: &SidebarLayout, source: u32, y: f32) -> (bool, usize, Option<u32>, bool) {
        let t = sidebar_target(l, ViewId(source), y);
        (t.private, t.index, t.tab_group, t.on_header)
    }

    #[test]
    fn reordering_within_the_normal_list() {
        let l = layout();
        // Row 1 (first) dragged below row 2's centre: it goes after it.
        assert_eq!(at(&l, 1, 44.0 + 44.0 + 22.0), (false, 1, None, false));
        // Above everything: the top.
        assert_eq!(at(&l, 3, 10.0), (false, 0, None, false));
        // Just above "Private": the normal list's end.
        assert_eq!(at(&l, 1, 170.0), (false, 2, None, false));
    }

    #[test]
    fn dropping_into_the_private_list() {
        let mut l = layout();
        // Onto "Private": the private tabs' start; between its rows: in between.
        assert_eq!(at(&l, 2, 190.0), (true, 0, None, false));
        assert_eq!(at(&l, 2, 250.0), (true, 1, None, false));
        // Locked (or folded) private tabs take it at their end.
        l.private_open = false;
        assert_eq!(at(&l, 2, 250.0), (true, usize::MAX, None, false));
    }

    /// Tabs 1 and 2 ungrouped, 3 and 4 in group 9 (header between 2 and 3), 5 ungrouped.
    fn grouped() -> SidebarLayout {
        let mut l = layout();
        l.normal = rows(&[1, 2, 3, 4, 5], 44.0);
        l.normal[2].group = Some(9);
        l.normal[3].group = Some(9);
        // The header takes a place between 2 and 3.
        for r in l.normal.iter_mut().skip(2) {
            r.frame.origin.y += 34.0;
        }
        l.headers =
            vec![SidebarHeader { group: 9, frame: Frame::new(0.0, 44.0 + 88.0, 200.0, 34.0), first: 2 }];
        l.private_header = Frame::new(0.0, 400.0, 200.0, 42.0);
        l
    }

    #[test]
    fn tab_groups_take_drops_on_their_header_and_inside() {
        let l = grouped();
        // Onto the header: the group's first place.
        assert_eq!(at(&l, 1, 44.0 + 88.0 + 10.0), (false, 1, Some(9), true));
        assert_eq!(at(&l, 5, 44.0 + 88.0 + 10.0), (false, 2, Some(9), true));
        // Between the group's two rows: inside it.
        let between = l.normal[2].frame.center().y + 1.0;
        assert_eq!(at(&l, 1, between), (false, 2, Some(9), false));
        // Over the lower half of its last row: still in the group, at its end.
        let last = l.normal[3].frame;
        assert_eq!(at(&l, 1, last.center().y + 5.0), (false, 3, Some(9), false));
        // Below that row: out of it.
        assert_eq!(at(&l, 3, last.origin.y + last.size.y + 2.0), (false, 3, None, false));
        // Between ungrouped rows: ungrouped.
        assert_eq!(at(&l, 5, 100.0), (false, 1, None, false));
    }

    #[test]
    fn folded_tabs_do_not_shift_the_index() {
        // Tab 3 of group 9 is folded away: its row is missing, the others keep their indices.
        let mut l = grouped();
        l.normal.remove(2);
        let last = l.normal[2].frame;
        assert_eq!(at(&l, 1, last.origin.y + last.size.y + 2.0), (false, 3, None, false));
    }

    fn bar(groups: [Option<u32>; 4]) -> Vec<SidebarRow> {
        (0..4)
            .map(|i| SidebarRow {
                view: ViewId(i),
                frame: Frame::new(i as f32 * 100.0, 0.0, 96.0, 36.0),
                index: i as usize,
                group: groups[i as usize],
            })
            .collect()
    }

    #[test]
    fn bar_segments_reorder_by_centre() {
        let items = bar([None; 4]);
        assert_eq!(bar_target(&items, ViewId(0), None, 160.0).0, 1);
        assert_eq!(bar_target(&items, ViewId(2), None, 10.0).0, 0);
        assert_eq!(bar_target(&items, ViewId(1), None, 260.0).0, 2);
    }

    #[test]
    fn bar_drops_between_group_tabs_join_the_group() {
        let items = bar([None, Some(5), Some(5), None]);
        // Tab 3 between the group's two tabs.
        assert_eq!(bar_target(&items, ViewId(3), None, 150.0), (2, Some(5)));
        // A group tab reordered past its group's end stays in it.
        assert_eq!(bar_target(&items, ViewId(1), Some(5), 260.0), (2, Some(5)));
        // An ungrouped tab behind the group stays out.
        assert_eq!(bar_target(&items, ViewId(0), None, 300.0), (2, None));
    }
}
