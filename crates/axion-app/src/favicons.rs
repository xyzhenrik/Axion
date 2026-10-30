//! Site icons, decoded by Chromium and kept as GPU textures per host and colour scheme — and
//! as PNG files in the profile, so a site's icon shows at once the next time.
//!
//! Sites may ship different icons for light and dark pages (`<link rel=icon media=
//! "(prefers-color-scheme: dark)">`, or a script that swaps the link when the scheme changes),
//! so icons are kept per scheme: `<host>.png` (light) and `<host>.dark.png`, each with the
//! address it came from in a `.url` file beside it. A scheme without an icon of its own shows
//! the other scheme's.
//!
//! Finding an icon tries, in order, the page's own icon links for the current scheme,
//! `/favicon.ico` and the Apple touch icons; when all fail it tries once more after the page
//! has finished loading (many pages add their icon links late). When the page's best link for
//! the scheme differs from where the cached icon came from (the site changed it, or the theme
//! switched), it is fetched again. Sites without any icon get a neutral globe.
//!
//! Among the links of a scheme, SVG icons and ones of 32 px or more come first. The cache keeps
//! the site's own pixels; what is drawn is adapted to the chrome (`favicon_contrast`): a dark
//! glyph that would vanish on dark chrome is inverted or put on a tile, per host and scheme,
//! and recomputed when the theme or the setting changes. SVG icons with `prefers-color-scheme`
//! rules inside are rewritten for the chrome's scheme before Chromium rasterises them
//! (`Favicons::alias` ties the rewritten `data:` URL back to the icon's address).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axion_compositor::Icon;
use axion_config::FaviconContrast;
use axion_engine::IconLink;
use axion_ui::taffy::prelude::*;
use axion_ui::{Paint, Ui, Widget};

use crate::favicon_contrast;
use crate::url::display_host;

/// 64 px PNG of the app icon: the site icon of every `axion://` page (start page, settings, ...).
pub(crate) static APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/hicolor/64x64/apps/axion.png");

/// Whether `url` is one of Axion's own pages, which show the app icon instead of a site icon.
pub fn is_internal(url: &str) -> bool {
    url.starts_with("axion://")
}

/// Requested icon size in pixels (shown at 16–18 px logical, crisp up to 3× scale).
pub const FETCH_SIZE: u32 = 64;
/// Complete passes over the candidates per site and session (the second after loading).
const MAX_ROUNDS: u32 = 2;

pub(crate) type SiteIcon = (wgpu::TextureView, (u32, u32));

/// The page colour scheme an icon is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Light,
    Dark,
}

impl Scheme {
    pub fn of(dark: bool) -> Self {
        if dark { Scheme::Dark } else { Scheme::Light }
    }

    fn other(self) -> Self {
        match self {
            Scheme::Light => Scheme::Dark,
            Scheme::Dark => Scheme::Light,
        }
    }
}

/// The scheme a link's `media` query is limited to (`None`: both).
fn media_scheme(media: &str) -> Option<Scheme> {
    let m: String = media.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if m.contains("prefers-color-scheme:dark") {
        Some(Scheme::Dark)
    } else if m.contains("prefers-color-scheme:light") {
        Some(Scheme::Light)
    } else {
        None
    }
}

/// The page's icon links usable in `scheme`, best first: links made for the scheme, then the
/// general ones; Apple touch icons last.
fn links_for(links: &[IconLink], scheme: Scheme) -> Vec<String> {
    let mut ranked: Vec<(u8, u8, usize, &str)> = links
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let rank = match media_scheme(&l.media) {
                Some(s) if s == scheme => 0,
                Some(_) => return None,
                None => 1,
            };
            let touch = u8::from(l.url.contains("apple-touch-icon"));
            Some((rank + 2 * touch, size_class(l), i, l.url.as_str()))
        })
        .collect();
    ranked.sort();
    let mut out: Vec<String> = Vec::new();
    for (_, _, _, url) in ranked {
        if !out.iter().any(|u| u == url) {
            out.push(url.to_owned());
        }
    }
    out
}

/// Whether an icon link is an SVG (by its `type`, `sizes="any"` or the address).
pub fn is_svg_link(link: &IconLink) -> bool {
    let path = link.url.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    link.mime.to_ascii_lowercase().contains("svg") || path.ends_with(".svg")
}

