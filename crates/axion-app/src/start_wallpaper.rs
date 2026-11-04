//! The native start page's wallpaper: the selected background as a GPU texture that the browser
//! draws as an image layer under the page's plane, so the glass of the page refracts it.
//!
//! Pictures are decoded, scaled down to what the window needs and blurred (the Blur setting) on
//! a worker thread, then uploaded once: an idle start page costs no GPU time (the scene's
//! fingerprint stays the same). Animated images (GIF, animated WebP, APNG) are decoded to
//! frames (capped by `home.animated_wallpaper_max_mb` and `_max_edge`) that play while
//! [`should_play`] says so: the start page is shown, Live wallpapers are on, and no media plays
//! anywhere (a wallpaper is not media and never competes with it). Videos have no decoder in
//! this tree: they show their cached thumbnail as a still.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use axion_physics::{Animated, Spring};
use image::{AnimationDecoder, ImageFormat, ImageReader, RgbaImage};

/// The edge of what is baked is rounded up to a multiple of this (px), so small resizes do not
/// bake again.
const BUCKET: u32 = 512;
const MIN_EDGE: u32 = 1024;
const MAX_EDGE: u32 = 3072;
/// The wallpaper fades in over this response (a spring; instant with Reduce Motion).
const FADE_SPRING: (f32, f32) = (0.35, 1.0);

/// What was baked: the picture, the blur and the size bucket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    /// The blur in tenths of a CSS px.
    pub blur: u32,
    pub edge: u32,
    /// The picture may move: all its frames are baked (smaller), not only the first.
    pub animated: bool,
}

/// Limits for an animated wallpaper's frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationLimits {
    /// The longest side of a frame (px).
    pub max_edge: u32,
    /// All frames together (bytes of RGBA).
    pub max_bytes: u64,
}

/// One baked frame: size, pixels and how long it shows (ms).
type Frame = (u32, u32, Vec<u8>, u32);

struct Job {
    key: Key,
    source: PathBuf,
    thumb: PathBuf,
    /// The page's size in CSS px (the blur is in those).
    page: (f32, f32),
    limits: AnimationLimits,
}

struct Baked {
    key: Key,
    frames: Option<Vec<Frame>>,
}

/// An uploaded frame.
struct Shown {
    view: wgpu::TextureView,
    size: (u32, u32),
    delay: Duration,
}

pub struct Wallpaper {
    jobs: Sender<Job>,
    done: Receiver<Baked>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    shown: Option<(Key, Vec<Shown>)>,
    /// The frame showing and how long it has shown (animated pictures).
    frame: usize,
    elapsed: Duration,
    playing: bool,
    pending: Option<Key>,
    next: Option<Job>,
    /// The picture asked for last had nothing to show (a format that cannot be decoded and no
    /// thumbnail): the page keeps its ground.
    failed: Option<String>,
    fade: Animated<f32>,
}

/// The edge to bake a picture for a window whose longest side is `longest` physical px.
pub fn edge_for(longest: f32) -> u32 {
    let up = (longest / BUCKET as f32).ceil() as u32 * BUCKET;
    up.clamp(MIN_EDGE, MAX_EDGE)
}

