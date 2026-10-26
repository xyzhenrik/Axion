//! `--bench-resize`: a scripted window-resize benchmark (X11/XWayland, where the app may resize
//! its own window), and `AXION_RESIZE_LOG=1`: the same per-frame timings for real resizes.
//!
//! The run: the page loads (`--after`, default 8 s), then the window is measured standing still
//! (`idle`), while the page scrolls (`scroll`), resized through a series of sizes one per
//! `STEP` (`sweep`), sent a burst of resizes without waiting (`burst`), left to settle, and
//! measured standing still and scrolling again (`idle-after`, `scroll-after`): a resize must
//! leave nothing behind. Each phase logs one summary line: frames, frame-time percentiles,
//! where the main thread's time went, resize work, CPU use of Axion and its Chromium
//! processes, and memory.

use std::time::{Duration, Instant};

use glam::Vec2;

/// Time between two steps of the sweep (about one 60 Hz frame).
const STEP: Duration = Duration::from_millis(16);
/// Steps of the sweep from the small size to the large one and back.
const SWEEP_STEPS: u32 = 120;
/// Resizes sent at once in the burst.
const BURST: u32 = 60;
/// The sweep's sizes (logical px).
const SMALL: Vec2 = Vec2::new(1000.0, 700.0);
const LARGE: Vec2 = Vec2::new(1600.0, 1000.0);
/// The size the window is left at (none of the sweep's).
const AFTER: Vec2 = Vec2::new(1237.0, 811.0);
/// How long each measured phase lasts, and the pause after the burst.
const MEASURE: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_secs(2);

/// Where the main thread's time in one frame went (milliseconds).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    /// The whole redraw (with the window's resize applied in it).
    pub total: f32,
    /// Applying the window's new size (swapchain, layout, page resizes).
    pub resize_events: f32,
    /// Swapchain reconfigurations (in resize events or the frame).
    pub configure: f32,
    /// `update_layout` (responsive layout, page resizes).
    pub layout: f32,
    /// Taffy layout and text shaping of the widget tree.
    pub ui_layout: f32,
    /// Waiting for the swapchain image.
    pub acquire: f32,
    /// Building the scene (`Ui::paint`).
    pub paint: f32,
    /// Recording and submitting the compositor's passes.
    pub render: f32,
    /// `present`.
    pub present: f32,
}

impl FrameTimes {
    /// Each stage's worst time so far, with `o`'s.
    pub fn max(&mut self, o: &FrameTimes) {
        self.total = self.total.max(o.total);
        self.resize_events = self.resize_events.max(o.resize_events);
        self.configure = self.configure.max(o.configure);
        self.layout = self.layout.max(o.layout);
        self.ui_layout = self.ui_layout.max(o.ui_layout);
        self.acquire = self.acquire.max(o.acquire);
        self.paint = self.paint.max(o.paint);
        self.render = self.render.max(o.render);
        self.present = self.present.max(o.present);
    }

    pub fn add(&mut self, o: &FrameTimes) {
        self.total += o.total;
        self.resize_events += o.resize_events;
        self.configure += o.configure;
        self.layout += o.layout;
        self.ui_layout += o.ui_layout;
        self.acquire += o.acquire;
        self.paint += o.paint;
        self.render += o.render;
        self.present += o.present;
    }
}

/// Counters of resize work, kept by the browser and the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResizeCounters {
    /// `WindowEvent::Resized` events.
    pub events: u64,
    /// Swapchain reconfigurations.
    pub configures: u64,
    /// `update_layout` runs.
    pub layouts: u64,
    /// Views told they were resized (`was_resized`).
    pub view_resizes: u64,
    /// Page frame textures created (GPU and software paths).
    pub frame_textures: u64,
    /// Read-back buffers created for GPU frames' drawn check.
    pub check_buffers: u64,
    /// Page frames that arrived.
    pub page_frames: u64,
    /// Window frames not composited because they would have looked like the one shown.
    pub skipped_frames: u64,
}

