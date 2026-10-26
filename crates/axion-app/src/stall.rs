//! `AXION_STALL_LOG=1`: a watchdog for freezes of the main thread (a debug aid for reports).
//!
//! The main thread marks each event-loop iteration as busy ([`busy`], when winit hands it new
//! events) and idle again ([`idle`], before it sleeps), and names what it is doing with
//! [`op`] guards around its main call sites (input, the Chromium pump, layout, rendering,
//! clipboard, dialogs, settings). A background thread wakes every few milliseconds; when an
//! iteration has been busy for longer than the threshold (150 ms, or `AXION_STALL_LOG=<ms>`)
//! it logs what the thread is doing and where the kernel has it waiting
//! (`/proc/self/task/<tid>/wchan` and `syscall`: a futex is a lock or a blocking call into
//! another thread, `do_poll`/`ep_poll` a socket or pipe, `wait4` a child process), then a
//! second line with the stall's full length once the thread moves on. With
//! `AXION_STALL_STACK=1` too, a stall that lasts a second also logs the main thread's stack,
//! read with `eu-stack` or `gdb` where installed (this lets debuggers of the same user attach
//! to Axion, so it is for debugging sessions only).
//!
//! When not enabled every call is a single relaxed atomic load.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::platform::stall as platform;

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Milliseconds since [`START`] when the current iteration began (0: idle).
static BUSY_SINCE: AtomicU64 = AtomicU64::new(0);
/// What the main thread is doing now, and since when (ms since [`START`]).
static CURRENT: Mutex<(&str, u64)> = Mutex::new(("idle", 0));
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// The default threshold: a stall is anything longer than this without the loop coming round.
const DEFAULT_THRESHOLD: Duration = Duration::from_millis(150);
/// How often the watchdog looks.
const POLL: Duration = Duration::from_millis(10);

/// The threshold `AXION_STALL_LOG` asks for: `None` when off. "1" (or any value under 10)
/// switches it on at the default; a larger number is the threshold in milliseconds.
pub fn threshold_from(value: Option<&str>) -> Option<Duration> {
    let value = value?.trim();
    if value.is_empty() || value == "0" {
        return None;
    }
    match value.parse::<u64>() {
        Ok(ms) if ms >= 10 => Some(Duration::from_millis(ms)),
        _ => Some(DEFAULT_THRESHOLD),
    }
}

/// Starts the watchdog if `AXION_STALL_LOG` is set. Call once, from the main thread.
pub fn start_from_env() {
    let Some(threshold) = threshold_from(std::env::var("AXION_STALL_LOG").ok().as_deref()) else { return };
    let main_tid = platform::main_thread_id();
    let stacks = std::env::var_os("AXION_STALL_STACK").is_some_and(|v| v != "0");
    if stacks {
        platform::allow_stack_capture();
    }
    START.get_or_init(Instant::now);
    ENABLED.store(true, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("axion-stall-watch".into()).spawn(move || {
        let mut watch = Watch::new(threshold);
        let mut stack_taken_for = None;
        loop {
            std::thread::sleep(POLL);
            let now = now_ms();
            let busy = BUSY_SINCE.load(Ordering::Relaxed);
            let current = CURRENT.try_lock().map(|c| *c).ok();
            match watch.observe(now, busy, current) {
                Report::None => {}
                Report::Stalled { busy_ms, op, op_ms } => {
                    let kernel = platform::kernel_wait(main_tid);
                    tracing::warn!(
                        "stall: main thread busy for {busy_ms} ms, in \"{op}\" for {op_ms} ms ({kernel})"
                    );
                }
                Report::StillStalled { busy_ms, op } => {
                    let kernel = platform::kernel_wait(main_tid);
                    tracing::warn!("stall: still busy after {busy_ms} ms, in \"{op}\" ({kernel})");
                    // Once per stall: the stuck thread's stack, where a debugger may read it.
                    if stacks && stack_taken_for != Some(busy) {
                        stack_taken_for = Some(busy);
                        match platform::main_thread_stack() {
                            Some(stack) => tracing::warn!("stall: main thread stack:\n{stack}"),
                            None => tracing::warn!("stall: no stack (install elfutils or gdb)"),
                        }
                    }
                }
                Report::Ended { busy_ms, op } => {
                    tracing::warn!("stall: main thread was busy for {busy_ms} ms (last in \"{op}\")");
                }
            }
        }
    });
    match spawned {
        Ok(_) => tracing::info!(threshold_ms = threshold.as_millis() as u64, "stall watchdog on"),
        Err(e) => tracing::warn!("stall watchdog unavailable: {e}"),
    }
}

/// An event-loop iteration starts (winit has new events for us).
pub fn busy() {
    if ENABLED.load(Ordering::Relaxed) && BUSY_SINCE.load(Ordering::Relaxed) == 0 {
        BUSY_SINCE.store(now_ms().max(1), Ordering::Relaxed);
    }
}

/// The iteration ended: the loop is about to sleep until the next event or wakeup.
pub fn idle() {
    if ENABLED.load(Ordering::Relaxed) {
        BUSY_SINCE.store(0, Ordering::Relaxed);
        set_current("idle");
    }
}

/// Names what the main thread does until the guard is dropped (then the previous name is back).
#[must_use = "the name holds only while the guard lives"]
pub fn op(label: &'static str) -> Op {
    if !ENABLED.load(Ordering::Relaxed) {
        return Op { previous: None };
    }
    let previous = CURRENT.lock().map(|c| c.0).ok();
    set_current(label);
    Op { previous }
}

/// See [`op`].
pub struct Op {
    previous: Option<&'static str>,
}

impl Drop for Op {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            set_current(previous);
        }
    }
}

