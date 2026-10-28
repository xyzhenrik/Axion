//! Bounded shutdown: quitting must never leave a window-less process hanging around.
//!
//! Everything of Axion's own (settings, session, window state, shields, downloads) is written
//! first, in `Browser::drop`, which then calls [`Guard::flushed`]. Shutting Chromium down
//! can take long (or hang: a renderer that does not answer, a GPU process stuck in a driver
//! call), so a watchdog thread, armed before the browser is dropped, ends the process
//! `window.shutdown_timeout_ms` after that, but never before the flush is done.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::accounts::Relaunch;

/// Shared between the watchdog and the browser being dropped.
#[derive(Clone, Default)]
pub struct Guard {
    flushed: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Guard {
    /// Our own data is on disk: the watchdog may end the process now.
    pub fn flushed(&self) {
        self.flushed.store(true, Ordering::SeqCst);
    }

    /// Shutdown finished by itself: the watchdog stands down.
    pub fn finished(&self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// Starts the watchdog. `limit` zero: never force the exit. A pending `relaunch` is started
/// by the watchdog if the exit is forced, so switching accounts still works.
pub fn arm(limit: Duration, relaunch: Option<Relaunch>) -> Guard {
    let guard = Guard::default();
    if limit.is_zero() {
        return guard;
    }
    let (flushed, done) = (Arc::clone(&guard.flushed), Arc::clone(&guard.done));
    let started = Instant::now();
    let spawned = std::thread::Builder::new().name("axion-shutdown-watch".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(20));
            if done.load(Ordering::SeqCst) {
                return;
            }
            if started.elapsed() >= limit && flushed.load(Ordering::SeqCst) {
                break;
            }
        }
        tracing::warn!(
            waited_ms = started.elapsed().as_millis() as u64,
            "shutdown took longer than the limit (window.shutdown_timeout_ms); ending the process"
        );
        if let Some(relaunch) = relaunch
            && let Err(e) = relaunch.spawn()
        {
            tracing::error!("restarting Axion failed: {e:#}");
        }
        // Chromium's helper processes end by themselves once the browser process is gone.
        std::process::exit(0);
    });
    if let Err(e) = spawned {
        tracing::warn!("shutdown watchdog unavailable: {e}");
    }
    guard
}
