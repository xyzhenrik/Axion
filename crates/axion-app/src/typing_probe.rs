//! Typing latency probe (a test aid): types a text one character at a time, like a keyboard
//! would between frames, and measures when each character became visible.
//!
//! `AXION_TYPE_PROBE=page:X,Y,W,H,INTERVAL_MS,START_S,TEXT` types into the page (the focused
//! field's rectangle X,Y,W,H in page pixels is sampled from every new page frame; the field
//! should show equally wide marks, e.g. a password field with a hidden caret);
//! `AXION_TYPE_PROBE=native:INTERVAL_MS,START_S,TEXT` types into the focused field of the
//! chrome. The summary is logged at info level as `typing probe`.

use std::time::{Duration, Instant};

/// Where the probe types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Into the page; the sampled rectangle is (x, y, width, height) in frame pixels.
    Page { region: (u32, u32, u32, u32) },
    /// Into the focused chrome field.
    Native,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub target: Target,
    pub interval: Duration,
    pub start_after: Duration,
    pub text: String,
}

impl Spec {
    /// Parses the value of `AXION_TYPE_PROBE`.
    pub fn parse(value: &str) -> Option<Self> {
        let (kind, rest) = value.split_once(':')?;
        match kind {
            "page" => {
                let mut parts = rest.splitn(7, ',');
                let mut number = || parts.next()?.parse::<u32>().ok();
                let region = (number()?, number()?, number()?, number()?);
                let interval = number()?;
                let start = number()?;
                let text = parts.next()?.to_owned();
                Some(Self::new(Target::Page { region }, interval, start, text))
            }
            "native" => {
                let mut parts = rest.splitn(3, ',');
                let interval = parts.next()?.parse().ok()?;
                let start = parts.next()?.parse().ok()?;
                Some(Self::new(Target::Native, interval, start, parts.next()?.to_owned()))
            }
            _ => None,
        }
    }

    fn new(target: Target, interval_ms: u32, start_s: u32, text: String) -> Self {
        Self {
            target,
            interval: Duration::from_millis(interval_ms.into()),
            start_after: Duration::from_secs(start_s.into()),
            text,
        }
    }
}

/// What the probe has seen of one run.
#[derive(Debug, Default)]
pub struct Tracker {
    /// When each character was handed to the browser.
    sent: Vec<Instant>,
    /// Every distinct frame seen after typing started: when, and its measure of ink.
    samples: Vec<(Instant, u32)>,
}

/// The result of a run.
#[derive(Debug, PartialEq)]
pub struct Summary {
    /// Per character: time from sending it until a frame showed at least that many characters.
    /// `None`: never shown.
    pub latency: Vec<Option<Duration>>,
    /// Frames that showed fewer characters than an earlier frame did (letters that vanished).
    pub regressions: usize,
    /// What every frame showed: milliseconds after the first character was sent, characters.
    pub shown: Vec<(i64, u32)>,
}

impl Tracker {
    pub fn sent(&mut self, at: Instant) {
        self.sent.push(at);
    }

    pub fn sent_count(&self) -> usize {
        self.sent.len()
    }

    pub fn sample(&mut self, at: Instant, ink: u32) {
        self.samples.push((at, ink));
    }

    /// The summary, once the last frame shows all characters: one character's ink is taken
    /// as the last frame's divided by the characters sent.
    pub fn summary(&self) -> Option<Summary> {
        let n = self.sent.len() as u32;
        let (_, last) = *self.samples.last()?;
        if n == 0 || last == 0 {
            return None;
        }
        let per_char = last as f32 / n as f32;
        let shown: Vec<(Instant, u32)> =
            self.samples.iter().map(|&(at, ink)| (at, ((ink as f32 / per_char) + 0.5) as u32)).collect();
        let mut regressions = 0;
        let mut best = 0;
        for &(_, count) in &shown {
            if count < best {
                regressions += 1;
            }
            best = best.max(count);
        }
        let latency = self
            .sent
            .iter()
            .enumerate()
            .map(|(i, &sent)| {
                shown
                    .iter()
                    .find(|&&(at, count)| at >= sent && count > i as u32)
                    .map(|&(at, _)| at.duration_since(sent))
            })
            .collect();
        let first = self.sent[0];
        let shown = shown
            .iter()
            .map(|&(at, count)| {
                let ms = if at >= first {
                    at.duration_since(first).as_millis() as i64
                } else {
                    -(first.duration_since(at).as_millis() as i64)
                };
                (ms, count)
            })
            .collect();
        Some(Summary { latency, regressions, shown })
    }
}

/// The pixels of `region` (x, y, width, height) of a texture, row by row, tightly packed.
pub fn read_region(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    region: (u32, u32, u32, u32),
) -> Option<Vec<u8>> {
    let (x, y, w, h) = region;
    let w = w.min(texture.width().saturating_sub(x));
    let h = h.min(texture.height().saturating_sub(y));
    if w == 0 || h == 0 {
        return None;
    }
    let row = (4 * w).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("typing probe"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let data = buffer.slice(..).get_mapped_range().ok()?;
    let mut pixels = Vec::with_capacity((4 * w * h) as usize);
    for line in 0..h as usize {
        pixels.extend_from_slice(&data[line * row as usize..][..4 * w as usize]);
    }
    Some(pixels)
}

/// How many pixels of `now` differ noticeably from `baseline` (the empty field).
pub fn ink(baseline: &[u8], now: &[u8]) -> u32 {
    baseline
        .as_chunks::<4>()
        .0
        .iter()
        .zip(now.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 40))
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_targets() {
        let page = Spec::parse("page:40,40,600,60,120,3,hello world").expect("page spec");
        assert_eq!(page.target, Target::Page { region: (40, 40, 600, 60) });
        assert_eq!(page.interval, Duration::from_millis(120));
        assert_eq!(page.text, "hello world");
        let native = Spec::parse("native:80,2,abc").expect("native spec");
        assert_eq!(native.target, Target::Native);
        assert_eq!(native.start_after, Duration::from_secs(2));
        assert!(Spec::parse("other:1").is_none());
    }

    #[test]
    fn ink_counts_changed_pixels() {
        let empty = [255u8; 16];
        let mut typed = empty;
        typed[4..8].copy_from_slice(&[0, 0, 0, 255]);
        typed[8] = 250;
        assert_eq!(ink(&empty, &typed), 1);
    }

    #[test]
    fn measures_latency_and_vanishing_letters() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut tracker = Tracker::default();
        tracker.sent(ms(0));
        tracker.sent(ms(100));
        tracker.sent(ms(200));
        // Ink of 10 per character: "a" appears, then "ab" appears and vanishes again for a
        // frame, and everything is there at the end.
        for (at, ink) in [(16, 0), (33, 10), (120, 20), (136, 10), (250, 30)] {
            tracker.sample(ms(at), ink);
        }
        let summary = tracker.summary().expect("summary");
        assert_eq!(summary.regressions, 1);
        assert_eq!(summary.shown.len(), 5);
        assert_eq!(
            summary.latency,
            [
                Some(Duration::from_millis(33)),
                Some(Duration::from_millis(20)),
                Some(Duration::from_millis(50))
            ]
        );
    }
}
