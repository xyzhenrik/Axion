//! Frame-delivery timings, logged at debug level every few seconds
//! (`RUST_LOG=axion_engine=debug`): how long Chromium's paint callbacks and the message-loop
//! pump hold the main thread, and how long input takes to show in a page frame.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// How often the timings are logged (and reset).
const LOG_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Span {
    count: u32,
    total: Duration,
    worst: Duration,
}

impl Span {
    fn add(&mut self, took: Duration) {
        self.count += 1;
        self.total += took;
        self.worst = self.worst.max(took);
    }

    fn avg_ms(&self) -> f32 {
        self.total.as_secs_f32() * 1000.0 / self.count.max(1) as f32
    }

    fn worst_ms(&self) -> f32 {
        self.worst.as_secs_f32() * 1000.0
    }
}

#[derive(Default)]
struct Window {
    since: Option<Instant>,
    paint: Span,
    pump: Span,
    input: Span,
    /// Frames that were copied again because they weren't drawn yet.
    late: u32,
    /// Frames given up on (never drawn, or scaled).
    dropped: u32,
}

#[derive(Default)]
pub(super) struct Stats {
    window: RefCell<Window>,
    /// When input was sent that no page frame has shown yet.
    pending_input: Cell<Option<Instant>>,
}

impl Stats {
    pub fn paint(&self, took: Duration) {
        self.window.borrow_mut().paint.add(took);
        if let Some(sent) = self.pending_input.take() {
            self.window.borrow_mut().input.add(sent.elapsed());
        }
    }

    pub fn late(&self) {
        self.window.borrow_mut().late += 1;
    }

    pub fn dropped(&self) {
        self.window.borrow_mut().dropped += 1;
    }

    pub fn input_sent(&self) {
        if self.pending_input.get().is_none() {
            self.pending_input.set(Some(Instant::now()));
        }
    }

    pub fn pump(&self, took: Duration) {
        let mut w = self.window.borrow_mut();
        w.pump.add(took);
        let since = *w.since.get_or_insert_with(Instant::now);
        if since.elapsed() < LOG_INTERVAL {
            return;
        }
        let secs = since.elapsed().as_secs_f32();
        tracing::debug!(
            paints_per_s = w.paint.count as f32 / secs,
            paint_avg_ms = w.paint.avg_ms(),
            paint_worst_ms = w.paint.worst_ms(),
            late = w.late,
            dropped = w.dropped,
            pumps_per_s = w.pump.count as f32 / secs,
            pump_avg_ms = w.pump.avg_ms(),
            pump_worst_ms = w.pump.worst_ms(),
            pump_busy_pct = w.pump.total.as_secs_f32() * 100.0 / secs,
            input_to_frame_avg_ms = w.input.avg_ms(),
            input_to_frame_worst_ms = w.input.worst_ms(),
            "engine frames"
        );
        *w = Window::default();
    }
}
