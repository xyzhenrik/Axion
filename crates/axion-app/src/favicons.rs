//! Site icons, decoded by Chromium and kept as GPU textures per host — and as PNG files in the
//! profile, so a site's icon shows at once the next time.
//!
//! Finding an icon tries, in order, the page's own icon links, `/favicon.ico` and the Apple
//! touch icons; when all fail it tries once more after the page has finished loading (many
//! pages add their icon links late). Sites without any icon get a neutral globe.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use axion_compositor::Icon;
use axion_ui::taffy::prelude::*;
use axion_ui::{Paint, Ui, Widget};

use crate::url::display_host;

/// Requested icon size in pixels (shown at 16–18 px logical, crisp up to 3× scale).
pub const FETCH_SIZE: u32 = 64;
/// Complete passes over the candidates per site and session (the second after loading).
const MAX_ROUNDS: u32 = 2;

type SiteIcon = (wgpu::TextureView, (u32, u32));

/// The search for one site's icon.
#[derive(Default)]
struct Search {
    candidates: VecDeque<String>,
    tried: HashSet<String>,
    in_flight: Option<String>,
    rounds: u32,
}

pub struct Favicons {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// `<profile>/favicons`, one `<host>.png` per site.
    dir: Option<PathBuf>,
    by_host: RefCell<HashMap<String, SiteIcon>>,
    /// Hosts with no icon on disk (so the disk isn't asked again).
    not_on_disk: RefCell<HashSet<String>>,
    searches: HashMap<String, Search>,
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
            by_host: RefCell::default(),
            not_on_disk: RefCell::default(),
            searches: HashMap::new(),
        }
    }

    /// The icon for `page_url`'s site, if known (in memory or cached on disk).
    pub fn get(&self, page_url: &str) -> Option<SiteIcon> {
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        if let Some(icon) = self.by_host.borrow().get(&host) {
            return Some(icon.clone());
        }
        if self.not_on_disk.borrow().contains(&host) {
            return None;
        }
        let icon = self.load_from_disk(&host);
        match &icon {
            Some(icon) => {
                self.by_host.borrow_mut().insert(host, icon.clone());
            }
            None => {
                self.not_on_disk.borrow_mut().insert(host);
            }
        }
        icon
    }

    fn cache_path(&self, host: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(cache_file_name(host)))
    }

    fn load_from_disk(&self, host: &str) -> Option<SiteIcon> {
        let image = image::open(self.cache_path(host)?).ok()?.into_rgba8();
        let (w, h) = image.dimensions();
        Some(self.upload(w, h, image.as_raw()))
    }

    /// The page reported its icon links (often none): returns the next URL to fetch, if any.
    pub fn icon_links(&mut self, page_url: &str, links: &[String]) -> Option<String> {
        if self.get(page_url).is_some() {
            return None;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        let search = self.searches.entry(host).or_default();
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
        if self.get(page_url).is_some() {
            return None;
        }
        let host = display_host(page_url);
        if host.is_empty() {
            return None;
        }
        let search = self.searches.entry(host).or_default();
        if search.rounds == 0 {
            // Loaded before the page reported its icon links: the first round starts now.
            return self.icon_links(page_url, &[]);
        }
        if search.in_flight.is_some() || !search.candidates.is_empty() || search.rounds >= MAX_ROUNDS {
            return None;
        }
        search.rounds += 1;
        search.tried.clear();
        search.candidates.extend(fallback_urls(page_url));
        next(search)
    }

    /// A fetch failed: the next candidate to try, if any.
    pub fn failed(&mut self, page_url: &str) -> Option<String> {
        let search = self.searches.get_mut(&display_host(page_url))?;
        search.in_flight = None;
        next(search)
    }

    /// An icon arrived for `page_url`'s site: shown from now on, and cached on disk if `persist`.
    pub fn insert(&mut self, page_url: &str, width: u32, height: u32, rgba: &[u8], persist: bool) {
        let host = display_host(page_url);
        if host.is_empty() {
            return;
        }
        let icon = self.upload(width, height, rgba);
        self.by_host.borrow_mut().insert(host.clone(), icon);
        self.not_on_disk.borrow_mut().remove(&host);
        self.searches.remove(&host);
        if persist
            && let Some(path) = self.cache_path(&host)
            && let Some(image) = image::RgbaImage::from_raw(width, height, rgba.to_vec())
            && let Err(e) = image.save(&path)
        {
            tracing::debug!("caching the icon of {host} failed: {e}");
        }
    }

    fn upload(&self, width: u32, height: u32, rgba: &[u8]) -> SiteIcon {
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
fn cache_file_name(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    format!("{safe}.png")
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
        None => ui.add(parent, Widget::icon(Icon::Globe, side).with_paint(Paint::Secondary), style),
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
        assert_eq!(cache_file_name("de.wikipedia.org"), "de.wikipedia.org.png");
        assert_eq!(cache_file_name("localhost:8765"), "localhost_8765.png");
        assert_eq!(cache_file_name("../etc"), ".._etc.png");
    }
}
