//! The start page's background images.
//!
//! Images the user adds are copied into the account's backgrounds directory
//! (`<profile>/home/backgrounds/`) under new, unique file names that keep the original's
//! extension; `home.background` names the selected one. Nothing outside that directory is
//! ever referenced, and only plain files with a safe name (see [`valid_name`]) count.
//! Thumbnails are cached in `backgrounds/.thumbs/<name>.png`; they are made — and blurred for
//! the Settings preview — on a worker thread ([`Worker`]), never on the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use image::{DynamicImage, ImageReader, RgbaImage};

/// The backgrounds directory inside `<profile>/home`.
pub const DIR: &str = "backgrounds";
/// The thumbnail cache inside it (a dot name, so it never counts as a background).
const THUMBS: &str = ".thumbs";
/// The single image older versions kept (`<profile>/home/background`), migrated once.
pub const LEGACY_FILE: &str = "background";
/// Larger images are refused as backgrounds.
const MAX_BYTES: u64 = 40 * 1024 * 1024;
/// Thumbnails fit in this many pixels: twice Settings' largest picture of one (its preview,
/// see `settings::BACKGROUND_PREVIEW`), so they stay sharp on HiDPI screens.
pub const THUMB_MAX: (u32, u32) = (840, 525);
/// Longest file name we create or accept.
const MAX_NAME: usize = 64;

/// A texture and its size in pixels, as image widgets take it.
pub type Image = (wgpu::TextureView, (u32, u32));
/// RGBA pixels: width, height, data.
pub type Pixels = (u32, u32, Vec<u8>);

/// Whether `name` is a file name we could have given a background: ASCII letters, digits,
/// `-`, `_` and `.`, not starting with a dot (no paths, no `..`, no hidden files).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('.')
        && !name.contains("..")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The MIME type of an image file, by its first bytes (`None`: not an image Chromium shows).
pub fn image_mime(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = [0u8; 32];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    let format = image::guess_format(&head[..n]).ok()?;
    use image::ImageFormat as F;
    matches!(format, F::Png | F::Jpeg | F::Gif | F::WebP | F::Avif | F::Bmp)
        .then(|| format.to_mime_type().to_owned())
}

/// The extension a copy of `source` gets: the original's (lower case) when it is a short
/// plain one, otherwise the format's usual one.
fn extension_for(source: &Path, mime: &str) -> String {
    let original = source.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match original {
        Some(e) if (1..=5).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_alphanumeric()) => e,
        _ => image::ImageFormat::from_mime_type(mime)
            .and_then(|f| f.extensions_str().first().copied())
            .unwrap_or("img")
            .to_owned(),
    }
}

/// The backgrounds in the account's directory.
pub struct Library {
    dir: PathBuf,
    /// File name and MIME type, oldest first (names start with the time they were added).
    entries: Vec<(String, String)>,
}

