//! Site icons, decoded by Chromium and kept as GPU textures per host.

use std::collections::{HashMap, HashSet};

use axion_ui::taffy::prelude::*;
use axion_ui::{Background, Paint, Ui, Widget};

use crate::url::display_host;

/// Requested icon size in pixels (shown at 16–18 px logical, crisp up to 3× scale).
pub const FETCH_SIZE: u32 = 64;

pub struct Favicons {
    device: wgpu::Device,
    queue: wgpu::Queue,
    by_host: HashMap<String, (wgpu::TextureView, (u32, u32))>,
    requested: HashSet<String>,
}

impl Favicons {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue, by_host: HashMap::new(), requested: HashSet::new() }
    }

    /// The icon for `page_url`'s site, if loaded.
    pub fn get(&self, page_url: &str) -> Option<(wgpu::TextureView, (u32, u32))> {
        self.by_host.get(&display_host(page_url)).cloned()
    }

    /// Whether to fetch an icon for `page_url` (once per site and session).
    pub fn should_fetch(&mut self, page_url: &str) -> bool {
        let host = display_host(page_url);
        !host.is_empty() && !self.by_host.contains_key(&host) && self.requested.insert(host)
    }

    pub fn insert(&mut self, page_url: &str, width: u32, height: u32, rgba: &[u8]) {
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
        let view = texture.create_view(&Default::default());
        self.by_host.insert(display_host(page_url), (view, (width, height)));
    }
}

/// `scheme://host/favicon.ico` for a page, the conventional icon location.
pub fn fallback_url(page_url: &str) -> Option<String> {
    let (scheme, rest) = page_url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}/favicon.ico"))
}

/// A favicon of `side` logical pixels: the site's icon, or a coloured square until it loads.
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
        None => {
            let color = Paint::Fixed(crate::chrome::favicon_color(url));
            ui.add(
                parent,
                Widget::container().with_background(Background::Fill(color, 1.0)).with_radius(5.0),
                style,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_icon_location() {
        assert_eq!(
            fallback_url("https://de.wikipedia.org/wiki/X?y=1").as_deref(),
            Some("https://de.wikipedia.org/favicon.ico")
        );
        assert_eq!(fallback_url("about:blank"), None);
        assert_eq!(fallback_url("file:///tmp/a.html"), None);
    }
}
