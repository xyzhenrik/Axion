//! Site icons of other browsers: choosing, decoding and normalising them, and writing them
//! into Axion's icon cache (`<profile>/favicons/<host>.png`, the files the browser's own
//! icon cache reads, so imported icons show at once like ones loaded while browsing).
//!
//! Everything read here is untrusted: blobs are size-checked before and after decoding, the
//! decoder runs with allocation and dimension limits, and a broken image is skipped, never an
//! error. Icons are re-encoded as small PNGs. SVG icons cannot be rasterised here and are
//! skipped (the host falls back to another icon of the site, or to the globe until a visit).

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits as DecodeLimits};

use crate::model::{ImportBookmark, ImportHistory, Limits};

/// Icons are stored at most this many pixels per side (Axion fetches 64 px icons itself).
const MAX_SIDE: u32 = 64;
/// Images wider or taller than this are not decoded at all.
const MAX_DECODE_SIDE: u32 = 1024;
/// Decoder allocation cap (bytes).
const MAX_DECODE_ALLOC: u64 = 32 * 1024 * 1024;
/// Candidate bitmaps tried per host before giving up.
const MAX_TRIES: usize = 4;
/// The side an icon should be close to.
const IDEAL_SIDE: u32 = 32;

/// A site icon ready for Axion's cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportIcon {
    /// The cache key: the host as Axion shows it (`www.` removed, port kept).
    pub host: String,
    /// A PNG, at most 64 px per side.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The host of a web address the way Axion keys its icon cache (`www.` removed, port kept);
/// `None` for anything but `http(s)` addresses.
pub fn host_key(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty() && host.len() <= 255).then(|| host.to_owned())
}

/// The cache file name of a host's icon for light pages (anything but letters, digits, '.' and
/// '-' becomes '_'). Must stay the same as the browser's icon cache.
pub fn icon_file_name(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    format!("{safe}.png")
}

