//! Glass sheets under popups.
//!
//! A popup (context menu, dropdown, confirm dialog: a widget with `Widget::popup`) is real
//! refractive glass that samples what is painted under it. Over a glass sheet (History,
//! Settings…) that is glass on glass: two blurs and two tints, so the popup loses its
//! refraction and reads as a flat grey card, and two live glass surfaces compete for attention.
//! So while a popup is open over a sheet (`Widget::popup_cover`), the sheet turns into a calm
//! solid card, on a spring, and back into glass when the popup closes.
//!
//! This module is the state: which sheets are covered, by how many popups, and how far each
//! sheet has turned (0 glass, 1 solid). Painting reads [`Covers::amount`]; the painter decides
//! what "turned" looks like (`glass.sheet_under_popup`).

use std::collections::HashMap;

use axion_physics::{Animated, Spring};
use taffy::NodeId;

use crate::Rect;

/// A shown popup: where it is painted in the tree and the screen area it takes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LivePopup {
    /// Child indices from the root: paint order is the lexicographic order of these (a node
    /// inside another one comes after it).
    pub path: Vec<usize>,
    /// Logical px.
    pub frame: Rect,
}

/// How many of `popups` are painted over `sheet` (a node at `path`, with `frame`) and overlap it.
/// Toasts and tooltips are no popups and never get here.
pub(crate) fn covering(path: &[usize], frame: Rect, popups: &[LivePopup]) -> usize {
    popups.iter().filter(|p| p.path.as_slice() > path && overlaps(p.frame, frame)).count()
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.origin.x < b.origin.x + b.size.x
        && b.origin.x < a.origin.x + a.size.x
        && a.origin.y < b.origin.y + b.size.y
        && b.origin.y < a.origin.y + a.size.y
}

/// The sheets that are (or are still turning back from being) covered.
#[derive(Default)]
pub(crate) struct Covers {
    nodes: HashMap<NodeId, Animated<f32>>,
}

/// Settled below this.
const SETTLED: f32 = 0.002;

impl Covers {
    /// How far `node` has turned into a solid card (0: glass).
    pub fn amount(&self, node: NodeId) -> f32 {
        self.nodes.get(&node).map_or(0.0, |a| a.value.clamp(0.0, 1.0))
    }

    /// Whether anything is still turning (the frame loop keeps going).
    pub fn is_moving(&self) -> bool {
        self.nodes.values().any(|a| !a.is_settled(SETTLED))
    }

    /// Sets where `node` is heading (1 covered, 0 glass).
    pub fn set_target(&mut self, node: NodeId, covered: bool, spring: Spring) {
        let target = if covered { 1.0 } else { 0.0 };
        match self.nodes.get_mut(&node) {
            Some(a) => {
                a.target = target;
                a.spring = spring;
            }
            None if covered => {
                let mut a = Animated::new(0.0, spring);
                a.target = target;
                self.nodes.insert(node, a);
            }
            None => {}
        }
    }

    /// A sheet that is gone (hidden or removed) forgets its state: it comes back as glass.
    pub fn forget(&mut self, node: NodeId) {
        self.nodes.remove(&node);
    }

    /// Nodes with a state.
    pub fn nodes(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    /// Advances every sheet; `snap` (Reduce Motion) jumps to the target instead.
    pub fn step(&mut self, dt: f32, snap: bool) {
        for a in self.nodes.values_mut() {
            if snap {
                a.snap(a.target);
            } else {
                a.step(dt);
            }
        }
        self.nodes.retain(|_, a| a.target > 0.0 || !a.is_settled(SETTLED));
    }

    #[cfg(test)]
    pub fn target(&self, node: NodeId) -> Option<f32> {
        self.nodes.get(&node).map(|a| a.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { origin: Vec2::new(x, y), size: Vec2::new(w, h) }
    }

    fn popup(path: &[usize], frame: Rect) -> LivePopup {
        LivePopup { path: path.to_vec(), frame }
    }

    const SHEET: Rect = Rect { origin: Vec2::new(100.0, 100.0), size: Vec2::new(600.0, 500.0) };

    #[test]
    fn a_popup_painted_after_the_sheet_and_over_it_covers_it() {
        let menu = popup(&[5, 2], rect(300.0, 200.0, 200.0, 150.0));
        assert_eq!(covering(&[3, 0], SHEET, &[menu]), 1);
    }

    #[test]
    fn a_popup_inside_the_sheet_covers_it_too() {
        // A dropdown that is a child of the sheet panel itself (painted after it).
        let dropdown = popup(&[3, 0, 4, 1], rect(300.0, 200.0, 200.0, 150.0));
        assert_eq!(covering(&[3, 0], SHEET, &[dropdown]), 1);
    }

    #[test]
    fn a_popup_painted_before_the_sheet_or_beside_it_does_not() {
        let under = popup(&[1, 0], rect(300.0, 200.0, 200.0, 150.0));
        let beside = popup(&[5, 0], rect(900.0, 200.0, 100.0, 100.0));
        assert_eq!(covering(&[3, 0], SHEET, &[under, beside]), 0);
    }

    #[test]
    fn covered_count_is_the_number_of_popups_over_it() {
        let a = popup(&[5, 0], rect(300.0, 200.0, 100.0, 100.0));
        let b = popup(&[6, 0], rect(320.0, 220.0, 100.0, 100.0));
        assert_eq!(covering(&[3, 0], SHEET, &[a.clone(), b]), 2);
        assert_eq!(covering(&[3, 0], SHEET, &[a]), 1);
        assert_eq!(covering(&[3, 0], SHEET, &[]), 0);
    }

    fn node(n: u64) -> NodeId {
        NodeId::new(n)
    }

    #[test]
    fn the_spring_heads_for_one_when_covered_and_back_to_zero_when_not() {
        let spring = Spring::new(0.15, 1.0);
        let mut covers = Covers::default();
        let sheet = node(1);
        covers.set_target(sheet, true, spring);
        assert_eq!(covers.target(sheet), Some(1.0));
        assert_eq!(covers.amount(sheet), 0.0);
        for _ in 0..8 {
            covers.step(1.0 / 60.0, false);
        }
        let mid = covers.amount(sheet);
        assert!(mid > 0.1 && mid < 1.0, "in motion after 8 frames: {mid}");
        for _ in 0..120 {
            covers.step(1.0 / 60.0, false);
        }
        assert!(covers.amount(sheet) > 0.99);
        covers.set_target(sheet, false, spring);
        assert_eq!(covers.target(sheet), Some(0.0));
        for _ in 0..180 {
            covers.step(1.0 / 60.0, false);
        }
        assert_eq!(covers.amount(sheet), 0.0);
        assert!(covers.nodes().is_empty(), "a settled glass sheet keeps no state");
        assert!(!covers.is_moving());
    }

    #[test]
    fn it_never_overshoots_and_reduce_motion_snaps() {
        let mut covers = Covers::default();
        let sheet = node(2);
        covers.set_target(sheet, true, Spring::new(0.15, 1.0));
        for _ in 0..90 {
            covers.step(1.0 / 60.0, false);
            assert!(covers.amount(sheet) <= 1.0);
        }
        let mut snapping = Covers::default();
        snapping.set_target(sheet, true, Spring::new(0.15, 1.0));
        snapping.step(1.0 / 60.0, true);
        assert_eq!(snapping.amount(sheet), 1.0);
    }

    #[test]
    fn an_uncovered_glass_sheet_gets_no_state() {
        let mut covers = Covers::default();
        covers.set_target(node(3), false, Spring::new(0.15, 1.0));
        assert!(covers.nodes().is_empty());
    }
}
