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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use axion_compositor::Icon;
use axion_engine::IconLink;
use axion_ui::taffy::prelude::*;
use axion_ui::{Paint, Ui, Widget};

use crate::url::display_host;

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
    let mut ranked: Vec<(u8, usize, &str)> = links
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let rank = match media_scheme(&l.media) {
                Some(s) if s == scheme => 0,
                Some(_) => return None,
                None => 1,
            };
            let touch = u8::from(l.url.contains("apple-touch-icon"));
            Some((rank + 2 * touch, i, l.url.as_str()))
        })
        .collect();
    ranked.sort();
    let mut out: Vec<String> = Vec::new();
    for (_, _, url) in ranked {
        if !out.iter().any(|u| u == url) {
            out.push(url.to_owned());
        }
    }
    out
}

/// The search for one site's icon in one scheme.
#[derive(Default)]
struct Search {
    candidates: VecDeque<String>,
    tried: HashSet<String>,
    in_flight: Option<String>,
    rounds: u32,
}

#[derive(Clone)]
struct Cached {
    icon: SiteIcon,
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
}

impl Favicons {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, dir: Option<PathBuf>) -> Self {
        if let Some(dir) = &dir
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("favicon cache unavailable: {e}");
        }
        Self {
            device,
            queue,
            dir,
            by_key: RefCell::default(),
            not_on_disk: RefCell::default(),
            searches: HashMap::new(),
            links: HashMap::new(),
            scheme: Scheme::Light,
        }
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
        changed
    }

    /// The icon for `page_url`'s site in the current scheme (else the other one's), if known
    /// (in memory or cached on disk).
    pub fn get(&self, page_url: &str) -> Option<SiteIcon> {
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        self.cached(&host, self.scheme).or_else(|| self.cached(&host, self.scheme.other())).map(|c| c.icon)
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
        Some(Cached { icon: self.upload(w, h, image.as_raw()), source })
    }

    /// Chromium reported the page's icon URLs (without media queries): returns the next URL to
    /// fetch, if any.
    pub fn icon_urls(&mut self, page_url: &str, urls: &[String]) -> Option<String> {
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
                    });
                }
            }
        }
        self.resolve(page_url)
    }

    /// The page reported its icon links with their media queries (the engine's page meta):
    /// returns the next URL to fetch, if any.
    pub fn icon_links(&mut self, page_url: &str, links: &[IconLink]) -> Option<String> {
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
        let best_is_general = best.as_deref().is_some_and(general);
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
        let host = display_host(page_url);
        let scheme = self.scheme_fetching(&host, url)?;
        let search = self.searches.get_mut(&(host, scheme))?;
        search.in_flight = None;
        next(search)
    }

    /// The icon from `url` arrived for `page_url`'s site: shown from now on (in the scheme it
    /// was fetched for), and cached on disk if `persist`.
    pub fn insert(&mut self, page_url: &str, url: &str, width: u32, height: u32, rgba: &[u8], persist: bool) {
        let host = display_host(page_url);
        if host.is_empty() {
            return;
        }
        let scheme = self.scheme_fetching(&host, url).unwrap_or(self.scheme);
        let icon = self.upload(width, height, rgba);
        let key = (host.clone(), scheme);
        self.by_key.borrow_mut().insert(key.clone(), Cached { icon, source: Some(url.to_owned()) });
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
        IconLink { url: url.into(), media: media.into(), sizes: String::new() }
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
}
