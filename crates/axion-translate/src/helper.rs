//! The engine page `axion://translate/`: a page nobody sees that runs the Bergamot engine
//! (WebAssembly) on the downloaded models. This serves it: its HTML and scripts, and under
//! `/api/` the `.wasm` and the models from the profile (the engine only serves `/api/` to the
//! page itself).
//!
//! The glue script `helper/bergamot-translator.js` is Mozilla's (MPL-2.0, unmodified; see
//! `helper/bergamot-translator.LICENSE`).

use std::path::PathBuf;

use axion_engine::{InternalPages, InternalRequest, InternalResponse};

use crate::models::ModelStore;

/// The page's host: `axion://translate/`.
pub const HOST: &str = "translate";
/// The page's address.
pub const URL: &str = "axion://translate/";

const HTML: &str = include_str!("helper/index.html");
const ENGINE_JS: &str = include_str!("helper/engine.js");
const BERGAMOT_JS: &str = include_str!("helper/bergamot-translator.js");

/// Own scripts only, plus WebAssembly; nothing from the network.
const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'";

pub struct HelperService {
    store: ModelStore,
}

impl HelperService {
    pub fn new(store: ModelStore) -> Self {
        Self { store }
    }
}

/// `de-en` pairs and file names only: no path separators, nothing that climbs out.
fn safe(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 40
        && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !part.starts_with('.')
}

impl InternalPages for HelperService {
    fn serve(&self, request: &InternalRequest<'_>) -> Option<InternalResponse> {
        if request.host != HOST {
            return None;
        }
        if request.private {
            return None;
        }
        match request.path {
            "/" | "/index.html" => Some(InternalResponse::ok("text/html", HTML).with_csp(CSP)),
            "/engine.js" => Some(InternalResponse::ok("text/javascript", ENGINE_JS)),
            "/bergamot.js" => Some(InternalResponse::ok("text/javascript", BERGAMOT_JS)),
            "/api/wasm" => read(self.store.wasm_path(), "application/wasm"),
            path => {
                let rest = path.strip_prefix("/api/model/")?;
                let (pair, file) = rest.split_once('/')?;
                if !safe(pair) || !safe(file) {
                    return None;
                }
                let (from, to) = pair.split_once('-')?;
                if !safe(from) || !safe(to) {
                    return None;
                }
                let mime =
                    if file.ends_with(".json") { "application/json" } else { "application/octet-stream" };
                read(self.store.pair_dir(from, to).join(file), mime)
            }
        }
    }
}

fn read(path: PathBuf, mime: &str) -> Option<InternalResponse> {
    match std::fs::read(&path) {
        Ok(bytes) => Some(InternalResponse::ok(mime, bytes)),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(path: &'a str) -> InternalRequest<'a> {
        InternalRequest { host: HOST, path, query: "", private: false, from_own_page: true }
    }

    #[test]
    fn serves_files_of_the_store_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        std::fs::create_dir_all(store.pair_dir("de", "en")).unwrap();
        std::fs::write(store.pair_dir("de", "en").join("model.bin"), b"MODEL").unwrap();
        std::fs::write(store.wasm_path(), b"\0asm").unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"nope").unwrap();
        let service = HelperService::new(store);
        assert!(service.serve(&request("/")).unwrap().csp.unwrap().contains("wasm-unsafe-eval"));
        assert_eq!(service.serve(&request("/api/model/de-en/model.bin")).unwrap().body, b"MODEL");
        assert_eq!(service.serve(&request("/api/wasm")).unwrap().mime, "application/wasm");
        assert!(service.serve(&request("/api/model/de-en/..%2Fsecret.txt")).is_none());
        assert!(service.serve(&request("/api/model/../secret.txt")).is_none());
        assert!(service.serve(&request("/api/model/de-en/../../secret.txt")).is_none());
        assert!(service.serve(&request("/api/model/de-en/missing.bin")).is_none());
        let other = InternalRequest { host: "home", ..request("/") };
        assert!(service.serve(&other).is_none());
        let private = InternalRequest { private: true, ..request("/") };
        assert!(service.serve(&private).is_none());
    }
}
