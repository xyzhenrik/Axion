//! Holds the newest GPU frame back for a moment before it is copied.
//!
//! Chromium reports a shared frame once its drawing is *submitted*, and the driver (NVIDIA)
//! attaches no fence to the dma-buf, so a copy made at once can still read the buffer's
//! previous content: letters just typed vanish for a frame, or until the page paints again
//! (a caret blink later). Copying a few milliseconds later is what fixes it (see
//! `tabs.gpu_frame_settle_ms`).

use std::time::{Duration, Instant};

/// A frame held back is copied at the latest this many settling times after the first frame
/// that started the hold, however many newer ones replaced it. Without a limit, a page that
/// paints more often than the settling time (240 Hz: every 4.2 ms, against 6 ms) replaced its
/// held frame before it was ever due, and no frame was copied at all: the page froze, or
/// showed a frame only when Chromium happened to pause.
const MAX_HOLD: u32 = 2;

/// The newest frame, waiting for its copy to be due. A newer frame replaces it.
#[derive(Debug)]
pub(super) struct Settle<T> {
    held: Option<Held<T>>,
}

#[derive(Debug)]
struct Held<T> {
    frame: T,
    /// When it is copied.
    due: Instant,
    /// The latest a frame of this hold may be copied (see `MAX_HOLD`).
    deadline: Instant,
}

impl<T> Default for Settle<T> {
    fn default() -> Self {
        Self { held: None }
    }
}

impl<T> Settle<T> {
    /// Hands over a frame that arrived at `now`. With no settling time it comes straight back
    /// to be copied; otherwise it is held (and replaces the one held before, keeping that
    /// one's deadline).
    pub fn offer(&mut self, frame: T, now: Instant, settle: Duration) -> Option<T> {
        if settle.is_zero() {
            self.held = None;
            return Some(frame);
        }
        let deadline = self.held.as_ref().map_or(now + settle * MAX_HOLD, |h| h.deadline);
        self.held = Some(Held { frame, due: (now + settle).min(deadline), deadline });
        None
    }

    /// The held frame, once `now` is past its due time.
    pub fn take_due(&mut self, now: Instant) -> Option<T> {
        if self.held.as_ref().is_some_and(|h| now >= h.due) {
            return self.held.take().map(|h| h.frame);
        }
        None
    }

    /// When the held frame is due.
    pub fn due(&self) -> Option<Instant> {
        self.held.as_ref().map(|h| h.due)
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

    #[test]
    fn frames_faster_than_the_settling_time_still_get_copied() {
        // 240 Hz: a frame every 4.2 ms, each one would push the copy 6 ms out again.
        let (mut gate, t0) = (Settle::default(), Instant::now());
        let interval = Duration::from_micros(4167);
        let mut copied = Vec::new();
        for i in 0..240u32 {
            let now = t0 + interval * i;
            if let Some(frame) = gate.take_due(now) {
                copied.push((frame, i));
            }
            gate.offer(i, now, SETTLE);
        }
        // About one copy per two settling times (12 ms): ~80 of 240 frames in a second.
        assert!(copied.len() >= 70, "only {} frames copied", copied.len());
        for pair in copied.windows(2) {
            let gap = interval * (pair[1].1 - pair[0].1);
            assert!(gap <= SETTLE * MAX_HOLD + interval, "a gap of {gap:?}");
        }
        // The copy is always of the newest frame that had arrived.
        for (frame, at) in &copied {
            assert_eq!(*frame, at - 1);
        }
    }

    #[test]
    fn a_new_hold_starts_after_a_copy() {
        let (mut gate, t0) = (Settle::default(), Instant::now());
        gate.offer("a", t0, SETTLE);
        assert_eq!(gate.take_due(t0 + SETTLE), Some("a"));
        // The next frame gets the whole settling time again.
        let t1 = t0 + SETTLE + Duration::from_millis(1);
        gate.offer("b", t1, SETTLE);
        assert_eq!(gate.due(), Some(t1 + SETTLE));
    }
}
