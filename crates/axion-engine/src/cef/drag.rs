//! Drag and drop for off-screen views.
//!
//! Off-screen Chromium has no window for the desktop's drag and drop, so both ends run here:
//! a drag the page starts (selected text, a link, an image, a `draggable` element) is driven
//! by the view's own mouse input and dropped back into the same view, and files dragged in
//! from the desktop arrive through `Engine::drag_*`.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use cef::sys::{cef_drag_operations_mask_t, cef_event_flags_t};
use cef::*;
use glam::Vec2;

use super::input::InputState;
use crate::{CursorKind, InputEvent, Modifiers, MouseButton};

/// What a drag from outside may do: files from a file manager can be copied, moved or linked.
const EXTERNAL_OPERATIONS: u32 = cef_drag_operations_mask_t::DRAG_OPERATION_COPY.0
    | cef_drag_operations_mask_t::DRAG_OPERATION_LINK.0
    | cef_drag_operations_mask_t::DRAG_OPERATION_MOVE.0;
const NONE: u32 = cef_drag_operations_mask_t::DRAG_OPERATION_NONE.0;
/// How long a drop from outside waits for the page to answer the drag (see `external_drop`).
const ANSWER_TIMEOUT: Duration = Duration::from_millis(500);
/// Windows virtual-key code of Escape.
const ESCAPE: i32 = 0x1B;

/// A drag the page started, running inside the view.
struct PageDrag {
    data: DragData,
    allowed: u32,
    /// Whether the pointer is over the view (the drag target is entered).
    inside: bool,
}

#[derive(Default)]
pub(super) struct ViewDrag {
    page: RefCell<Option<PageDrag>>,
    /// What the page's drop target under the pointer would do (from `update_drag_cursor`).
    operation: Cell<u32>,
    /// Files from outside are over the view.
    external: Cell<bool>,
    /// The page answered their arrival (see `external_drop`).
    answered: Cell<bool>,
    /// Their drop, waiting for that answer (and since when).
    pending_drop: RefCell<Option<(MouseEvent, Instant)>>,
}

fn mask(bits: u32) -> DragOperationsMask {
    cef_drag_operations_mask_t(bits).into()
}

/// A mouse event at `pos` with the left button held (it is, during a drag).
fn held(input: &InputState, pos: Vec2, modifiers: Modifiers) -> MouseEvent {
    let mut event = input.mouse(pos.x, pos.y, modifiers);
    event.modifiers |= cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON.0;
    event
}

impl ViewDrag {
    /// The page started dragging something (`start_dragging`): runs the drag inside the view.
    /// Returns `false` if it can't (Chromium then cancels the drag).
    pub fn start(
        &self,
        host: &BrowserHost,
        data: &DragData,
        allowed: DragOperationsMask,
        x: i32,
        y: i32,
    ) -> bool {
        // A copy we may change: the target must not get file contents (see
        // `CefBrowserHost::DragTargetDragEnter`).
        let Some(mut data) = ImplDragData::clone(data) else { return false };
        data.reset_file_contents();
        let allowed = allowed.as_ref().0;
        let event = MouseEvent { x, y, modifiers: cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON.0 };
        host.drag_target_drag_enter(Some(&mut data), Some(&event), mask(allowed));
        self.operation.set(NONE);
        self.page.replace(Some(PageDrag { data, allowed, inside: true }));
        tracing::debug!(allowed, "page drag started");
        true
    }

    /// The drop target under the pointer changed what it would do (`update_drag_cursor`);
    /// returns the cursor for it.
    pub fn set_operation(&self, host: Option<&BrowserHost>, operation: DragOperationsMask) -> CursorKind {
        let op = operation.as_ref().0;
        self.operation.set(op);
        self.answer(host);
        if op == NONE {
            CursorKind::NotAllowed
        } else if op & cef_drag_operations_mask_t::DRAG_OPERATION_COPY.0 != 0 {
            CursorKind::Copy
        } else if op & cef_drag_operations_mask_t::DRAG_OPERATION_LINK.0 != 0 {
            CursorKind::Alias
        } else {
            CursorKind::Grabbing
        }
    }

