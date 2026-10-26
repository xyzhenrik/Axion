//! Pacing page resizes while the window is resized.
//!
//! Every size a page is given costs Chromium a layout and a new frame (of a new buffer size).
//! Resized at every step of a window resize, the shown page re-laid itself out faster than it
//! could deliver frames, and every page in the background did the same: a resize lagged. Now
//! the shown page gets one size at a time ([`ResizePacer`]: the next once the frame of the last
//! one came, or after a while), and the background tabs follow once the window has been still
//! for a moment ([`Debounce`]).

use std::time::{Duration, Instant};

use axion_engine::ViewSize;

/// Frames this many pixels off the size asked for count as of that size (Chromium rounds the
/// logical size before scaling it, see `ViewSize::physical`).
const SIZE_SLACK: u32 = 2;

/// One page resize at a time: a new size waits until the page's frame of the size it was
/// given last has arrived, or `wait` has passed.
#[derive(Debug, Default)]
pub struct ResizePacer {
    /// The size the page was given last and when, until its frame arrives.
    in_flight: Option<(ViewSize, Instant)>,
    /// The newest size, waiting for its turn.
    waiting: Option<ViewSize>,
}

impl ResizePacer {
    /// The page should get `size`: returns it when it should be sent now (`None`: it waits for
    /// [`ResizePacer::poll`]).
    pub fn request(&mut self, size: ViewSize, now: Instant, wait: Duration) -> Option<ViewSize> {
        if self.busy(now, wait) {
            self.waiting = Some(size);
            return None;
        }
        self.waiting = None;
        self.in_flight = Some((size, now));
        Some(size)
    }

    /// The page's current frame is `width` × `height` pixels.
    pub fn frame(&mut self, width: u32, height: u32) {
        if let Some((size, _)) = self.in_flight {
            let (w, h) = size.physical();
            if w.abs_diff(width) <= SIZE_SLACK && h.abs_diff(height) <= SIZE_SLACK {
                self.in_flight = None;
            }
        }
    }

    /// Once a frame: the waiting size, if its turn has come.
    pub fn poll(&mut self, now: Instant, wait: Duration) -> Option<ViewSize> {
        if self.busy(now, wait) {
            return None;
        }
        let size = self.waiting.take();
        self.in_flight = size.map(|s| (s, now));
        size
    }

    /// Forgets everything (the shown page changed).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn busy(&self, now: Instant, wait: Duration) -> bool {
        self.in_flight.is_some_and(|(_, since)| now.duration_since(since) < wait)
    }
}

/// Something to do once things have been still for a while.
#[derive(Debug, Default)]
pub struct Debounce {
    due: Option<Instant>,
}

impl Debounce {
    /// Things changed: it is due `delay` from now.
    pub fn touch(&mut self, now: Instant, delay: Duration) {
        self.due = Some(now + delay);
    }

    /// Whether it is due now (then it is done).
    pub fn take_due(&mut self, now: Instant) -> bool {
        self.due.take_if(|due| now >= *due).is_some()
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;

    const WAIT: Duration = Duration::from_millis(100);

    fn size(w: f32) -> ViewSize {
        ViewSize { logical: Vec2::new(w, 500.0), scale: 1.0 }
    }

    #[test]
    fn first_size_goes_at_once_later_ones_wait_for_its_frame() {
        let t = Instant::now();
        let mut p = ResizePacer::default();
        assert_eq!(p.request(size(800.0), t, WAIT), Some(size(800.0)));
        assert_eq!(p.request(size(810.0), t, WAIT), None);
        assert_eq!(p.request(size(820.0), t, WAIT), None);
        assert_eq!(p.poll(t, WAIT), None);
        // A frame of another size changes nothing.
        p.frame(700, 500);
        assert_eq!(p.poll(t, WAIT), None);
        // The frame of 800 came: only the newest size goes.
        p.frame(801, 500);
        assert_eq!(p.poll(t, WAIT), Some(size(820.0)));
        assert_eq!(p.poll(t, WAIT), None);
        p.frame(820, 500);
        assert_eq!(p.poll(t, WAIT), None);
        // Nothing in flight: a new size goes at once.
        assert_eq!(p.request(size(900.0), t, WAIT), Some(size(900.0)));
    }

    #[test]
    fn a_page_that_never_delivers_is_resized_after_the_wait() {
        let t = Instant::now();
        let mut p = ResizePacer::default();
        assert!(p.request(size(800.0), t, WAIT).is_some());
        assert_eq!(p.request(size(900.0), t + WAIT / 2, WAIT), None);
        assert_eq!(p.poll(t + WAIT / 2, WAIT), None);
        assert_eq!(p.poll(t + WAIT, WAIT), Some(size(900.0)));
        // After the wait a request goes straight through.
        assert_eq!(p.request(size(950.0), t + WAIT * 3, WAIT), Some(size(950.0)));
    }

    #[test]
    fn no_wait_sends_every_size() {
        let t = Instant::now();
        let mut p = ResizePacer::default();
        for w in [800.0, 810.0, 820.0] {
            assert_eq!(p.request(size(w), t, Duration::ZERO), Some(size(w)));
        }
    }

    #[test]
    fn debounce_fires_once_after_the_last_touch() {
        let t = Instant::now();
        let delay = Duration::from_millis(300);
        let mut d = Debounce::default();
        assert!(!d.take_due(t));
        d.touch(t, delay);
        d.touch(t + delay / 2, delay);
        assert!(!d.take_due(t + delay));
        assert!(d.take_due(t + delay / 2 + delay));
        assert!(!d.take_due(t + delay * 10));
    }
}
