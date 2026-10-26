//! The frame rate pages render at and the display's frame interval, from the monitor the
//! window is on.
//!
//! On Wayland the window only learns its monitor once the compositor shows it (the surface
//! "enters" an output after the first frame), so at start the monitor is unknown and the
//! fallback rate applies. It is looked at again every `CHECK_INTERVAL`: before, the rate was
//! only updated on a resize, so a window on a 240 Hz monitor rendered pages at 60 until the
//! first resize (fullscreen, say) and then suddenly at 240.

use std::time::{Duration, Instant};

/// Refresh rate assumed while the window's monitor is unknown.
pub const FALLBACK_HZ: u32 = 60;
/// How often the window's monitor's refresh rate is looked at again.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Frames per second pages render at: `configured` (`tabs.max_frame_rate`) if set, else the
/// monitor's refresh rate (`monitor_mhz`, millihertz), at most `cap`
/// (`tabs.max_frame_rate_cap`, 0: no cap).
pub fn page_frame_rate(configured: u32, cap: u32, monitor_mhz: Option<u32>) -> u32 {
    if configured > 0 {
        return configured;
    }
    let display = display_hz(monitor_mhz);
    if cap > 0 { display.min(cap) } else { display }
}

/// One refresh of the monitor (`monitor_mhz` millihertz, or `FALLBACK_HZ`).
pub fn display_interval(monitor_mhz: Option<u32>) -> Duration {
    let hz = monitor_mhz.filter(|m| *m > 0).map_or(f64::from(FALLBACK_HZ), |m| f64::from(m) / 1000.0);
    Duration::from_secs_f64(1.0 / hz.max(1.0))
}

fn display_hz(monitor_mhz: Option<u32>) -> u32 {
    monitor_mhz.filter(|m| *m > 0).map_or(FALLBACK_HZ, |m| m.div_ceil(1000)).max(1)
}

/// Notices when the window's monitor's refresh rate differs from the one the frame rates were
/// set for (the window was shown, moved to another monitor, the mode changed).
#[derive(Debug, Default)]
pub struct RefreshWatch {
    applied: Option<u32>,
    next_check: Option<Instant>,
}

impl RefreshWatch {
    /// The frame rates were just set for `monitor_mhz`.
    pub fn applied(&mut self, monitor_mhz: Option<u32>, now: Instant) {
        self.applied = monitor_mhz;
        self.next_check = Some(now + CHECK_INTERVAL);
    }

    /// Whether the refresh rate should be looked at again.
    pub fn due(&self, now: Instant) -> bool {
        self.next_check.is_none_or(|at| now >= at)
    }

    /// The monitor's refresh rate is `monitor_mhz` now: whether the frame rates should be set
    /// again. A monitor that can't be told (`None`, e.g. between two outputs) changes nothing.
    pub fn changed(&mut self, monitor_mhz: Option<u32>, now: Instant) -> bool {
        self.next_check = Some(now + CHECK_INTERVAL);
        monitor_mhz.is_some() && monitor_mhz != self.applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_rate_wins() {
        assert_eq!(page_frame_rate(30, 120, Some(240_000)), 30);
        assert_eq!(page_frame_rate(240, 120, Some(60_000)), 240);
    }

    #[test]
    fn the_display_rate_is_capped() {
        assert_eq!(page_frame_rate(0, 120, Some(240_000)), 120);
        assert_eq!(page_frame_rate(0, 120, Some(143_998)), 120);
        assert_eq!(page_frame_rate(0, 120, Some(59_940)), 60);
        assert_eq!(page_frame_rate(0, 0, Some(239_960)), 240, "0: no cap");
    }

    #[test]
    fn an_unknown_monitor_uses_the_fallback() {
        assert_eq!(page_frame_rate(0, 120, None), FALLBACK_HZ);
        assert_eq!(page_frame_rate(0, 120, Some(0)), FALLBACK_HZ);
        assert_eq!(display_interval(None), Duration::from_secs_f64(1.0 / 60.0));
        assert_eq!(display_interval(Some(240_000)), Duration::from_secs_f64(1.0 / 240.0));
    }

    #[test]
    fn a_monitor_known_later_is_noticed_once() {
        let t = Instant::now();
        let mut watch = RefreshWatch::default();
        assert!(watch.due(t));
        // Start: the window isn't shown yet, so its monitor is unknown.
        watch.applied(None, t);
        assert!(!watch.due(t + Duration::from_millis(500)));
        let later = t + CHECK_INTERVAL;
        assert!(watch.due(later));
        assert!(!watch.changed(None, later), "still unknown");
        // It was shown on a 240 Hz monitor.
        let shown = later + CHECK_INTERVAL;
        assert!(watch.changed(Some(240_000), shown));
        watch.applied(Some(240_000), shown);
        assert!(!watch.changed(Some(240_000), shown + CHECK_INTERVAL));
        // Between two outputs for a moment: nothing changes.
        assert!(!watch.changed(None, shown + CHECK_INTERVAL * 2));
        // Moved to the 60 Hz one.
        assert!(watch.changed(Some(60_000), shown + CHECK_INTERVAL * 3));
    }
}