/// How well a link's size suits a 16–18 px slot at up to 3×: 0 scalable, 1 at least 32 px (or
/// unknown), 2 smaller.
fn size_class(link: &IconLink) -> u8 {
    if is_svg_link(link) || link.sizes.trim().eq_ignore_ascii_case("any") {
        return 0;
    }
    let largest = link
        .sizes
        .split_whitespace()
        .filter_map(|s| {
            let (w, h) = s.to_ascii_lowercase().split_once('x').map(|(w, h)| (w.to_owned(), h.to_owned()))?;
            Some(w.parse::<u32>().ok()?.max(h.parse::<u32>().ok()?))
        })
        .max();
    match largest {
        Some(side) if side < 32 => 2,
        _ => 1,
    }
}

/// The search for one site's icon in one scheme.
#[derive(Default)]
struct Search {
    candidates: VecDeque<String>,
    tried: HashSet<String>,
    in_flight: Option<String>,
    rounds: u32,
}

/// Straight-alpha RGBA pixels.
struct Raw {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Clone)]
struct Cached {
    icon: SiteIcon,
    /// The icon's pixels as the site made them (for adapting it to the chrome).
    raw: Arc<Raw>,
    /// Where it was downloaded from (unknown for icons cached by older versions).
    source: Option<String>,
}

/// What a site's pages said about their icons.
#[derive(Default)]
struct SiteLinks {
    links: Vec<IconLink>,
    /// The page meta script reported (with media queries): Chromium's own list, which lacks
    /// them, is no longer merged in.
    from_meta: bool,
}

type Key = (String, Scheme);

pub struct Favicons {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// `<profile>/favicons`, see the module docs.
    dir: Option<PathBuf>,
    by_key: RefCell<HashMap<Key, Cached>>,
    /// Hosts with no icon on disk for a scheme (so the disk isn't asked again).
    not_on_disk: RefCell<HashSet<Key>>,
    searches: HashMap<Key, Search>,
    links: HashMap<String, SiteLinks>,
    scheme: Scheme,
    contrast: FaviconContrast,
    /// What is drawn for a cached icon on the current chrome (its adapted look), by key of the
    /// cached icon; emptied when the scheme or the setting changes.
    shown: RefCell<HashMap<Key, SiteIcon>>,
    /// Rewritten `data:` URLs of SVG icons -> the address they stand for.
    aliases: HashMap<String, String>,
    /// The app icon, the icon of all `axion://` pages.
    app_icon: Option<SiteIcon>,
}