impl ResizeCounters {
    fn since(&self, start: &ResizeCounters) -> ResizeCounters {
        ResizeCounters {
            skipped_frames: self.skipped_frames - start.skipped_frames,
            events: self.events - start.events,
            configures: self.configures - start.configures,
            layouts: self.layouts - start.layouts,
            view_resizes: self.view_resizes - start.view_resizes,
            frame_textures: self.frame_textures - start.frame_textures,
            check_buffers: self.check_buffers - start.check_buffers,
            page_frames: self.page_frames - start.page_frames,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Load,
    Idle,
    Scroll,
    Sweep,
    Burst,
    Settle,
    IdleAfter,
    ScrollAfter,
    Done,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Load => "load",
            Phase::Idle => "idle",
            Phase::Scroll => "scroll",
            Phase::Sweep => "sweep",
            Phase::Burst => "burst",
            Phase::Settle => "settle",
            Phase::IdleAfter => "idle-after",
            Phase::ScrollAfter => "scroll-after",
            Phase::Done => "done",
        }
    }

    fn next(self) -> Phase {
        match self {
            Phase::Load => Phase::Idle,
            Phase::Idle => Phase::Scroll,
            Phase::Scroll => Phase::Sweep,
            Phase::Sweep => Phase::Burst,
            Phase::Burst => Phase::Settle,
            Phase::Settle => Phase::IdleAfter,
            Phase::IdleAfter => Phase::ScrollAfter,
            Phase::ScrollAfter | Phase::Done => Phase::Done,
        }
    }

    /// Whether the page should be scrolled (one wheel event per frame) in this phase.
    pub fn scrolls(self) -> bool {
        matches!(self, Phase::Scroll | Phase::ScrollAfter)
    }
}

/// What the benchmark wants done now.
#[derive(Debug, PartialEq)]
pub enum Step {
    Nothing,
    /// Request these window sizes (logical px), in order, right now.
    Resize(Vec<Vec2>),
    /// The run is over.
    Finish,
}

/// Measurements of one phase.
#[derive(Default)]
struct Measure {
    frames: Vec<FrameTimes>,
    wakeups: u64,
    pump_ms: f32,
    counters: ResizeCounters,
    cpu: Option<(f64, f64)>,
}

/// A frame-time log for one phase or one interactive resize (`AXION_RESIZE_LOG`).
pub struct Recorder {
    started: Instant,
    m: Measure,
}

impl Recorder {
    pub fn new(counters: ResizeCounters) -> Self {
        let m = Measure { counters, cpu: cpu_seconds(), ..Default::default() };
        Self { started: Instant::now(), m }
    }

    pub fn frame(&mut self, times: FrameTimes) {
        self.m.frames.push(times);
    }

    pub fn wakeup(&mut self, pump: Duration) {
        self.m.wakeups += 1;
        self.m.pump_ms += pump.as_secs_f32() * 1000.0;
    }

