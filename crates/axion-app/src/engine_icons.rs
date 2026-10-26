//! The search engines' icons for the start page's engine menu, fetched in the background the
//! first time the start page shows: the menu should have them before the user ever visited
//! each engine. (Chromium won't load them for the start page itself: its own scheme.)

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::favicons::FETCH_SIZE;

const TIMEOUT: Duration = Duration::from_secs(10);
/// Where engines keep an icon, tried in order.
const PATHS: [&str; 2] = ["/favicon.ico", "/apple-touch-icon.png"];

/// A fetched icon: the engine's site (`https://host/`), the icon's URL, its size and pixels.
pub struct EngineIcon {
    pub site: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Fetches the icons of `hosts` on a thread; they arrive on the returned channel.
pub fn fetch(hosts: Vec<String>) -> Receiver<EngineIcon> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("engine-icons".into()).spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("Axion/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        for host in hosts {
            let load = |url: String| {
                let bytes = agent.get(&url).call().ok()?.body_mut().read_to_vec().ok()?;
                let image = image::load_from_memory(&bytes).ok()?;
                Some((url, image.thumbnail(FETCH_SIZE, FETCH_SIZE).to_rgba8()))
            };
            // The usual places first, then what the engine's page links to.
            let icon = PATHS.iter().find_map(|path| load(format!("https://{host}{path}"))).or_else(|| {
                let page =
                    agent.get(&format!("https://{host}/")).call().ok()?.body_mut().read_to_string().ok()?;
                icon_links(&page, &host).into_iter().find_map(load)
            });
            match icon {
                Some((url, image)) => {
                    let (width, height) = image.dimensions();
                    let icon = EngineIcon {
                        site: format!("https://{host}/"),
                        url,
                        width,
                        height,
                        rgba: image.into_raw(),
                    };
                    if tx.send(icon).is_err() {
                        return;
                    }
                }
                None => tracing::debug!("no icon for {host}"),
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("fetching search engine icons failed: {e}");
    }
    rx
}

/// The bitmap icons a page's `<link rel="…icon…">` tags point to (absolute URLs; SVG left out).
fn icon_links(html: &str, host: &str) -> Vec<String> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
        Some(tag[at..].split('"').next()?.to_owned())
    };
    html.split("<link")
        .skip(1)
        .filter_map(|rest| rest.split('>').next())
        .filter(|tag| attr(tag, "rel").is_some_and(|rel| rel.contains("icon")))
        .filter_map(|tag| attr(tag, "href"))
        .filter(|href| !href.ends_with(".svg") && !href.contains("svg+xml"))
        .map(|href| match href {
            h if h.starts_with("https://") => h,
            h if h.starts_with("//") => format!("https:{h}"),
            h if h.starts_with('/') => format!("https://{host}{h}"),
            h => format!("https://{host}/{h}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bitmap_icon_links() {
        let html = r#"<link rel="icon" type="image/png" href="https://cdn.example/a.png" sizes="96x96"/>
            <link rel="icon" type="image/svg+xml" href="/b.svg"/><link rel="stylesheet" href="/c.css">
            <link rel="apple-touch-icon" href="/d.png">"#;
        assert_eq!(
            icon_links(html, "example.com"),
            ["https://cdn.example/a.png", "https://example.com/d.png"]
        );
    }
}