/// A page address without its fragment, for comparing pages between browsers.
fn page_key(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

/// The pages whose icons are wanted: the imported bookmarks first, then history (most recent
/// first), de-duplicated by host.
#[derive(Debug, Default)]
pub struct WantedIcons {
    /// Hosts in priority order, as Axion keys them.
    pub hosts: Vec<String>,
    /// Lowercase host -> index into `hosts`.
    by_host: HashMap<String, usize>,
    /// Exact pages (without fragment).
    pages: HashSet<String>,
}

impl WantedIcons {
    pub fn from_imported(bookmarks: &[ImportBookmark], history: &[ImportHistory]) -> Self {
        let mut wanted = WantedIcons::default();
        for url in bookmarks.iter().map(|b| b.url.as_str()).chain(history.iter().map(|h| h.url.as_str())) {
            wanted.add(url);
        }
        wanted
    }

    pub fn add(&mut self, url: &str) {
        let Some(host) = host_key(url) else { return };
        let lower = host.to_ascii_lowercase();
        if !self.by_host.contains_key(&lower) {
            self.by_host.insert(lower, self.hosts.len());
            self.hosts.push(host);
        }
        self.pages.insert(page_key(url).to_owned());
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// The index of the wanted host a page of another browser belongs to.
    pub fn host_of(&self, page_url: &str) -> Option<usize> {
        let host = host_key(page_url)?.to_ascii_lowercase();
        self.by_host.get(&host).copied()
    }

    /// Whether this exact page was imported.
    pub fn has_page(&self, page_url: &str) -> bool {
        self.pages.contains(page_key(page_url))
    }
}

/// An icon image in another browser's database, not read yet.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The row to read the image from.
    pub id: i64,
    /// Its side in pixels as the database says (0: unknown).
    pub side: u32,
    /// Stored for exactly the page that was imported.
    pub exact: bool,
}

impl Candidate {
    /// Smaller is better: icons for the exact page first, then sizes close to 32 px (16 px and
    /// up before anything smaller; unknown sizes in the middle).
    fn rank(&self) -> (bool, u8, u32) {
        let (class, distance) = match self.side {
            0 => (1, 0),
            s if s < 16 => (2, 16 - s),
            s => (0, s.abs_diff(IDEAL_SIDE)),
        };
        (!self.exact, class, distance)
    }
}

/// Candidates per wanted host (index into [`WantedIcons::hosts`]).
#[derive(Debug, Default)]
pub struct Candidates {
    by_host: HashMap<usize, Vec<Candidate>>,
}

impl Candidates {
    /// Adds an image for a page of another browser. Images of zero size or larger than
    /// `limits.max_favicon_bytes` are dropped here.
    pub fn add(
        &mut self,
        wanted: &WantedIcons,
        page_url: &str,
        id: i64,
        side: u32,
        bytes: i64,
        limits: &Limits,
    ) {
        if bytes <= 0 || bytes as u64 > limits.max_favicon_bytes as u64 {
            return;
        }
        let Some(host) = wanted.host_of(page_url) else { return };
        let list = self.by_host.entry(host).or_default();
        if list.iter().any(|c| c.id == id) {
            return;
        }
        list.push(Candidate { id, side, exact: wanted.has_page(page_url) });
    }

    /// Reads the best working image of every host with `fetch` (a database lookup by row id),
    /// at most `limits.max_favicons` icons, hosts in the order they were wanted.
    pub fn resolve(
        mut self,
        wanted: &WantedIcons,
        limits: &Limits,
        mut fetch: impl FnMut(i64) -> Option<Vec<u8>>,
    ) -> Vec<ImportIcon> {
        let mut out = Vec::new();
        for (index, host) in wanted.hosts.iter().enumerate() {
            if out.len() >= limits.max_favicons {
                break;
            }
            let Some(mut list) = self.by_host.remove(&index) else { continue };
            list.sort_by_key(Candidate::rank);
            for candidate in list.iter().take(MAX_TRIES) {
                let Some(blob) = fetch(candidate.id) else { continue };
                if let Some((png, width, height)) = normalize(&blob, limits.max_favicon_bytes) {
                    out.push(ImportIcon { host: host.clone(), png, width, height });
                    break;
                }
            }
        }
        out
    }
}

/// Decodes an icon image (PNG, ICO, JPEG, GIF, BMP, WebP; found out by content) and returns it
/// as a PNG of at most 64 px per side with its size. `None` for anything broken, empty, fully
/// transparent, too big or of a format that cannot be decoded (SVG).
pub fn normalize(blob: &[u8], max_bytes: usize) -> Option<(Vec<u8>, u32, u32)> {
    if blob.is_empty() || blob.len() > max_bytes {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(blob)).with_guessed_format().ok()?;
    let mut limits = DecodeLimits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let mut rgba = image.into_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 || rgba.pixels().all(|p| p.0[3] == 0) {
        return None;
    }
    if w.max(h) > MAX_SIDE {
        let scale = f64::from(MAX_SIDE) / f64::from(w.max(h));
        let nw = ((f64::from(w) * scale).round() as u32).max(1);
        let nh = ((f64::from(h) * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, nw, nh, FilterType::Lanczos3);
    }
    let (w, h) = rgba.dimensions();
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(rgba).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).ok()?;
    Some((png, w, h))
}

/// The image of a `data:image/…;base64,…` address (the `ICON` attribute of exported bookmark
/// files); `None` for any other kind of address or content.
pub fn decode_data_uri(uri: &str, max_bytes: usize) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let uri = uri.trim();
    if !uri.get(..5)?.eq_ignore_ascii_case("data:") {
        return None;
    }
    let (meta, data) = uri[5..].split_once(',')?;
    let meta = meta.to_ascii_lowercase();
    let mut parts = meta.split(';').map(str::trim);
    let mime = parts.next()?;
    // SVG and anything that is no image is of no use here.
    if !mime.starts_with("image/") || mime.contains("svg") || !parts.any(|p| p == "base64") {
        return None;
    }
    // Base64 is 4/3 the size of its content.
    if data.len() > max_bytes.saturating_mul(4) / 3 + 16 {
        return None;
    }
    let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('=')).ok()?;
    (bytes.len() <= max_bytes).then_some(bytes)
}

/// What [`write`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IconReport {
    pub added: usize,
    /// Hosts that had an icon already (kept: it was loaded by browsing).
    pub existing: usize,
}