    /// Feeds the view's input to a running page drag. Returns `true` if the drag took it.
    pub fn page_input(&self, host: &BrowserHost, input: &mut InputState, event: &InputEvent) -> bool {
        // Taken out while Chromium is called, in case it calls back into the view.
        let Some(mut drag) = self.page.take() else { return false };
        let mut running = true;
        match event {
            InputEvent::MouseMove { pos, modifiers } => {
                let event = held(input, *pos, *modifiers);
                if drag.inside {
                    host.drag_target_drag_over(Some(&event), mask(drag.allowed));
                } else {
                    host.drag_target_drag_enter(Some(&mut drag.data), Some(&event), mask(drag.allowed));
                    drag.inside = true;
                }
            }
            InputEvent::MouseLeave { .. } => {
                if drag.inside {
                    host.drag_target_drag_leave();
                    drag.inside = false;
                    self.operation.set(NONE);
                }
            }
            InputEvent::MouseButton { pos, button: MouseButton::Left, pressed: false, modifiers, .. } => {
                input.release(MouseButton::Left);
                let event = input.mouse(pos.x, pos.y, *modifiers);
                let operation = if drag.inside {
                    host.drag_target_drop(Some(&event));
                    self.operation.get()
                } else {
                    NONE
                };
                host.drag_source_ended_at(event.x, event.y, mask(operation));
                host.drag_source_system_drag_ended();
                tracing::debug!(operation, "page drag ended");
                running = false;
            }
            // Escape cancels, as in any browser.
            InputEvent::Key { pressed: true, key_code: ESCAPE, .. } => {
                if drag.inside {
                    host.drag_target_drag_leave();
                }
                host.drag_source_ended_at(0, 0, mask(NONE));
                host.drag_source_system_drag_ended();
                running = false;
            }
            // Other buttons, the wheel and keys don't end a drag; the page gets nothing else.
            _ => {}
        }
        if running {
            self.page.replace(Some(drag));
        }
        true
    }

    /// Ends any drag without a drop (the view is being hidden).
    pub fn cancel(&self, host: &BrowserHost) {
        if let Some(drag) = self.page.take() {
            if drag.inside {
                host.drag_target_drag_leave();
            }
            host.drag_source_ended_at(0, 0, mask(NONE));
            host.drag_source_system_drag_ended();
        }
        self.external_leave(host);
    }

    fn is_dragging_page(&self) -> bool {
        self.page.borrow().is_some()
    }

    pub fn external_enter(
        &self,
        host: &BrowserHost,
        input: &InputState,
        files: &[std::path::PathBuf],
        pos: Vec2,
        modifiers: Modifiers,
    ) {
        if self.is_dragging_page() {
            return;
        }
        let Some(mut data) = drag_data_create() else { return };
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            data.add_file(Some(&path.to_string_lossy().as_ref().into()), Some(&name.as_str().into()));
        }
        self.external_leave(host);
        self.external.set(true);
        self.answered.set(false);
        let event = held(input, pos, modifiers);
        host.drag_target_drag_enter(Some(&mut data), Some(&event), mask(EXTERNAL_OPERATIONS));
    }

    pub fn external_over(&self, host: &BrowserHost, input: &InputState, pos: Vec2, modifiers: Modifiers) {
        if self.external.get() && self.pending_drop.borrow().is_none() {
            let event = held(input, pos, modifiers);
            host.drag_target_drag_over(Some(&event), mask(EXTERNAL_OPERATIONS));
        }
    }

    pub fn external_leave(&self, host: &BrowserHost) {
        self.pending_drop.take();
        if self.external.replace(false) {
            host.drag_target_drag_leave();
        }
    }

    /// Drops what is dragged in. Until the page has answered the drag's arrival (its
    /// `dragenter` / `dragover` handlers decide whether it takes the drop), the drop waits:
    /// dropped before, Chromium treats it as unwanted and opens the file instead.
    pub fn external_drop(&self, host: &BrowserHost, input: &InputState, pos: Vec2, modifiers: Modifiers) {
        if !self.external.get() || self.pending_drop.borrow().is_some() {
            return;
        }
        host.drag_target_drag_over(Some(&held(input, pos, modifiers)), mask(EXTERNAL_OPERATIONS));
        let event = input.mouse(pos.x, pos.y, modifiers);
        if self.answered.get() {
            self.external.set(false);
            host.drag_target_drop(Some(&event));
        } else {
            self.pending_drop.replace(Some((event, Instant::now())));
        }
    }

    /// The page answered the drag over it: a waiting drop goes in now.
    fn answer(&self, host: Option<&BrowserHost>) {
        if !self.external.get() {
            return;
        }
        self.answered.set(true);
        let Some(host) = host else { return };
        if let Some((event, _)) = self.pending_drop.take() {
            self.external.set(false);
            host.drag_target_drop(Some(&event));
        }
    }

    /// A drop waits for the page's answer (see [`ViewDrag::flush`]).
    pub fn is_waiting(&self) -> bool {
        self.pending_drop.borrow().is_some()
    }

    /// Drops a waiting drop whose answer never came (call regularly).
    pub fn flush(&self, host: &BrowserHost) {
        let overdue =
            self.pending_drop.borrow().as_ref().is_some_and(|(_, at)| at.elapsed() >= ANSWER_TIMEOUT);
        if overdue && let Some((event, _)) = self.pending_drop.take() {
            tracing::debug!("the page didn't answer a drag in time; dropping anyway");
            self.external.set(false);
            host.drag_target_drop(Some(&event));
        }
    }
}
