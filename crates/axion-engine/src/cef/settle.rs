//! Holds the newest GPU frame back for a moment before it is copied.
//!
//! Chromium reports a shared frame once its drawing is *submitted*, and the driver (NVIDIA)
//! attaches no fence to the dma-buf, so a copy made at once can still read the buffer's
//! previous content: letters just typed vanish for a frame, or until the page paints again
//! (a caret blink later). Copying a few milliseconds later is what fixes it (see
//! `tabs.gpu_frame_settle_ms`).

use std::time::{Duration, Instant};

/// The newest frame, waiting for its copy to be due. A newer frame replaces it.
#[derive(Debug)]
pub(super) struct Settle<T> {
    held: Option<(T, Instant)>,
}

impl<T> Default for Settle<T> {
    fn default() -> Self {
        Self { held: None }
    }
}

impl<T> Settle<T> {
    /// Hands over a frame that arrived at `now`. With no settling time it comes straight back
    /// to be copied; otherwise it is held (and replaces the one held before).
    pub fn offer(&mut self, frame: T, now: Instant, settle: Duration) -> Option<T> {
        if settle.is_zero() {
            self.held = None;
            return Some(frame);
        }
        self.held = Some((frame, now + settle));
        None
    }

    /// The held frame, once `now` is past its due time.
    pub fn take_due(&mut self, now: Instant) -> Option<T> {
        if self.held.as_ref().is_some_and(|(_, due)| now >= *due) {
            return self.held.take().map(|(frame, _)| frame);
        }
        None
    }

    /// When the held frame is due.
    pub fn due(&self) -> Option<Instant> {
        self.held.as_ref().map(|(_, due)| *due)
    }

    pub fn clear(&mut self) {
        self.held = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTLE: Duration = Duration::from_millis(6);

    #[test]
    fn without_settling_time_frames_pass_straight_through() {
        let mut gate = Settle::default();
        assert_eq!(gate.offer(1, Instant::now(), Duration::ZERO), Some(1));
        assert_eq!(gate.due(), None);
    }

    #[test]
    fn a_frame_is_copied_only_once_it_has_settled() {
        let (mut gate, t0) = (Settle::default(), Instant::now());
        assert_eq!(gate.offer("a", t0, SETTLE), None);
        assert_eq!(gate.due(), Some(t0 + SETTLE));
        assert_eq!(gate.take_due(t0 + Duration::from_millis(5)), None);
        assert_eq!(gate.take_due(t0 + SETTLE), Some("a"));
        assert_eq!(gate.take_due(t0 + SETTLE), None, "taken once");
    }

    #[test]
    fn a_newer_frame_replaces_the_one_held() {
        let (mut gate, t0) = (Settle::default(), Instant::now());
        gate.offer("old", t0, SETTLE);
        gate.offer("new", t0 + Duration::from_millis(3), SETTLE);
        // The older frame's due time passes without it being copied: the newer one is held.
        assert_eq!(gate.take_due(t0 + SETTLE), None);
        assert_eq!(gate.take_due(t0 + Duration::from_millis(9)), Some("new"));
    }
}