impl Library {
    /// The library in `<home>/backgrounds` (`home`: `<profile>/home`).
    pub fn new(home: &Path) -> Self {
        let mut library = Self { dir: home.join(DIR), entries: Vec::new() };
        library.scan();
        library
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads the directory again: plain image files with a valid name (no links).
    pub fn scan(&mut self) {
        self.entries.clear();
        let Ok(read) = std::fs::read_dir(&self.dir) else { return };
        for entry in read.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let plain = entry.file_type().is_ok_and(|t| t.is_file());
            if !plain || !valid_name(&name) {
                continue;
            }
            if let Some(mime) = image_mime(&entry.path()) {
                self.entries.push((name, mime));
            }
        }
        self.entries.sort();
    }

    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|(n, _)| n == name)
    }

    /// The file and MIME type of a listed background.
    pub fn file(&self, name: &str) -> Option<(PathBuf, String)> {
        self.entries.iter().find(|(n, _)| n == name).map(|(n, m)| (self.dir.join(n), m.clone()))
    }

    pub fn thumbs_dir(&self) -> PathBuf {
        self.dir.join(THUMBS)
    }

    pub fn thumb_path(&self, name: &str) -> PathBuf {
        self.thumbs_dir().join(format!("{name}.png"))
    }

    /// A new, unused file name for an image of type `ext`.
    fn new_name(&self, ext: &str) -> String {
        let millis =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        (0u32..)
            .map(|n| format!("bg-{millis:013}-{n}.{ext}"))
            .find(|name| !self.contains(name) && !self.dir.join(name).exists())
            .unwrap_or_else(|| format!("bg-{millis:013}.{ext}"))
    }

    /// Copies the image at `source` in (at most `max` are kept). Returns its new name.
    pub fn add(&mut self, source: &Path, max: usize) -> Option<String> {
        let size = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
        let Some(mime) = image_mime(source).filter(|_| size > 0 && size <= MAX_BYTES) else {
            tracing::warn!(path = %source.display(), "not a usable background image");
            return None;
        };
        if self.entries.len() >= max {
            tracing::warn!(max, "too many background images; remove one first");
            return None;
        }
        let name = self.new_name(&extension_for(source, &mime));
        let result = std::fs::create_dir_all(&self.dir).and_then(|()| {
            let tmp = self.dir.join(format!(".{name}.tmp"));
            std::fs::copy(source, &tmp)?;
            std::fs::rename(&tmp, self.dir.join(&name))
        });
        if let Err(e) = result {
            tracing::warn!("saving the background image failed: {e}");
            return None;
        }
        self.scan();
        Some(name)
    }

    /// Deletes a background's copy and thumbnail. Returns whether it was there.
    pub fn remove(&mut self, name: &str) -> bool {
        if !self.contains(name) {
            return false;
        }
        for path in [self.dir.join(name), self.thumb_path(name)] {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), "removing a background failed: {e}");
            }
        }
        self.scan();
        true
    }

    /// Moves the single image of older versions (`legacy`) in. Returns its new name.
    pub fn migrate(&mut self, legacy: &Path) -> Option<String> {
        let plain = std::fs::symlink_metadata(legacy).is_ok_and(|m| m.is_file());
        if !plain {
            return None;
        }
        let Some(mime) = image_mime(legacy) else {
            tracing::warn!(path = %legacy.display(), "old background is no image; left alone");
            return None;
        };
        let ext = extension_for(Path::new(""), &mime);
        let name = self.new_name(&ext);
        let target = self.dir.join(&name);
        let moved = std::fs::create_dir_all(&self.dir).and_then(|()| {
            std::fs::rename(legacy, &target).or_else(|_| std::fs::copy(legacy, &target).map(drop))
        });
        if let Err(e) = moved {
            tracing::warn!("moving the old background image failed: {e}");
            return None;
        }
        let _ = std::fs::remove_file(legacy);
        tracing::info!(name, "old background image moved into the backgrounds directory");
        self.scan();
        Some(name)
    }
}

/// Work for the thumbnail thread.
enum Job {
    /// Make (or load) a background's thumbnail.
    Thumb { name: String, source: PathBuf, cache: PathBuf },
    /// The thumbnail blurred by `sigma` px (Settings' preview).
    Preview { key: PreviewKey, source: PathBuf, cache: PathBuf },
}

/// Which preview: the background's name and the blur (sigma in tenths of a thumbnail px).
pub type PreviewKey = (String, u32);

enum Done {
    Thumb(String, Option<Pixels>),
    Preview(PreviewKey, Option<Pixels>),
}

/// The thumbnail at `cache`, or made from `source` (and cached there).
fn thumbnail(source: &Path, cache: &Path) -> Option<RgbaImage> {
    if let Ok(image) = ImageReader::open(cache).and_then(|r| r.with_guessed_format()).map(|r| r.decode())
        && let Ok(image) = image
    {
        return Some(image.to_rgba8());
    }
    let decoded: DynamicImage = match ImageReader::open(source).and_then(|r| r.with_guessed_format()) {
        Ok(reader) => match reader.decode() {
            Ok(image) => image,
            Err(e) => {
                // Formats Chromium shows but we can't decode (WebP, GIF, AVIF): no thumbnail.
                tracing::debug!(path = %source.display(), "no thumbnail: {e}");
                return None;
            }
        },
        Err(e) => {
            tracing::debug!(path = %source.display(), "no thumbnail: {e}");
            return None;
        }
    };
    // Only ever smaller (`thumbnail` would also enlarge small images).
    let fits = decoded.width() <= THUMB_MAX.0 && decoded.height() <= THUMB_MAX.1;
    let thumb = if fits { decoded } else { decoded.thumbnail(THUMB_MAX.0, THUMB_MAX.1) }.to_rgba8();
    if let Some(dir) = cache.parent()
        && let Err(e) = std::fs::create_dir_all(dir).map_err(image::ImageError::IoError).and_then(|()| {
            let tmp = cache.with_extension("tmp");
            thumb.save_with_format(&tmp, image::ImageFormat::Png)?;
            std::fs::rename(&tmp, cache).map_err(image::ImageError::IoError)
        })
    {
        tracing::debug!("caching a background thumbnail failed: {e}");
    }
    Some(thumb)
}

