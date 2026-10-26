//! `AXION_PERF_LOG=1`: one line per second about where frames went, for lag reports.
//!
//! Each line says how often the window was asked to redraw and actually redrew (the
//! compositor's frame callbacks pace that on Wayland), how many frames were presented or
//! skipped as unchanged, how long the main thread spent per stage, how long the GPU took for
//! the window's passes (timestamp queries, where the adapter has them), how many page frames
//! Chromium painted and how many reached the window, the page frame rate asked of Chromium,
//! the window's size, scale, present mode and monitor, the page-frame memory held, and the
//! CPU use of Axion's main process and of Chromium's processes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use axion_engine::cef::Counters;

use crate::resize_bench::{FrameTimes, cpu_seconds, rss_mb};

/// How often a line is logged.
const INTERVAL: Duration = Duration::from_secs(1);

/// What the browser knows about its window and pages when a line is due.
pub struct Snapshot {
    pub surface: (u32, u32),
    pub scale: f64,
    pub present_mode: String,
    pub monitor: Option<String>,
    pub refresh_mhz: Option<u32>,
    /// Frame rate Chromium was asked for.
    pub page_rate: u32,
    pub window_fullscreen: bool,
    pub presentation: bool,
    pub page_fullscreen: bool,
    /// The shown page's frame size.
    pub page_frame: Option<(u32, u32)>,
    pub gpu_frames: bool,
    pub engine: Counters,
    /// Views, page-frame bytes, imported buffers (see `CefEngine::frame_memory`).
    pub memory: (usize, u64, usize),
    pub skip_unchanged: bool,
}

/// The per-second log (`None` in `Browser` unless `AXION_PERF_LOG` is set).
pub struct PerfLog {
    since: Instant,
    frames: Vec<FrameTimes>,
    presented: u64,
    skipped: u64,
    surface_errors: u64,
    redraw_requests: u64,
    wakeups: u64,
    pump_ms: f32,
    last_redraw: Option<Instant>,
    worst_gap_ms: f32,
    engine: Option<Counters>,
    cpu: Option<(f64, f64)>,
    gpu: Option<GpuTimer>,
}

impl PerfLog {
    pub fn from_env() -> Option<Self> {
        std::env::var_os("AXION_PERF_LOG").filter(|v| v != "0").map(|_| Self::new())
    }

    fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: Vec::new(),
            presented: 0,
            skipped: 0,
            surface_errors: 0,
            redraw_requests: 0,
            wakeups: 0,
            pump_ms: 0.0,
            last_redraw: None,
            worst_gap_ms: 0.0,
            engine: None,
            cpu: cpu_seconds(),
            gpu: None,
        }
    }

    /// GPU timing of the window's passes, if the device can (`TIMESTAMP_QUERY`).
    pub fn enable_gpu_timer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            self.gpu = Some(GpuTimer::new(device, queue));
        } else {
            tracing::info!("perf log: the GPU has no timestamp queries; GPU time is not measured");
        }
    }

    /// The event loop woke up and pumped Chromium (`pump` took that long).
    pub fn wakeup(&mut self, pump: Duration) {
        self.wakeups += 1;
        self.pump_ms += pump.as_secs_f32() * 1000.0;
    }

    pub fn redraw_requested(&mut self) {
        self.redraw_requests += 1;
    }

    /// A redraw ran: its times, and whether it presented, was skipped as unchanged, or got no
    /// swapchain image.
    pub fn frame(&mut self, times: FrameTimes, outcome: FrameOutcome) {
        let now = Instant::now();
        if let Some(last) = self.last_redraw {
            self.worst_gap_ms = self.worst_gap_ms.max(now.duration_since(last).as_secs_f32() * 1000.0);
        }
        self.last_redraw = Some(now);
        self.frames.push(times);
        match outcome {
            FrameOutcome::Presented => self.presented += 1,
            FrameOutcome::Skipped => self.skipped += 1,
            FrameOutcome::NoImage => self.surface_errors += 1,
        }
    }

    /// Brackets the compositor's work of a frame with GPU timestamps.
    pub fn gpu_begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(gpu) = &mut self.gpu {
            gpu.begin(device, queue);
        }
    }

    pub fn gpu_end(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(gpu) = &mut self.gpu {
            gpu.end(device, queue);
        }
    }

    /// Whether a line is due.
    pub fn due(&self) -> bool {
        self.since.elapsed() >= INTERVAL
    }

    /// Logs a line and starts the next second.
    pub fn log(&mut self, snap: &Snapshot) {
        let secs = self.since.elapsed().as_secs_f32().max(1e-3);
        let n = self.frames.len().max(1) as f32;
        let mut sum = FrameTimes::default();
        let mut worst = FrameTimes::default();
        for f in &self.frames {
            sum.add(f);
            worst.max(f);
        }
        let e0 = self.engine.unwrap_or(snap.engine);
        let e = &snap.engine;
        let per_s = |a: u64, b: u64| format!("{:.0}", a.saturating_sub(b) as f32 / secs);
        let cpu = cpu_seconds();
        let (main_cpu, chromium_cpu) = match (self.cpu, cpu) {
            (Some((m0, c0)), Some((m1, c1))) => {
                (((m1 - m0) / f64::from(secs) * 100.0) as f32, ((c1 - c0) / f64::from(secs) * 100.0) as f32)
            }
            _ => (f32::NAN, f32::NAN),
        };
        let gpu = self.gpu.as_mut().map(GpuTimer::take);
        let (gpu_avg, gpu_max) = match gpu {
            Some((count, total, max)) if count > 0 => {
                (format!("{:.2}", total / count as f32), format!("{max:.2}"))
            }
            _ => ("-".to_owned(), "-".to_owned()),
        };
        tracing::info!(
            redraw_requests_per_s = format!("{:.0}", self.redraw_requests as f32 / secs),
            redraws_per_s = format!("{:.0}", self.frames.len() as f32 / secs),
            presented_fps = format!("{:.0}", self.presented as f32 / secs),
            skipped_per_s = format!("{:.0}", self.skipped as f32 / secs),
            no_image = self.surface_errors,
            worst_redraw_gap_ms = format!("{:.1}", self.worst_gap_ms),
            frame_avg_ms = format!("{:.2}", sum.total / n),
            frame_max_ms = format!("{:.2}", worst.total),
            layout_ms = format!("{:.2}", (sum.layout + sum.ui_layout) / n),
            paint_ms = format!("{:.2}", sum.paint / n),
            acquire_ms = format!("{:.2}", sum.acquire / n),
            acquire_max_ms = format!("{:.2}", worst.acquire),
            render_ms = format!("{:.2}", sum.render / n),
            present_ms = format!("{:.2}", sum.present / n),
            present_max_ms = format!("{:.2}", worst.present),
            configure_ms = format!("{:.2}", sum.configure),
            gpu_ms = gpu_avg,
            gpu_max_ms = gpu_max,
            wakeups_per_s = format!("{:.0}", self.wakeups as f32 / secs),
            pump_ms_per_s = format!("{:.1}", self.pump_ms / secs),
            chromium_paints_per_s = per_s(e.paints, e0.paints),
            page_fps = per_s(e.page_frames, e0.page_frames),
            gpu_copies_per_s = per_s(e.gpu_copies, e0.gpu_copies),
            settle_replaced_per_s = per_s(e.settle_replaced, e0.settle_replaced),
            late = e.late.saturating_sub(e0.late),
            dropped = e.dropped.saturating_sub(e0.dropped),
            view_resizes = e.view_resizes.saturating_sub(e0.view_resizes),
            frame_textures_new = e.frame_textures.saturating_sub(e0.frame_textures),
            page_rate = snap.page_rate,
            page_frame = ?snap.page_frame,
            gpu_frames = snap.gpu_frames,
            surface = ?snap.surface,
            scale = snap.scale,
            present_mode = %snap.present_mode,
            monitor = snap.monitor.as_deref().unwrap_or("?"),
            refresh_hz = snap.refresh_mhz.map_or(-1.0, |m| m as f32 / 1000.0),
            fullscreen = snap.window_fullscreen,
            f11 = snap.presentation,
            page_fullscreen = snap.page_fullscreen,
            skip_unchanged = snap.skip_unchanged,
            views = snap.memory.0,
            page_frames_mb = snap.memory.1 / (1024 * 1024),
            imported_buffers = snap.memory.2,
            cpu_main_pct = format!("{main_cpu:.0}"),
            cpu_chromium_pct = format!("{chromium_cpu:.0}"),
            rss_mb = rss_mb().map_or(-1, |m| m as i64),
            "perf"
        );
        self.since = Instant::now();
        self.frames.clear();
        (self.presented, self.skipped, self.surface_errors) = (0, 0, 0);
        (self.redraw_requests, self.wakeups, self.pump_ms, self.worst_gap_ms) = (0, 0, 0.0, 0.0);
        self.engine = Some(snap.engine);
        self.cpu = cpu;
    }
}