    /// Logs the summary under `label`.
    pub fn log(&self, label: &str, counters: ResizeCounters) {
        let secs = self.started.elapsed().as_secs_f32().max(1e-3);
        let mut totals: Vec<f32> = self.m.frames.iter().map(|f| f.total).collect();
        totals.sort_by(f32::total_cmp);
        let pct =
            |p: f32| totals.get(((totals.len() as f32 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
        let mut sum = FrameTimes::default();
        for f in &self.m.frames {
            sum.add(f);
        }
        let n = self.m.frames.len().max(1) as f32;
        let c = counters.since(&self.m.counters);
        let (main_cpu, child_cpu) = match (self.m.cpu, cpu_seconds()) {
            (Some((m0, c0)), Some((m1, c1))) => {
                (((m1 - m0) / secs as f64 * 100.0) as f32, ((c1 - c0) / secs as f64 * 100.0) as f32)
            }
            _ => (f32::NAN, f32::NAN),
        };
        tracing::info!(
            phase = label,
            secs = format!("{secs:.2}"),
            fps = format!("{:.1}", self.m.frames.len() as f32 / secs),
            frame_avg_ms = format!("{:.2}", sum.total / n),
            frame_p50_ms = format!("{:.2}", pct(0.5)),
            frame_p95_ms = format!("{:.2}", pct(0.95)),
            frame_max_ms = format!("{:.2}", pct(1.0)),
            resize_events_ms = format!("{:.2}", sum.resize_events / n),
            configure_ms = format!("{:.2}", sum.configure / n),
            layout_ms = format!("{:.2}", sum.layout / n),
            ui_layout_ms = format!("{:.2}", sum.ui_layout / n),
            acquire_ms = format!("{:.2}", sum.acquire / n),
            paint_ms = format!("{:.2}", sum.paint / n),
            render_ms = format!("{:.2}", sum.render / n),
            present_ms = format!("{:.2}", sum.present / n),
            wakeups_per_s = format!("{:.0}", self.m.wakeups as f32 / secs),
            pump_ms_per_s = format!("{:.1}", self.m.pump_ms / secs),
            resize_events = c.events,
            configures = c.configures,
            layouts = c.layouts,
            view_resizes = c.view_resizes,
            frame_textures = c.frame_textures,
            check_buffers = c.check_buffers,
            presented_fps =
                format!("{:.1}", (self.m.frames.len() as u64).saturating_sub(c.skipped_frames) as f32 / secs),
            page_fps = format!("{:.1}", c.page_frames as f32 / secs),
            cpu_main_pct = format!("{main_cpu:.0}"),
            cpu_chromium_pct = format!("{child_cpu:.0}"),
            rss_mb = rss_mb().map_or(-1, |m| m as i64),
            pid = std::process::id(),
            "resize bench"
        );
    }
}

/// The scripted run of `--bench-resize`.
pub struct ResizeBench {
    phase: Phase,
    phase_started: Instant,
    load: Duration,
    /// Sweep steps done, and when the last one was.
    step: u32,
    last_step: Instant,
    pub recorder: Option<Recorder>,
}

impl ResizeBench {
    pub fn new(load: Duration) -> Self {
        let now = Instant::now();
        Self { phase: Phase::Load, phase_started: now, load, step: 0, last_step: now, recorder: None }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    fn phase_length(&self) -> Option<Duration> {
        match self.phase {
            Phase::Load => Some(self.load),
            Phase::Idle | Phase::Scroll | Phase::IdleAfter | Phase::ScrollAfter => Some(MEASURE),
            Phase::Settle => Some(SETTLE),
            // Until their resizes are sent (the burst: and a moment after them, see `tick`).
            Phase::Sweep | Phase::Burst | Phase::Done => None,
        }
    }

    /// Advances the script; `counters` start the next phase's measurement.
    pub fn tick(&mut self, now: Instant, counters: ResizeCounters) -> Step {
        let over = match self.phase {
            Phase::Sweep => self.step >= SWEEP_STEPS * 2,
            // The burst's events arrive over the next frames: measured for one second.
            Phase::Burst => self.step > 0 && now.duration_since(self.phase_started) >= Duration::from_secs(1),
            Phase::Done => return Step::Finish,
            _ => self.phase_length().is_some_and(|l| now.duration_since(self.phase_started) >= l),
        };
        if over {
            if let Some(r) = self.recorder.take()
                && self.phase != Phase::Load
                && self.phase != Phase::Settle
            {
                r.log(self.phase.name(), counters);
            }
            self.phase = self.phase.next();
            self.phase_started = now;
            self.step = 0;
            self.recorder = Some(Recorder::new(counters));
            if self.phase == Phase::Done {
                return Step::Finish;
            }
        }
        match self.phase {
            Phase::Sweep if now.duration_since(self.last_step) >= STEP => {
                self.last_step = now;
                self.step += 1;
                Step::Resize(vec![sweep_size(self.step)])
            }
            Phase::Burst if self.step == 0 => {
                self.step = 1;
                Step::Resize((1..=BURST).map(|i| sweep_size(i * 2 * SWEEP_STEPS / BURST)).collect())
            }
            // A size of its own, which no frame before had: what stays after a resize.
            Phase::Settle if self.step == 0 => {
                self.step = 1;
                Step::Resize(vec![after_size()])
            }
            _ => Step::Nothing,
        }
    }
}

/// The size the window is left at: `AFTER`, or `AXION_BENCH_AFTER=WxH` (logical px).
fn after_size() -> Vec2 {
    std::env::var("AXION_BENCH_AFTER")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(Vec2::new(w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or(AFTER)
}

/// Size at sweep step `step` (1-based): from `SMALL` to `LARGE` and back, linearly.
fn sweep_size(step: u32) -> Vec2 {
    let step = step.min(SWEEP_STEPS * 2);
    let t = if step <= SWEEP_STEPS { step } else { SWEEP_STEPS * 2 - step } as f32 / SWEEP_STEPS as f32;
    (SMALL + (LARGE - SMALL) * t).round()
}

/// CPU seconds used so far by this process and by all its descendants (Chromium's processes).
pub fn cpu_seconds() -> Option<(f64, f64)> {
    let me = std::process::id();
    let ticks = 100.0; // USER_HZ on Linux.
    let stat = |pid: u32| -> Option<(u32, f64)> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Fields after the command (which may contain spaces) start at the last ')'.
        let rest = &s[s.rfind(')')? + 2..];
        let f: Vec<&str> = rest.split(' ').collect();
        let ppid = f.get(1)?.parse().ok()?;
        let utime: f64 = f.get(11)?.parse().ok()?;
        let stime: f64 = f.get(12)?.parse().ok()?;
        Some((ppid, (utime + stime) / ticks))
    };
    let mut procs = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        if let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok())
            && let Some((ppid, cpu)) = stat(pid)
        {
            procs.push((pid, ppid, cpu));
        }
    }
    let main = procs.iter().find(|p| p.0 == me)?.2;
    let mut family = vec![me];
    let mut children = 0.0;
    let mut i = 0;
    while i < family.len() {
        let parent = family[i];
        for p in procs.iter().filter(|p| p.1 == parent) {
            family.push(p.0);
            children += p.2;
        }
        i += 1;
    }
    Some((main, children))
}

/// Resident memory of this process (MB).
pub fn rss_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_goes_up_and_back() {
        assert_eq!(sweep_size(0), SMALL);
        assert_eq!(sweep_size(SWEEP_STEPS), LARGE);
        assert_eq!(sweep_size(SWEEP_STEPS * 2), SMALL);
        assert_eq!(sweep_size(SWEEP_STEPS * 3), SMALL);
    }

    #[test]
    fn phases_run_in_order_and_finish() {
        let start = Instant::now();
        let mut bench = ResizeBench::new(Duration::ZERO);
        let c = ResizeCounters::default();
        let mut t = start;
        let mut seen = vec![];
        let mut resizes = 0;
        for _ in 0..100_000 {
            t += Duration::from_millis(17);
            match bench.tick(t, c) {
                Step::Finish => break,
                Step::Resize(sizes) => resizes += sizes.len(),
                Step::Nothing => {}
            }
            if seen.last() != Some(&bench.phase()) {
                seen.push(bench.phase());
            }
        }
        assert_eq!(
            seen,
            [
                Phase::Idle,
                Phase::Scroll,
                Phase::Sweep,
                Phase::Burst,
                Phase::Settle,
                Phase::IdleAfter,
                Phase::ScrollAfter,
            ]
        );
        assert_eq!(bench.phase(), Phase::Done);
        assert_eq!(resizes as u32, SWEEP_STEPS * 2 + BURST + 1);
    }
}
