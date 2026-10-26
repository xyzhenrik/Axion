//! SVG site icons that change with the colour scheme (`@media (prefers-color-scheme: dark)`
//! inside the file): Chromium would rasterise them for the light scheme whatever the chrome
//! looks like. The SVG's text is fetched on a thread here, its media queries are decided for the
//! chrome (`favicon_contrast::svg_for_scheme`), and the rewritten copy goes to Chromium as a
//! `data:` URL to rasterise. SVGs without such rules are fetched by Chromium as they are.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use axion_engine::ViewId;

use crate::favicon_contrast::{svg_data_url, svg_for_scheme};

const TIMEOUT: Duration = Duration::from_secs(8);
/// Larger files aren't icons.
const MAX_BYTES: u64 = 256 * 1024;

/// An SVG icon ready for Chromium.
pub struct Prepared {
    pub view: ViewId,
    /// The icon's address.
    pub url: String,
    /// What to have Chromium download: a rewritten `data:` URL, or `url` itself.
    pub fetch: String,
}

pub struct SvgIcons {
    tx: Sender<Prepared>,
    rx: Receiver<Prepared>,
}

impl SvgIcons {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx }
    }

    /// Prepares `url` for `view` on chrome of the given tone (`dark`).
    pub fn request(&self, view: ViewId, url: &str, dark: bool) {
        let tx = self.tx.clone();
        let task = {
            let url = url.to_owned();
            move || {
                let fetch = rewritten(&url, dark).unwrap_or_else(|| url.clone());
                // The receiver only goes away when the browser quits.
                tx.send(Prepared { view, url, fetch }).ok();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("svg-icon".into()).spawn(task) {
            tracing::warn!("preparing the SVG icon {url} failed: {e}");
        }
    }

    /// Icons that are ready.
    pub fn poll(&self) -> Vec<Prepared> {
        self.rx.try_iter().collect()
    }
}

/// The data URL of `url`'s SVG for the scheme, if the file has scheme-dependent rules.
fn rewritten(url: &str, dark: bool) -> Option<String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("Axion/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut response = agent.get(url).call().ok()?;
    let bytes = response.body_mut().with_config().limit(MAX_BYTES).read_to_vec().ok()?;
    let text = String::from_utf8(bytes).ok()?;
    svg_for_scheme(&text, dark).map(|svg| svg_data_url(&svg))
}