impl Wallpaper {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        crate::downloads::spawn("start-wallpaper", move || {
            for job in job_rx {
                let frames = bake(&job);
                if done_tx.send(Baked { key: job.key, frames }).is_err() {
                    return;
                }
            }
        });
        Self {
            jobs,
            done,
            device,
            queue,
            shown: None,
            frame: 0,
            elapsed: Duration::ZERO,
            playing: false,
            pending: None,
            next: None,
            failed: None,
            fade: Animated::new(0.0, Spring::new(FADE_SPRING.0, FADE_SPRING.1)),
        }
    }

    /// Asks for `key` (when it is not what is shown or being made). Only the latest request counts.
    pub fn request(
        &mut self,
        key: Key,
        source: PathBuf,
        thumb: PathBuf,
        page: (f32, f32),
        limits: AnimationLimits,
    ) {
        if self.shown.as_ref().is_some_and(|(k, ..)| *k == key)
            || self.pending.as_ref() == Some(&key)
            || self.failed.as_deref() == Some(key.name.as_str()) && self.shown.is_none()
        {
            return;
        }
        let job = Job { key: key.clone(), source, thumb, page, limits };
        if self.pending.is_some() {
            self.next = Some(job);
            return;
        }
        self.send(job);
    }

    fn send(&mut self, job: Job) {
        let key = job.key.clone();
        if self.jobs.send(job).is_ok() {
            self.pending = Some(key);
        }
    }

    /// Takes what the worker baked. Returns whether the picture shown changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(baked) = self.done.try_recv() {
            self.pending = None;
            match baked.frames {
                Some(frames) if !frames.is_empty() => {
                    let uploaded: Vec<Shown> = frames
                        .iter()
                        .map(|(w, h, rgba, delay)| Shown {
                            view: self.upload(*w, *h, rgba),
                            size: (*w, *h),
                            delay: Duration::from_millis(u64::from(*delay)),
                        })
                        .collect();
                    // A different picture fades in; the same one re-baked (blur) swaps at once.
                    if self.shown.as_ref().is_none_or(|(k, ..)| k.name != baked.key.name) {
                        self.fade.snap(0.0);
                        self.frame = 0;
                        self.elapsed = Duration::ZERO;
                    }
                    self.frame = self.frame.min(uploaded.len() - 1);
                    self.fade.target = 1.0;
                    self.failed = None;
                    self.shown = Some((baked.key, uploaded));
                    changed = true;
                }
                Some(_) => {}
                None => {
                    tracing::debug!(name = baked.key.name, "start wallpaper could not be decoded");
                    self.failed = Some(baked.key.name);
                    self.shown = None;
                    changed = true;
                }
            }
            if let Some(job) = self.next.take() {
                self.send(job);
            }
        }
        changed
    }

    /// Nothing selected: the picture goes.
    pub fn clear(&mut self) {
        self.shown = None;
        self.pending = None;
        self.next = None;
        self.failed = None;
        self.fade.snap(0.0);
    }

    /// Plays or pauses an animated picture (a paused one keeps its frame).
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Steps the frame by wall time `dt` while playing. Returns whether the frame changed.
    pub fn advance(&mut self, dt: Duration) -> bool {
        let Some((_, frames)) = &self.shown else { return false };
        if !self.playing || frames.len() < 2 {
            return false;
        }
        self.elapsed += dt;
        let before = self.frame;
        // A frame shows at least a few ms (GIFs with a zero delay play at ~10 fps in browsers).
        while self.elapsed >= frames[self.frame].delay.max(MIN_DELAY) {
            self.elapsed -= frames[self.frame].delay.max(MIN_DELAY);
            self.frame = (self.frame + 1) % frames.len();
            if self.frame == before {
                break;
            }
        }
        self.frame != before
    }

    /// The picture and its size, once baked.
    pub fn view(&self) -> Option<(&wgpu::TextureView, (u32, u32))> {
        let (_, frames) = self.shown.as_ref()?;
        frames.get(self.frame).map(|f| (&f.view, f.size))
    }

    /// Advances the fade; returns whether it still moves.
    pub fn animate(&mut self, dt: f32, reduce_motion: bool, spring: Spring) -> bool {
        self.fade.spring = spring;
        if reduce_motion {
            self.fade.snap(self.fade.target);
        } else {
            self.fade.step(dt);
        }
        !self.fade.is_settled(0.004)
    }

    pub fn opacity(&self) -> f32 {
        self.fade.value.clamp(0.0, 1.0)
    }

    fn upload(&self, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("start page wallpaper"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            size,
        );
        texture.create_view(&Default::default())
    }
}

fn decode(path: &Path) -> Option<RgbaImage> {
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    reader.decode().ok().map(|i| i.to_rgba8())
}

/// The frames of an animated GIF, WebP or APNG at `path`, each scaled to `max_edge`, until the
/// frames together would pass `max_bytes` (then the animation is cut there). `None` for a
/// picture that does not move.
fn decode_animation(path: &Path, limits: AnimationLimits) -> Option<Vec<(RgbaImage, u32)>> {
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let format = reader.format()?;
    let file = BufReader::new(std::fs::File::open(path).ok()?);
    let frames = match format {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(file).ok()?.into_frames(),
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(file).ok()?.into_frames(),
        ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(file).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.apng().ok()?.into_frames()
        }
        _ => return None,
    };
    let mut out = Vec::new();
    let mut bytes = 0u64;
    for frame in frames {
        let Ok(frame) = frame else { break };
        let (num, den) = frame.delay().numer_denom_ms();
        let delay = num.checked_div(den).unwrap_or(0);
        let mut image = frame.into_buffer();
        if image.width().max(image.height()) > limits.max_edge {
            image = image::DynamicImage::ImageRgba8(image)
                .resize(limits.max_edge, limits.max_edge, image::imageops::FilterType::Triangle)
                .to_rgba8();
        }
        bytes += u64::from(image.width()) * u64::from(image.height()) * 4;
        if bytes > limits.max_bytes && !out.is_empty() {
            break;
        }
        out.push((image, delay));
    }
    (out.len() > 1).then_some(out)
}