fn pixels(image: RgbaImage) -> Pixels {
    (image.width(), image.height(), image.into_raw())
}

fn work(jobs: Receiver<Job>, done: Sender<Done>) {
    for job in jobs {
        let result = match job {
            Job::Thumb { name, source, cache } => Done::Thumb(name, thumbnail(&source, &cache).map(pixels)),
            Job::Preview { key, source, cache } => {
                let sigma = key.1 as f32 / 10.0;
                let image = thumbnail(&source, &cache)
                    .map(|t| if sigma > 0.0 { image::imageops::fast_blur(&t, sigma) } else { t });
                Done::Preview(key, image.map(pixels))
            }
        };
        if done.send(result).is_err() {
            return;
        }
    }
}

/// GPU side: the thumbnails and the preview as textures (once a device is set).
pub struct Textures {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Textures {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue }
    }

    fn upload(&self, (width, height, rgba): &Pixels) -> Image {
        let size = wgpu::Extent3d { width: *width, height: *height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("background thumbnail"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Like site icons: sRGB values as they are (the compositor blends in that space).
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(*height),
            },
            size,
        );
        (texture.create_view(&Default::default()), (*width, *height))
    }
}

/// What changed after [`Thumbnails::poll`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arrived {
    pub thumbs: bool,
    pub preview: bool,
}

/// The thumbnail thread and what it made (main thread).
pub struct Thumbnails {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    gpu: Option<Textures>,
    /// Uploaded thumbnails (`None`: it has none, e.g. a format we can't decode).
    textures: HashMap<String, Option<Image>>,
    pending: HashSet<String>,
    /// The preview shown, the one being made, and the one asked for next.
    preview: Option<(PreviewKey, Image)>,
    preview_pending: Option<PreviewKey>,
    preview_next: Option<PreviewKey>,
}

impl Thumbnails {
    pub fn start() -> Self {
        let (jobs, job_rx) = channel();
        let (done_tx, done) = channel();
        // Should the thread not start, its receiver is gone and every request simply fails.
        crate::downloads::spawn("background-thumbnails", move || work(job_rx, done_tx));
        Self {
            jobs,
            done,
            gpu: None,
            textures: HashMap::new(),
            pending: HashSet::new(),
            preview: None,
            preview_pending: None,
            preview_next: None,
        }
    }

    pub fn set_gpu(&mut self, gpu: Textures) {
        self.gpu = Some(gpu);
    }

    fn send(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }

    /// Makes sure every background in `library` has a thumbnail on disk (and as a texture,
    /// once a device is set); forgets removed ones.
    pub fn request(&mut self, library: &Library) {
        self.textures.retain(|name, _| library.contains(name));
        for name in library.names() {
            if self.textures.contains_key(name) || self.pending.contains(name) {
                continue;
            }
            let job = Job::Thumb {
                name: name.to_owned(),
                source: library.dir().join(name),
                cache: library.thumb_path(name),
            };
            if self.send(job) {
                self.pending.insert(name.to_owned());
            }
        }
    }

    /// The thumbnail of `name`: `None` while it is being made, `Some(None)` if it has none.
    pub fn get(&self, name: &str) -> Option<Option<Image>> {
        self.textures.get(name).cloned()
    }

    /// Asks for the preview of `name` blurred by `sigma` thumbnail px (made after the one
    /// being made, if any; only the latest request counts).
    pub fn request_preview(&mut self, library: &Library, name: &str, sigma: f32) {
        if !library.contains(name) {
            return;
        }
        let key = (name.to_owned(), (sigma.max(0.0) * 10.0).round() as u32);
        let shown = self.preview.as_ref().map(|(k, _)| k);
        if shown == Some(&key) || self.preview_pending.as_ref() == Some(&key) {
            self.preview_next = None;
            return;
        }
        if self.preview_pending.is_some() {
            self.preview_next = Some(key);
            return;
        }
        self.send_preview(library, key);
    }

