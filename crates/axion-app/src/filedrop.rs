//! Files dragged onto the window from the desktop or a file manager.
//!
//! winit reports a drag as a burst of `HoveredFile` events (one per file) when it enters,
//! `HoveredFileCancelled` when it leaves, and `DroppedFile` events on the drop, without a
//! position. The browser collects them here and, every frame, tells [`FileDrop::update`]
//! where the pointer is; the page under it gets the drag like in Chrome (drop zones light
//! up, `<input type=file>` takes the files, an image dropped on a page opens it).

use std::path::PathBuf;

use axion_engine::{DropData, Engine, Modifiers, ViewId};
use glam::Vec2;

/// What a drop outside the page asks of the browser.
#[derive(Debug, PartialEq, Eq)]
pub enum DropOutcome {
    /// Dropped on the chrome (bar, sidebar): open the files in new tabs.
    OpenTabs(Vec<PathBuf>),
}

#[derive(Default)]
pub struct FileDrop {
    files: Vec<PathBuf>,
    /// A drag is over the window.
    hovering: bool,
    dropped: bool,
    cancelled: bool,
    /// The page the drag is over, and where it was last reported (view coordinates).
    over: Option<(ViewId, Vec2)>,
}

impl FileDrop {
    pub fn hovered(&mut self, path: PathBuf) {
        if !self.hovering || self.cancelled {
            self.files.clear();
            self.cancelled = false;
        }
        self.hovering = true;
        if !self.files.contains(&path) {
            self.files.push(path);
        }
    }

    pub fn dropped(&mut self, path: PathBuf) {
        self.hovering = true;
        self.dropped = true;
        if !self.files.contains(&path) {
            self.files.push(path);
        }
    }

    pub fn cancelled(&mut self) {
        self.cancelled = true;
    }

    /// The files being dragged.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// A drag is over the window (the browser should look for the pointer).
    pub fn is_active(&self) -> bool {
        self.hovering || self.cancelled
    }

    /// Moves the drag to `target` (the page under the pointer and the position in it; `None`
    /// over the chrome or outside) and finishes a drop or a cancel.
    pub fn update(
        &mut self,
        engine: &mut impl Engine,
        target: Option<(ViewId, Vec2)>,
        modifiers: Modifiers,
    ) -> Option<DropOutcome> {
        if self.cancelled {
            if let Some((view, _)) = self.over.take() {
                engine.drag_leave(view);
            }
            *self = Self::default();
            return None;
        }
        if !self.hovering {
            return None;
        }
        match (self.over, target) {
            (Some((view, last)), Some((now, pos))) if view == now => {
                if last != pos {
                    engine.drag_over(view, pos, modifiers);
                    self.over = Some((view, pos));
                }
            }
            (over, target) => {
                if let Some((view, _)) = over {
                    engine.drag_leave(view);
                }
                self.over = target;
                if let Some((view, pos)) = target {
                    engine.drag_enter(view, &DropData { files: self.files.clone() }, pos, modifiers);
                }
            }
        }
        if !self.dropped {
            return None;
        }
        let outcome = match self.over.take() {
            Some((view, pos)) => {
                engine.drag_drop(view, pos, modifiers);
                None
            }
            None => Some(DropOutcome::OpenTabs(std::mem::take(&mut self.files))),
        };
        *self = Self::default();
        outcome
    }
}