impl Favicons {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, dir: Option<PathBuf>) -> Self {
        if let Some(dir) = &dir
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("favicon cache unavailable: {e}");
        }
        let mut this = Self {
            device,
            queue,
            dir,
            by_key: RefCell::default(),
            not_on_disk: RefCell::default(),
            searches: HashMap::new(),
            links: HashMap::new(),
            scheme: Scheme::Light,
            contrast: FaviconContrast::default(),
            shown: RefCell::default(),
            aliases: HashMap::new(),
            app_icon: None,
        };
        this.app_icon = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png)
            .inspect_err(|e| tracing::warn!("app icon as site icon: {e}"))
            .ok()
            .map(|img| {
                let img = img.into_rgba8();
                this.upload(img.width(), img.height(), img.as_raw())
            });
        this
    }

    /// Icon files were added to the cache folder by someone else (an import): hosts that had
    /// none on disk are looked up again.
    pub fn rescan_disk(&self) {
        self.not_on_disk.borrow_mut().clear();
    }

    /// Where the icons are cached (`None`: nowhere, they live in memory only).
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Switches the colour scheme icons are shown and searched for. Returns whether it changed
    /// (then [`Favicons::resolve`] each open page to fetch the new scheme's icons).
    pub fn set_dark(&mut self, dark: bool) -> bool {
        let scheme = Scheme::of(dark);
        let changed = scheme != self.scheme;
        self.scheme = scheme;
        if changed {
            self.shown.borrow_mut().clear();
        }
        changed
    }

    /// Sets what happens to icons that would vanish on the chrome. Returns whether it changed.
    pub fn set_contrast(&mut self, mode: FaviconContrast) -> bool {
        let changed = mode != self.contrast;
        self.contrast = mode;
        if changed {
            self.shown.borrow_mut().clear();
        }
        changed
    }

    /// Whether the chrome the icons are shown on is dark.
    pub fn is_dark(&self) -> bool {
        self.scheme == Scheme::Dark
    }

    /// Whether `url` is, by what the pages reported, an SVG icon of `page_url`'s site.
    pub fn is_svg(&self, page_url: &str, url: &str) -> bool {
        let known = self
            .links
            .get(&display_host(page_url))
            .is_some_and(|s| s.links.iter().any(|l| l.url == url && is_svg_link(l)));
        known || is_svg_link(&IconLink { url: url.to_owned(), ..IconLink::default() })
    }

    /// `derived` (a rewritten copy of an icon, fetched instead of it) stands for `original`.
    pub fn alias(&mut self, derived: &str, original: &str) {
        self.aliases.insert(derived.to_owned(), original.to_owned());
    }

    fn canonical(&self, url: &str) -> String {
        self.aliases.get(url).cloned().unwrap_or_else(|| url.to_owned())
    }

    /// The icon for `page_url`'s site in the current scheme (else the other one's), if known
    /// (in memory or cached on disk).
    pub fn get(&self, page_url: &str) -> Option<SiteIcon> {
        if is_internal(page_url) {
            return self.app_icon.clone();
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        let (scheme, cached) = self
            .cached(&host, self.scheme)
            .map(|c| (self.scheme, c))
            .or_else(|| self.cached(&host, self.scheme.other()).map(|c| (self.scheme.other(), c)))?;
        Some(self.shown_icon((host, scheme), &cached))
    }

    /// The site's own icon pixels for `page_url` (current scheme, else the other), for drawing
    /// outside the GPU frame (the drag image). `None` while no icon is known.
    pub fn pixels(&self, page_url: &str) -> Option<image::RgbaImage> {
        if is_internal(page_url) {
            return image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png)
                .ok()
                .map(image::DynamicImage::into_rgba8);
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        let cached = self.cached(&host, self.scheme).or_else(|| self.cached(&host, self.scheme.other()))?;
        image::RgbaImage::from_raw(cached.raw.width, cached.raw.height, cached.raw.rgba.clone())
    }

    /// The cached icon as drawn on the current chrome: adapted when it would fade into it.
    fn shown_icon(&self, key: Key, cached: &Cached) -> SiteIcon {
        if let Some(shown) = self.shown.borrow().get(&key) {
            return shown.clone();
        }
        let raw = &cached.raw;
        let icon = favicon_contrast::adapt(&raw.rgba, raw.width, raw.height, self.is_dark(), self.contrast)
            .map_or_else(|| cached.icon.clone(), |(w, h, rgba)| self.upload(w, h, &rgba));
        self.shown.borrow_mut().insert(key, icon.clone());
        icon
    }

    fn cached(&self, host: &str, scheme: Scheme) -> Option<Cached> {
        let key = (host.to_owned(), scheme);
        if let Some(cached) = self.by_key.borrow().get(&key) {
            return Some(cached.clone());
        }
        if self.not_on_disk.borrow().contains(&key) {
            return None;
        }
        let cached = self.load_from_disk(host, scheme);
        match &cached {
            Some(cached) => {
                self.by_key.borrow_mut().insert(key, cached.clone());
            }
            None => {
                self.not_on_disk.borrow_mut().insert(key);
            }
        }
        cached
    }

    fn cache_path(&self, host: &str, scheme: Scheme) -> Option<PathBuf> {
        Some(cache_file(self.dir.as_ref()?, host, scheme == Scheme::Dark))
    }

    fn load_from_disk(&self, host: &str, scheme: Scheme) -> Option<Cached> {
        let path = self.cache_path(host, scheme)?;
        let image = image::open(&path).ok()?.into_rgba8();
        let (w, h) = image.dimensions();
        let source = std::fs::read_to_string(source_file(&path)).ok().map(|s| s.trim().to_owned());
        let icon = self.upload(w, h, image.as_raw());
        Some(Cached { icon, raw: Arc::new(Raw { width: w, height: h, rgba: image.into_raw() }), source })
    }

    /// Chromium reported the page's icon URLs (without media queries): returns the next URL to
    /// fetch, if any.
    pub fn icon_urls(&mut self, page_url: &str, urls: &[String]) -> Option<String> {
        if is_internal(page_url) {
            return None;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        let site = self.links.entry(host).or_default();
        if !site.from_meta {
            for url in urls {
                if !site.links.iter().any(|l| &l.url == url) {
                    site.links.push(IconLink {
                        url: url.clone(),
                        media: String::new(),
                        sizes: String::new(),
                        mime: String::new(),
                    });
                }
            }
        }
        self.resolve(page_url)
    }

    /// The page reported its icon links with their media queries (the engine's page meta):
    /// returns the next URL to fetch, if any.
    pub fn icon_links(&mut self, page_url: &str, links: &[IconLink]) -> Option<String> {
        if is_internal(page_url) {
            return None;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        self.links.insert(host, SiteLinks { links: links.to_vec(), from_meta: true });
        self.resolve(page_url)
    }

    /// Checks `page_url`'s site against the links its pages reported, for the current scheme:
    /// returns a URL to fetch when the site has no icon for it yet, or its best link changed.
    pub fn resolve(&mut self, page_url: &str) -> Option<String> {
        if is_internal(page_url) {
            return None;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        let scheme = self.scheme;
        let site = self.links.get(&host);
        let links = site.map(|s| links_for(&s.links, scheme)).unwrap_or_default();
        let best = links.first().cloned();
        let general = |url: &str| {
            site.is_some_and(|s| s.links.iter().any(|l| l.url == url && media_scheme(&l.media).is_none()))
        };
        // An SVG may look different per scheme (rules inside it), so it never serves both.
        let svg = |url: &str| site.is_some_and(|s| s.links.iter().any(|l| l.url == url && is_svg_link(l)));
        let best_is_general = best.as_deref().is_some_and(|b| general(b) && !svg(b));
        let own = self.cached(&host, scheme);
        match (&own, &best) {
            (Some(cached), Some(best)) if cached.source.as_ref() == Some(best) => return None,
            (Some(_), None) => return None,
            // A general link the other scheme's icon came from: that one serves both.
            (None, Some(best))
                if best_is_general
                    && self.cached(&host, scheme.other()).and_then(|c| c.source).as_ref() == Some(best) =>
            {
                return None;
            }
            _ => {}
        }
        let search = self.searches.entry((host, scheme)).or_default();
        if own.is_some() {
            // A newer or more fitting icon than the cached one: only the page's best link,
            // once per session.
            let best = best?;
            if search.tried.contains(&best) {
                return None;
            }
            search.candidates.retain(|c| c != &best);
            search.candidates.push_front(best);
            return next(search);
        }
        // New links go first; candidates already tried are skipped.
        for link in links.iter().rev() {
            if !search.tried.contains(link) && !search.candidates.contains(link) {
                search.candidates.push_front(link.clone());
            }
        }
        if search.rounds == 0 {
            search.rounds = 1;
            for fallback in fallback_urls(page_url) {
                if !search.candidates.contains(&fallback) {
                    search.candidates.push_back(fallback);
                }
            }
        }
        next(search)
    }

    /// The page finished loading: if its site still has no icon, searches once more.
    pub fn page_loaded(&mut self, page_url: &str) -> Option<String> {
        if is_internal(page_url) {
            return None;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        if self.cached(&host, self.scheme).is_some() {
            return self.resolve(page_url);
        }
        let search = self.searches.entry((host, self.scheme)).or_default();
        if search.rounds == 0 {
            // Loaded before the page reported its icon links: the first round starts now.
            return self.resolve(page_url);
        }
        if search.in_flight.is_some() || !search.candidates.is_empty() || search.rounds >= MAX_ROUNDS {
            return None;
        }
        search.rounds += 1;
        search.tried.clear();
        search.candidates.extend(fallback_urls(page_url));
        next(search)
    }

    /// The scheme whose search has `url` in flight for `host`.
    fn scheme_fetching(&self, host: &str, url: &str) -> Option<Scheme> {
        [self.scheme, self.scheme.other()].into_iter().find(|s| {
            self.searches.get(&(host.to_owned(), *s)).is_some_and(|x| x.in_flight.as_deref() == Some(url))
        })
    }

    /// Fetching `url` failed: the next candidate to try, if any.
    pub fn failed(&mut self, page_url: &str, url: &str) -> Option<String> {
        let url = &self.canonical(url);
        let host = display_host(page_url);
        let scheme = self.scheme_fetching(&host, url)?;
        let search = self.searches.get_mut(&(host, scheme))?;
        search.in_flight = None;
        next(search)
    }

    /// The icon from `url` arrived for `page_url`'s site: shown from now on (in the scheme it
    /// was fetched for), and cached on disk if `persist`.
    pub fn insert(&mut self, page_url: &str, url: &str, width: u32, height: u32, rgba: &[u8], persist: bool) {
        let url = &self.canonical(url);
        if is_internal(page_url) {
            return;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return;
        }
        let scheme = self.scheme_fetching(&host, url).unwrap_or(self.scheme);
        let icon = self.upload(width, height, rgba);
        let key = (host.clone(), scheme);
        let raw = Arc::new(Raw { width, height, rgba: rgba.to_vec() });
        self.shown.borrow_mut().remove(&key);
        self.by_key.borrow_mut().insert(key.clone(), Cached { icon, raw, source: Some(url.to_owned()) });
        self.not_on_disk.borrow_mut().remove(&key);
        if let Some(search) = self.searches.get_mut(&key) {
            // Done; what was tried stays tried (no fetching in circles).
            search.in_flight = None;
            search.candidates.clear();
        }
        if persist
            && let Some(path) = self.cache_path(&host, scheme)
            && let Some(image) = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        {
            if let Err(e) = image.save(&path) {
                tracing::debug!("caching the icon of {host} failed: {e}");
            } else if let Err(e) = std::fs::write(source_file(&path), url) {
                tracing::debug!("noting where the icon of {host} came from failed: {e}");
            }
        }
    }

    pub(crate) fn upload(&self, width: u32, height: u32, rgba: &[u8]) -> SiteIcon {
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("favicon"),
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
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        (texture.create_view(&Default::default()), (width, height))
    }
}

/// Takes the next untried candidate and marks it in flight (none while one is in flight).
fn next(search: &mut Search) -> Option<String> {
    if search.in_flight.is_some() {
        return None;
    }
    while let Some(url) = search.candidates.pop_front() {
        if search.tried.insert(url.clone()) {
            search.in_flight = Some(url.clone());
            return Some(url);
        }
    }
    None
}

/// The cache file of a host's icon (anything but letters, digits, '.' and '-' becomes '_').
fn cache_file_name(host: &str, dark: bool) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    if dark { format!("{safe}.dark.png") } else { format!("{safe}.png") }
}

/// Where the icon of `host` for light or dark pages is cached in `dir`.
pub fn cache_file(dir: &Path, host: &str, dark: bool) -> PathBuf {
    dir.join(cache_file_name(host, dark))
}

/// The file next to a cached icon that holds the address it came from.
fn source_file(icon: &Path) -> PathBuf {
    let mut name = icon.as_os_str().to_owned();
    name.push(".url");
    PathBuf::from(name)
}

/// The conventional icon locations of a page's site: `/favicon.ico` and the Apple touch icons.
pub fn fallback_urls(page_url: &str) -> Vec<String> {
    let Some((scheme, rest)) = page_url.split_once("://") else { return Vec::new() };
    if scheme != "http" && scheme != "https" {
        return Vec::new();
    }
    let Some(host) = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty()) else { return Vec::new() };
    ["favicon.ico", "apple-touch-icon.png", "apple-touch-icon-precomposed.png"]
        .iter()
        .map(|file| format!("{scheme}://{host}/{file}"))
        .collect()
}

/// A favicon of `side` logical pixels: the site's icon, or a neutral globe while it has none.
pub fn favicon_node(ui: &mut Ui, parent: NodeId, url: &str, side: f32, favicons: &Favicons) -> NodeId {
    favicon_or(ui, parent, url, side, favicons, Icon::Globe)
}

/// A favicon of `side` logical pixels, or `fallback` (secondary colour) while the site has none.
pub fn favicon_or(
    ui: &mut Ui,
    parent: NodeId,
    url: &str,
    side: f32,
    favicons: &Favicons,
    fallback: Icon,
) -> NodeId {
    let style = Style {
        size: Size { width: length(side), height: length(side) },
        flex_shrink: 0.0,
        ..Default::default()
    };
    match favicons.get(url) {
        Some(image) => {
            let node = ui.add(parent, Widget::image().with_radius(side * 0.22), style);
            ui.set_image(node, Some(image));
            node
        }
        None => ui.add(parent, Widget::icon(fallback, side).with_paint(Paint::Secondary), style),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_pages_are_recognised() {
        assert!(is_internal("axion://home/"));
        assert!(is_internal("axion://settings/#general"));
        assert!(!is_internal("https://axion.example/"));
        assert!(!is_internal("about:blank"));
        let img = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));
    }

    #[test]
    fn fallback_icon_locations() {
        assert_eq!(
            fallback_urls("https://de.wikipedia.org/wiki/X?y=1"),
            [
                "https://de.wikipedia.org/favicon.ico",
                "https://de.wikipedia.org/apple-touch-icon.png",
                "https://de.wikipedia.org/apple-touch-icon-precomposed.png",
            ]
        );
        assert!(fallback_urls("about:blank").is_empty());
        assert!(fallback_urls("file:///tmp/a.html").is_empty());
    }

    #[test]
    fn candidates_in_order_without_repeats() {
        let mut search = Search::default();
        search.candidates.extend(["a".to_owned(), "b".to_owned(), "a".to_owned()]);
        assert_eq!(next(&mut search).as_deref(), Some("a"));
        assert_eq!(next(&mut search), None, "one at a time");
        search.in_flight = None;
        assert_eq!(next(&mut search).as_deref(), Some("b"));
        search.in_flight = None;
        assert_eq!(next(&mut search), None, "\"a\" was tried already");
    }

    #[test]
    fn cache_file_names() {
        assert_eq!(cache_file_name("de.wikipedia.org", false), "de.wikipedia.org.png");
        assert_eq!(cache_file_name("github.com", true), "github.com.dark.png");
        assert_eq!(cache_file_name("localhost:8765", false), "localhost_8765.png");
        assert_eq!(cache_file_name("../etc", false), ".._etc.png");
        assert_eq!(source_file(Path::new("/a/b.png")), Path::new("/a/b.png.url"));
    }

    fn link(url: &str, media: &str) -> IconLink {
        IconLink { url: url.into(), media: media.into(), ..IconLink::default() }
    }

    #[test]
    fn links_per_scheme() {
        let links = [
            link("https://x/apple-touch-icon.png", ""),
            link("https://x/all.ico", ""),
            link("https://x/light.svg", "(prefers-color-scheme: light)"),
            link("https://x/dark.svg", "(prefers-color-scheme:dark)"),
        ];
        assert_eq!(
            links_for(&links, Scheme::Dark),
            ["https://x/dark.svg", "https://x/all.ico", "https://x/apple-touch-icon.png"]
        );
        assert_eq!(links_for(&links, Scheme::Light)[0], "https://x/light.svg");
        assert_eq!(media_scheme("screen and (prefers-color-scheme: DARK)"), Some(Scheme::Dark));
        assert_eq!(media_scheme("(min-width: 10px)"), None);
    }

    fn sized(url: &str, sizes: &str, mime: &str) -> IconLink {
        IconLink { url: url.into(), sizes: sizes.into(), mime: mime.into(), ..IconLink::default() }
    }

    #[test]
    fn scalable_and_large_icons_come_first() {
        let links = [
            sized("https://x/16.png", "16x16", "image/png"),
            sized("https://x/48.png", "48x48", "image/png"),
            sized("https://x/icon.svg", "any", "image/svg+xml"),
            sized("https://x/apple-touch-icon.png", "180x180", ""),
        ];
        assert_eq!(
            links_for(&links, Scheme::Light),
            ["https://x/icon.svg", "https://x/48.png", "https://x/16.png", "https://x/apple-touch-icon.png"]
        );
        assert!(is_svg_link(&sized("https://x/a.SVG?v=2", "", "")));
        assert!(is_svg_link(&sized("https://x/a", "", "image/svg+xml")));
        assert!(!is_svg_link(&sized("https://x/a.png", "", "")));
    }

    #[test]
    fn dark_variant_beats_general_icon_in_dark_chrome_only() {
        let links = [
            sized("https://x/general.png", "64x64", ""),
            IconLink {
                media: "(prefers-color-scheme: dark)".into(),
                ..sized("https://x/dark.svg", "any", "")
            },
            IconLink {
                media: "(prefers-color-scheme: light)".into(),
                ..sized("https://x/light.svg", "any", "")
            },
        ];
        assert_eq!(links_for(&links, Scheme::Dark), ["https://x/dark.svg", "https://x/general.png"]);
        assert_eq!(links_for(&links, Scheme::Light), ["https://x/light.svg", "https://x/general.png"]);
    }

    #[test]
    fn imported_icons_use_the_cache_file_names() {
        for host in ["de.wikipedia.org", "localhost:8765", "../etc", "a b"] {
            assert_eq!(axion_import::icon_file_name(host), cache_file_name(host, false));
        }
        for url in ["https://www.github.com/a", "http://u@x.org:81/b?c", "https://de.wikipedia.org/wiki/X"] {
            assert_eq!(axion_import::host_key(url).as_deref(), Some(display_host(url).as_str()));
        }
    }

    #[test]
    fn cache_files_differ_per_scheme() {
        let dir = Path::new("/c");
        assert_ne!(cache_file(dir, "github.com", false), cache_file(dir, "github.com", true));
        assert_eq!(cache_file(dir, "github.com", true), Path::new("/c/github.com.dark.png"));
    }
}
