//! When Chromium's message loop wants to run (CEF's external message pump).
//!
//! Chromium tells us through `on_schedule_message_pump_work`, from any of its threads, when it
//! has work for its UI thread (ours): now, or after a delay. The embedder's event loop sleeps
//! until then instead of calling `do_message_loop_work` in a busy loop (which kept a CPU core
//! at 100 % and took it from the page's own threads).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cef::*;

/// Longest the message loop goes without a pump even if Chromium asked for none (CEF's own
/// recommendation for external pumps: 30 times a second).
const MAX_PUMP_DELAY: Duration = Duration::from_millis(1000 / 30);

/// Wakes the embedder's event loop (from any thread) so it calls `Engine::pump`.
#[derive(Clone)]
pub struct Waker(Arc<dyn Fn() + Send + Sync>);

impl Waker {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(wake))
    }
}

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Waker")
    }
}

#[derive(Default)]
pub(super) struct PumpSchedule {
    /// The earliest time Chromium asked to be pumped at.
    due: Mutex<Option<Instant>>,
    waker: Mutex<Option<Waker>>,
}

impl PumpSchedule {
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = waker;
    }

    fn schedule(&self, delay: Duration) {
        let at = Instant::now() + delay;
        {
            let mut due = self.due.lock().unwrap_or_else(PoisonError::into_inner);
            *due = Some(due.map_or(at, |d| d.min(at)));
        }
        // A delayed pump is picked up by the deadline (`next`); only "now" needs a wake-up.
        if delay.is_zero() {
            let waker = self.waker.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(waker) = waker {
                (waker.0)();
            }
        }
    }

    /// Called right before `do_message_loop_work`: the work it does covers what was due.
    pub fn pumping(&self) {
        *self.due.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// When the next pump should happen.
    pub fn next(&self) -> Instant {
        let fallback = Instant::now() + MAX_PUMP_DELAY;
        let due = *self.due.lock().unwrap_or_else(PoisonError::into_inner);
        due.map_or(fallback, |d| d.min(fallback))
    }
}

wrap_browser_process_handler! {
    pub(super) struct BrowserProcessHandlerBuilder {
        schedule: Arc<PumpSchedule>,
    }

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.schedule.schedule(Duration::from_millis(delay_ms.max(0).unsigned_abs()));
        }
    }
}