/// Writes `icons` as `<dir>/<host>.png`. A host that already has an icon keeps it. Failures to
/// write are logged and skipped.
pub fn write(dir: &Path, icons: &[ImportIcon]) -> IconReport {
    let mut report = IconReport::default();
    if icons.is_empty() {
        return report;
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("site icon folder unavailable: {e}");
        return report;
    }
    for icon in icons {
        let path = dir.join(icon_file_name(&icon.host));
        if path.exists() {
            report.existing += 1;
            continue;
        }
        // Written beside and renamed, so a half-written file is never read as an icon.
        let tmp = path.with_extension("png.tmp");
        match std::fs::write(&tmp, &icon.png).and_then(|()| std::fs::rename(&tmp, &path)) {
            Ok(()) => report.added += 1,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                tracing::debug!("writing an imported site icon failed: {e}");
            }
        }
    }
    report
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn png(side: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(side, side, image::Rgba(rgba));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn hosts_are_keyed_like_the_browser_does() {
        assert_eq!(host_key("https://www.rust-lang.org/x?y#z").as_deref(), Some("rust-lang.org"));
        assert_eq!(host_key("http://user@localhost:8080/").as_deref(), Some("localhost:8080"));
        assert_eq!(host_key("ftp://example.com/"), None);
        assert_eq!(host_key("file:///tmp/a"), None);
        assert_eq!(icon_file_name("localhost:8080"), "localhost_8080.png");
        assert_eq!(icon_file_name("../etc"), ".._etc.png");
    }

    #[test]
    fn size_ranking_prefers_32_px_and_the_exact_page() {
        let c = |side, exact| Candidate { id: 0, side, exact };
        assert!(c(32, false).rank() < c(64, false).rank());
        assert!(c(16, false).rank() < c(8, false).rank());
        assert!(c(48, false).rank() < c(8, false).rank());
        assert!(c(8, true).rank() < c(32, false).rank());
        assert!(c(16, false).rank() < c(0, false).rank());
    }

    #[test]
    fn normalize_rejects_garbage_and_scales_down() {
        assert!(normalize(b"", 1000).is_none());
        assert!(normalize(b"not an image at all", 1000).is_none());
        let good = png(16, [10, 20, 30, 255]);
        assert!(normalize(&good, 10).is_none(), "over the byte limit");
        let (_, w, h) = normalize(&good, 100_000).unwrap();
        assert_eq!((w, h), (16, 16));
        let (out, w, h) = normalize(&png(256, [1, 2, 3, 255]), 1 << 20).unwrap();
        assert_eq!((w, h), (64, 64));
        assert!(image::load_from_memory(&out).is_ok());
        assert!(normalize(&png(8, [0, 0, 0, 0]), 100_000).is_none(), "fully transparent");
        let mut truncated = good.clone();
        truncated.truncate(truncated.len() / 2);
        assert!(normalize(&truncated, 100_000).is_none());
        assert!(normalize(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#, 1000).is_none());
    }

    #[test]
    fn data_uris() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png(16, [9, 9, 9, 255]));
        let uri = format!("data:image/png;base64,{b64}");
        assert!(decode_data_uri(&uri, 100_000).is_some());
        assert!(decode_data_uri(&uri, 10).is_none());
        assert!(decode_data_uri(&format!("DATA:IMAGE/PNG;BASE64,{b64}"), 100_000).is_some());
        assert!(decode_data_uri("data:image/svg+xml;base64,PHN2Zz4=", 100_000).is_none());
        assert!(decode_data_uri("data:text/html;base64,PGI+", 100_000).is_none());
        assert!(decode_data_uri("data:image/png,rawbytes", 100_000).is_none());
        assert!(decode_data_uri("https://x/y.png", 100_000).is_none());
        assert!(decode_data_uri("data:image/png;base64,!!!", 100_000).is_none());
    }

    #[test]
    fn writing_keeps_existing_icons() {
        let dir = tempfile::tempdir().unwrap();
        let icon = |host: &str| ImportIcon {
            host: host.into(),
            png: png(16, [1, 1, 1, 255]),
            width: 16,
            height: 16,
        };
        std::fs::write(dir.path().join("old.example.png"), b"mine").unwrap();
        let report = write(dir.path(), &[icon("old.example"), icon("new.example"), icon("localhost:80")]);
        assert_eq!(report, IconReport { added: 2, existing: 1 });
        assert_eq!(std::fs::read(dir.path().join("old.example.png")).unwrap(), b"mine");
        assert!(dir.path().join("localhost_80.png").is_file());
        assert!(!dir.path().join("new.example.png.tmp").exists());
    }
}