fn set_current(label: &'static str) {
    if let Ok(mut current) = CURRENT.lock() {
        *current = (label, now_ms());
    }
}

fn now_ms() -> u64 {
    START.get().map_or(0, |start| start.elapsed().as_millis() as u64)
}

/// What the watchdog has to say after a look.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    None,
    Stalled { busy_ms: u64, op: &'static str, op_ms: u64 },
    StillStalled { busy_ms: u64, op: &'static str },
    Ended { busy_ms: u64, op: &'static str },
}

/// The watchdog's state between looks (pure, for tests).
struct Watch {
    threshold_ms: u64,
    /// The stalled iteration being reported: (its start, the last op seen, when last logged).
    stalled: Option<(u64, &'static str, u64)>,
}

/// While a stall lasts, a reminder this often.
const REMIND_MS: u64 = 1000;

impl Watch {
    fn new(threshold: Duration) -> Self {
        Self { threshold_ms: threshold.as_millis().max(1) as u64, stalled: None }
    }

    /// `busy`: when the running iteration began (0: idle); `current`: the op and since when
    /// (`None` when its lock was busy this time).
    fn observe(&mut self, now: u64, busy: u64, current: Option<(&'static str, u64)>) -> Report {
        if let Some((since, op, logged)) = self.stalled {
            if busy != since {
                // The loop came round (or went idle): the stall is over.
                self.stalled = None;
                return Report::Ended { busy_ms: now.saturating_sub(since), op };
            }
            let op = current.map_or(op, |c| c.0);
            if now.saturating_sub(logged) >= REMIND_MS {
                self.stalled = Some((since, op, now));
                return Report::StillStalled { busy_ms: now.saturating_sub(since), op };
            }
            self.stalled = Some((since, op, logged));
            return Report::None;
        }
        if busy == 0 || now.saturating_sub(busy) < self.threshold_ms {
            return Report::None;
        }
        let (op, op_since) = current.unwrap_or(("?", busy));
        self.stalled = Some((busy, op, now));
        Report::Stalled {
            busy_ms: now.saturating_sub(busy),
            op,
            op_ms: now.saturating_sub(op_since.max(busy)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_from_the_environment() {
        assert_eq!(threshold_from(None), None);
        assert_eq!(threshold_from(Some("0")), None);
        assert_eq!(threshold_from(Some("")), None);
        assert_eq!(threshold_from(Some("1")), Some(DEFAULT_THRESHOLD));
        assert_eq!(threshold_from(Some("yes")), Some(DEFAULT_THRESHOLD));
        assert_eq!(threshold_from(Some("400")), Some(Duration::from_millis(400)));
    }

    #[test]
    fn a_stall_is_reported_once_reminded_and_ended() {
        let mut watch = Watch::new(Duration::from_millis(150));
        // Idle, or busy for less than the threshold: nothing.
        assert_eq!(watch.observe(1000, 0, Some(("idle", 0))), Report::None);
        assert_eq!(watch.observe(1100, 1000, Some(("input", 1000))), Report::None);
        // Over it: reported with the op and how long it has run.
        assert_eq!(
            watch.observe(1200, 1000, Some(("cef pump", 1050))),
            Report::Stalled { busy_ms: 200, op: "cef pump", op_ms: 150 }
        );
        // Not again at every look …
        assert_eq!(watch.observe(1300, 1000, Some(("cef pump", 1050))), Report::None);
        // … but as a reminder a second later (the op lock was taken: the last op stays).
        assert_eq!(watch.observe(2300, 1000, None), Report::StillStalled { busy_ms: 1300, op: "cef pump" });
        // The loop came round: the stall's full length.
        assert_eq!(
            watch.observe(5010, 0, Some(("idle", 5005))),
            Report::Ended { busy_ms: 4010, op: "cef pump" }
        );
        assert_eq!(watch.observe(5020, 0, Some(("idle", 5005))), Report::None);
        // A new iteration that stalls is a new report.
        assert_eq!(watch.observe(6000, 5800, None), Report::Stalled { busy_ms: 200, op: "?", op_ms: 200 });
    }

    #[test]
    fn guards_restore_the_previous_name() {
        START.get_or_init(Instant::now);
        ENABLED.store(true, Ordering::Relaxed);
        {
            let _outer = op("outer");
            {
                let _inner = op("inner");
                assert_eq!(CURRENT.lock().map(|c| c.0).ok(), Some("inner"));
            }
            assert_eq!(CURRENT.lock().map(|c| c.0).ok(), Some("outer"));
        }
        assert_ne!(CURRENT.lock().map(|c| c.0).ok(), Some("outer"));
    }
}