/// How a redraw ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    Skipped,
    NoImage,
}

/// Frames in flight for the GPU timer (each has its query set and read-back buffer).
const GPU_SLOTS: usize = 4;

/// A frame's two GPU timestamps, written by empty compute passes submitted before and after
/// the compositor's work (only `TIMESTAMP_QUERY` is needed for those), read back without
/// waiting.
struct GpuSlot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// 0 free, 1 begun, 2 mapping, 3 mapped, 4 map failed.
    state: Arc<AtomicU8>,
}

struct GpuTimer {
    slots: Vec<GpuSlot>,
    current: Option<usize>,
    period_ns: f32,
    count: u32,
    total_ms: f32,
    max_ms: f32,
}

const SLOT_FREE: u8 = 0;
const SLOT_BEGUN: u8 = 1;
const SLOT_MAPPING: u8 = 2;
const SLOT_MAPPED: u8 = 3;
const SLOT_FAILED: u8 = 4;

impl GpuTimer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let slots = (0..GPU_SLOTS)
            .map(|_| GpuSlot {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("perf log timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("perf log resolve"),
                    size: 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                read: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("perf log read"),
                    size: 16,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(SLOT_FREE)),
            })
            .collect();
        Self {
            slots,
            current: None,
            period_ns: queue.get_timestamp_period(),
            count: 0,
            total_ms: 0.0,
            max_ms: 0.0,
        }
    }

    fn stamp(device: &wgpu::Device, queue: &wgpu::Queue, slot: &GpuSlot, index: u32, resolve: bool) {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("perf log") });
        {
            let _pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("perf log stamp"),
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &slot.queries,
                    beginning_of_pass_write_index: Some(index),
                    end_of_pass_write_index: None,
                }),
            });
        }
        if resolve {
            encoder.resolve_query_set(&slot.queries, 0..2, &slot.resolve, 0);
            encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.read, 0, 16);
        }
        queue.submit([encoder.finish()]);
    }

    fn collect(&mut self) {
        for slot in &self.slots {
            match slot.state.load(Ordering::Acquire) {
                SLOT_MAPPED => {
                    let ticks = slot.read.slice(..).get_mapped_range().ok().map(|data| {
                        let word = |i: usize| {
                            let mut b = [0u8; 8];
                            b.copy_from_slice(&data[i * 8..i * 8 + 8]);
                            u64::from_le_bytes(b)
                        };
                        word(1).saturating_sub(word(0))
                    });
                    slot.read.unmap();
                    slot.state.store(SLOT_FREE, Ordering::Release);
                    let ms = ticks.map_or(f32::INFINITY, |t| t as f32 * self.period_ns / 1e6);
                    // A bogus pair (clock reset) would swamp the average.
                    if ms < 1000.0 {
                        self.count += 1;
                        self.total_ms += ms;
                        self.max_ms = self.max_ms.max(ms);
                    }
                }
                SLOT_FAILED => {
                    slot.read.unmap();
                    slot.state.store(SLOT_FREE, Ordering::Release);
                }
                _ => {}
            }
        }
    }

    fn begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.collect();
        let Some(i) = self.slots.iter().position(|s| s.state.load(Ordering::Acquire) == SLOT_FREE) else {
            self.current = None;
            return;
        };
        self.slots[i].state.store(SLOT_BEGUN, Ordering::Release);
        Self::stamp(device, queue, &self.slots[i], 0, false);
        self.current = Some(i);
    }

    fn end(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(i) = self.current.take() else { return };
        let slot = &self.slots[i];
        Self::stamp(device, queue, slot, 1, true);
        slot.state.store(SLOT_MAPPING, Ordering::Release);
        let state = slot.state.clone();
        slot.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            state.store(if r.is_ok() { SLOT_MAPPED } else { SLOT_FAILED }, Ordering::Release);
        });
    }

    /// Frames measured, total and worst GPU milliseconds since the last call.
    fn take(&mut self) -> (u32, f32, f32) {
        self.collect();
        let out = (self.count, self.total_ms, self.max_ms);
        (self.count, self.total_ms, self.max_ms) = (0, 0.0, 0.0);
        out
    }
}
