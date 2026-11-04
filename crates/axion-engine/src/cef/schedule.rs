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

    /// Wakes the event loop.
    pub fn wake(&self) {
        (self.0)();
    }
}

/// Called with the URLs of a launch that Chromium's own process singleton handed over (see
/// `on_already_running_app_relaunch`).
#[derive(Clone)]
pub struct RelaunchHandler(Arc<dyn Fn(Vec<String>) + Send + Sync>);

impl RelaunchHandler {
    pub fn new(handler: impl Fn(Vec<String>) + Send + Sync + 'static) -> Self {
        Self(Arc::new(handler))
    }
}

impl fmt::Debug for RelaunchHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelaunchHandler")
    }
}

/// The URL arguments in a command line string: Chromium joins the arguments with spaces
/// (URLs have none), so these are the words after the program that are not switches.
pub fn urls_in_command_line(line: &str) -> Vec<String> {
    line.split_whitespace().skip(1).filter(|w| !w.starts_with('-')).map(str::to_owned).collect()
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
    relaunch: Mutex<Option<RelaunchHandler>>,
}

impl PumpSchedule {
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = waker;
    }

    pub fn set_relaunch(&self, handler: Option<RelaunchHandler>) {
        *self.relaunch.lock().unwrap_or_else(PoisonError::into_inner) = handler;
    }

    fn relaunched(&self, urls: Vec<String>) {
        let handler = self.relaunch.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(handler) = handler {
            (handler.0)(urls);
        }
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
        fn on_context_initialized(&self) {
            tracing::info!("Chromium context initialized");
        }

        /// Another process was started on this profile and Chromium's singleton handed its
        /// command line over. Without an answer Chromium opens one of its own browser windows
        /// (its UI, not Axion's); we take the URLs and say it is handled.
        fn on_already_running_app_relaunch(
            &self,
            command_line: Option<&mut CommandLine>,
            _current_directory: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            let line = command_line.map(|c| CefString::from(&c.command_line_string()).to_string()).unwrap_or_default();
            tracing::info!("another launch reached this profile through Chromium");
            self.schedule.relaunched(urls_in_command_line(&line));
            1
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.schedule.schedule(Duration::from_millis(delay_ms.max(0).unsigned_abs()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_the_non_switch_words_after_the_program() {
        let line = "/usr/bin/axion --password-store=basic https://a.example/x file:///tmp/a.html";
        assert_eq!(urls_in_command_line(line), ["https://a.example/x", "file:///tmp/a.html"]);
        assert!(urls_in_command_line("axion --foo").is_empty());
    }
}
