//! Filter lists: downloaded in the background, cached in the profile, and compiled into one
//! adblock engine that is itself cached (deserializing it is much faster than parsing).
//!
//! Layout of the cache directory:
//! - `lists/<hash>.txt` — the raw text of each list, kept so a failed download can fall back
//! - `engine.dat` — the serialized engine
//! - `popups.dat` — the serialized popup engine (see [`popup_rules`])
//! - `engine.stamp` — `<unix time of the update>\n<hash of the list URLs>`

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adblock::lists::{FilterSet, ParseOptions};

/// Lists bigger than this are refused (EasyList is ~1.5 MB).
const MAX_LIST_BYTES: u64 = 32 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error("no filter list could be loaded")]
    NothingLoaded,
    #[error("writing the filter cache failed: {0}")]
    Write(#[from] std::io::Error),
}

pub(crate) struct ListCache {
    dir: PathBuf,
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

fn lists_hash(lists: &[String]) -> u64 {
    fnv(lists.iter().flat_map(|l| l.bytes().chain([0])))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Writes through a temp file so a crash never leaves half a file behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

impl ListCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn list_path(&self, url: &str) -> PathBuf {
        self.dir.join("lists").join(format!("{:016x}.txt", fnv(url.bytes())))
    }

    fn stamp(&self) -> Option<(u64, u64)> {
        let text = std::fs::read_to_string(self.dir.join("engine.stamp")).ok()?;
        let mut lines = text.lines();
        Some((lines.next()?.parse().ok()?, lines.next()?.parse().ok()?))
    }

    /// The cached engines, if they were built from exactly these lists.
    pub fn load(&self, lists: &[String]) -> Option<Engines> {
        let (_, hash) = self.stamp()?;
        if hash != lists_hash(lists) {
            return None;
        }
        let load = |name: &str| {
            let bytes = std::fs::read(self.dir.join(name)).ok()?;
            let mut engine = adblock::Engine::default();
            match engine.deserialize(&bytes) {
                Ok(()) => Some(engine),
                Err(e) => {
                    tracing::info!("the cached filter engine {name} is unusable ({e:?}); rebuilding");
                    None
                }
            }
        };
        Some(Engines { network: load("engine.dat")?, popups: load("popups.dat")? })
    }

    /// Whether these lists should be downloaded again.
    pub fn is_stale(&self, lists: &[String], max_age: Duration) -> bool {
        match self.stamp() {
            Some((at, hash)) => hash != lists_hash(lists) || now().saturating_sub(at) > max_age.as_secs(),
            None => true,
        }
    }

    /// Downloads every list (falling back to the cached copy of any that fails), compiles
    /// them and caches the result.
    pub fn update(&self, lists: &[String]) -> Result<Engines, ListError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .user_agent(concat!("Axion/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        std::fs::create_dir_all(self.dir.join("lists"))?;
        let mut set = FilterSet::new(false);
        let mut popups = FilterSet::new(false);
        let mut loaded = 0;
        for url in lists {
            let path = self.list_path(url);
            let text = match download(&agent, url) {
                Ok(text) => {
                    write_atomic(&path, text.as_bytes())?;
                    Some(text)
                }
                Err(e) => {
                    tracing::warn!(%url, "filter list download failed: {e}");
                    std::fs::read_to_string(&path).ok()
                }
            };
            if let Some(text) = text {
                popups.add_filter_list(popup_rules(&text), ParseOptions::default());
                set.add_filter_list(text, ParseOptions::default());
                loaded += 1;
            }
        }
        if loaded == 0 {
            return Err(ListError::NothingLoaded);
        }
        let engines = Engines {
            network: adblock::Engine::new_with_filter_set(set),
            popups: adblock::Engine::new_with_filter_set(popups),
        };
        write_atomic(&self.dir.join("engine.dat"), &engines.network.serialize())?;
        write_atomic(&self.dir.join("popups.dat"), &engines.popups.serialize())?;
        write_atomic(
            &self.dir.join("engine.stamp"),
            format!("{}\n{}\n", now(), lists_hash(lists)).as_bytes(),
        )?;
        tracing::info!(lists = loaded, "filter lists updated");
        Ok(engines)
    }
}

/// The compiled lists.
pub(crate) struct Engines {
    pub network: adblock::Engine,
    /// The lists' `$popup` rules, as `$document` rules: adblock-rust has no popup type, and
    /// the same ad sites are where unwanted redirects go.
    pub popups: adblock::Engine,
}

impl Engines {
    pub fn empty() -> Self {
        Self { network: adblock::Engine::default(), popups: adblock::Engine::default() }
    }

    pub fn from_rules(rules: &str) -> Self {
        Self {
            network: adblock::Engine::new_with_list_text(rules),
            popups: adblock::Engine::new_with_list_text(popup_rules(rules)),
        }
    }
}

/// The `$popup` network rules of a list, rewritten to `$document`.
pub(crate) fn popup_rules(list: &str) -> String {
    let mut out = String::new();
    for line in list.lines() {
        let line = line.trim();
        if line.starts_with('!') || line.starts_with('[') || line.contains('#') {
            continue;
        }
        let Some((pattern, options)) = line.rsplit_once('$') else { continue };
        if !options.split(',').any(|o| o == "popup") {
            continue;
        }
        let options: Vec<&str> =
            options.split(',').map(|o| if o == "popup" { "document" } else { o }).collect();
        out.push_str(pattern);
        out.push('$');
        out.push_str(&options.join(","));
        out.push('\n');
    }
    out
}

fn download(agent: &ureq::Agent, url: &str) -> Result<String, ureq::Error> {
    agent.get(url).call()?.body_mut().with_config().limit(MAX_LIST_BYTES).read_to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable() {
        // The cache layout depends on these never changing.
        assert_eq!(fnv(*b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(lists_hash(&["ab".into()]), lists_hash(&["a".into(), "b".into()]));
    }

    #[test]
    fn extracts_popup_rules() {
        let list = "! comment\n||ads.com^\n||pop.biz^$popup,third-party\n@@||ok.biz^$popup\nsite##.x\n";
        assert_eq!(popup_rules(list), "||pop.biz^$document,third-party\n@@||ok.biz^$document\n");
    }
}