fn bake_one(mut image: RgbaImage, edge: u32, blur: f32, page: (f32, f32)) -> RgbaImage {
    if image.width().max(image.height()) > edge {
        let dynamic = image::DynamicImage::ImageRgba8(image);
        image = dynamic.resize(edge, edge, image::imageops::FilterType::Triangle).to_rgba8();
    }
    if blur > 0.0 {
        // The blur is in page px; the picture covers the page, so a page px is this many of its own.
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        // `cover` scales by the larger ratio, so a page px holds the smaller one of its own.
        let per_page_px = (iw / page.0.max(1.0)).min(ih / page.1.max(1.0)).max(0.01);
        let sigma = blur * per_page_px;
        if sigma >= 0.3 {
            image = image::imageops::fast_blur(&image, sigma);
        }
    }
    image
}

/// Decodes the picture (a video has only its cached thumbnail), scales it down and blurs it:
/// one frame, or all of an animated image's.
fn bake(job: &Job) -> Option<Vec<Frame>> {
    let blur = job.key.blur as f32 / 10.0;
    if job.key.animated
        && let Some(frames) = decode_animation(&job.source, job.limits)
    {
        let edge = job.limits.max_edge;
        return Some(
            frames
                .into_iter()
                .map(|(image, delay)| {
                    let image = bake_one(image, edge, blur, job.page);
                    (image.width(), image.height(), image.into_raw(), delay)
                })
                .collect(),
        );
    }
    let image = decode(&job.source).or_else(|| decode(&job.thumb))?;
    let image = bake_one(image, job.key.edge, blur, job.page);
    Some(vec![(image.width(), image.height(), image.into_raw(), 0)])
}

/// An animated picture's frames show at least this long.
const MIN_DELAY: Duration = Duration::from_millis(20);

/// What decides whether a wallpaper that moves (animated image, video) plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayInputs {
    /// `wallpapers_animated(...)`: Live wallpapers on, and Reduce Motion off or ignored.
    pub live: bool,
    /// The start page is the page being shown (not covered by Settings, the overview, the
    /// Playlist, …) and the window is visible.
    pub shown: bool,
    /// Any tab, the Playlist or Picture in Picture plays media: playback is priority 1.
    pub media_playing: bool,
    /// The tab is frozen or discarded.
    pub frozen: bool,
}

/// Whether a moving wallpaper plays: only while shown and live, and never while something else
/// plays (it pauses on its still frame then).
pub fn should_play(inputs: PlayInputs) -> bool {
    inputs.live && inputs.shown && !inputs.media_playing && !inputs.frozen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_bucketed() {
        assert_eq!(edge_for(900.0), 1024);
        assert_eq!(edge_for(1024.0), 1024);
        assert_eq!(edge_for(1025.0), 1536);
        assert_eq!(edge_for(1600.0), 2048);
        assert_eq!(edge_for(2560.0), 2560);
        assert_eq!(edge_for(7680.0), 3072);
    }

    #[test]
    fn playback_policy() {
        let base = PlayInputs { live: true, shown: true, media_playing: false, frozen: false };
        assert!(should_play(base));
        assert!(!should_play(PlayInputs { live: false, ..base }));
        assert!(!should_play(PlayInputs { shown: false, ..base }));
        assert!(!should_play(PlayInputs { media_playing: true, ..base }));
        assert!(!should_play(PlayInputs { frozen: true, ..base }));
    }

    #[test]
    fn baking_scales_down_and_blurs() {
        let dir = std::env::temp_dir().join(format!("axion-wall-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("w.png");
        let mut img = RgbaImage::from_pixel(2000, 1000, image::Rgba([0, 0, 0, 255]));
        for x in 0..1000 {
            for y in 0..1000 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        img.save(&path).unwrap();
        let job = |blur: u32| Job {
            key: Key { name: "w".into(), blur, edge: 1024, animated: false },
            source: path.clone(),
            thumb: dir.join("none.png"),
            page: (1000.0, 500.0),
            limits: AnimationLimits { max_edge: 512, max_bytes: 1 << 20 },
        };
        let (w, h, plain, _) = bake(&job(0)).unwrap().remove(0);
        assert_eq!((w, h), (1024, 512));
        let (_, _, blurred, _) = bake(&job(200)).unwrap().remove(0);
        // The hard edge in the middle is soft after blurring.
        let at = |data: &[u8], x: u32| data[((256 * w + x) * 4) as usize];
        assert_eq!(at(&plain, 505), 255);
        assert!(at(&blurred, 515) > 0 && at(&blurred, 515) < 255);
        assert!(bake(&Job { source: dir.join("missing.png"), ..job(0) }).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