    fn send_preview(&mut self, library: &Library, key: PreviewKey) {
        let job = Job::Preview {
            source: library.dir().join(&key.0),
            cache: library.thumb_path(&key.0),
            key: key.clone(),
        };
        if self.send(job) {
            self.preview_pending = Some(key);
        }
    }

    /// The preview for `name` (blurred), if one was made.
    pub fn preview(&self, name: &str) -> Option<Image> {
        self.preview.as_ref().filter(|(k, _)| k.0 == name).map(|(_, image)| image.clone())
    }

    /// Takes what the thread made.
    pub fn poll(&mut self, library: &Library) -> Arrived {
        let mut arrived = Arrived::default();
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Thumb(name, pixels) => {
                    self.pending.remove(&name);
                    if !library.contains(&name) {
                        continue;
                    }
                    let Some(gpu) = &self.gpu else { continue };
                    self.textures.insert(name, pixels.as_ref().map(|p| gpu.upload(p)));
                    arrived.thumbs = true;
                }
                Done::Preview(key, pixels) => {
                    self.preview_pending = None;
                    if let (Some(gpu), Some(pixels)) = (&self.gpu, pixels) {
                        self.preview = Some((key, gpu.upload(&pixels)));
                        arrived.preview = true;
                    }
                    if let Some(next) = self.preview_next.take() {
                        self.send_preview(library, next);
                    }
                }
            }
        }
        arrived
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("bg-1700000000000-0.jpg"));
        assert!(!valid_name(""));
        assert!(!valid_name(".thumbs"));
        assert!(!valid_name("../secret"));
        assert!(!valid_name("a/b.png"));
        assert!(!valid_name("a..png"));
        assert!(!valid_name("a b.png"));
        assert!(!valid_name(&"a".repeat(80)));
    }

    fn png(path: &Path, w: u32, h: u32) {
        RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]))
            .save_with_format(path, image::ImageFormat::Png)
            .unwrap();
    }

    #[test]
    fn add_remove_migrate() {
        let tmp = std::env::temp_dir().join(format!("axion-bg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let source = tmp.join("Sea Side.PNG");
        png(&source, 64, 40);
        let mut library = Library::new(&tmp.join("home"));
        assert_eq!(library.entries().len(), 0);
        let a = library.add(&source, 2).unwrap();
        assert!(a.ends_with(".png") && valid_name(&a));
        let b = library.add(&source, 2).unwrap();
        assert_ne!(a, b);
        assert!(library.add(&source, 2).is_none(), "the limit holds");
        assert!(library.add(&tmp.join("missing.png"), 9).is_none());
        std::fs::write(tmp.join("text.png"), b"not an image").unwrap();
        assert!(library.add(&tmp.join("text.png"), 9).is_none());
        assert_eq!(library.file(&a).map(|(_, m)| m).as_deref(), Some("image/png"));
        // The source stays where it was; the copy is inside the directory.
        assert!(source.exists());
        assert!(library.file(&a).unwrap().0.starts_with(library.dir()));
        // A thumbnail is made and cached.
        let thumb = thumbnail(&library.dir().join(&a), &library.thumb_path(&a)).unwrap();
        assert_eq!(thumb.dimensions(), (64, 40));
        assert!(library.thumb_path(&a).exists());
        // Hidden files and the thumbnail cache don't count.
        library.scan();
        assert_eq!(library.entries().len(), 2);
        assert!(library.remove(&a));
        assert!(!library.thumb_path(&a).exists());
        assert!(!library.remove(&a));
        assert!(!library.remove("../Sea Side.PNG"));
        // The old single image moves in.
        let legacy = tmp.join("home").join(LEGACY_FILE);
        png(&legacy, 8, 8);
        let migrated = library.migrate(&legacy).unwrap();
        assert!(migrated.ends_with(".png"));
        assert!(!legacy.exists());
        assert!(library.contains(&migrated));
        assert!(library.migrate(&legacy).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_for(Path::new("/x/a.JPEG"), "image/jpeg"), "jpeg");
        assert_eq!(extension_for(Path::new("/x/a"), "image/jpeg"), "jpg");
        assert_eq!(extension_for(Path::new("/x/a.tar~gz"), "image/png"), "png");
    }
}
