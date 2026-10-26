//! `axion://media/`: the page of the Playlist's media view. A blank page with one `<video>`
//! that plays a captured [`MediaSource`] and nothing else (see `axion_media::media_view`).
//! The page learns its source from `/api/source?s=<token>`, which the engine serves to that
//! page only; the token was made when the view was opened.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use axion_engine::{InternalPages, InternalRequest, InternalResponse};
use axion_media::{MediaSource, StreamKind, media_view};

/// Serves the media view's files and sources (on Chromium's network thread).
#[derive(Default)]
pub struct MediaService {
    /// The sources handed to open media views, by token.
    sources: Mutex<HashMap<String, MediaSource>>,
    /// The bridge script for the page (with the browser's secret).
    bridge: RwLock<String>,
    next: Mutex<u64>,
}

impl MediaService {
    pub fn set_bridge(&self, script: String) {
        *self.bridge.write().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Makes a token for `source`; open the view at [`view_url`] with it.
    pub fn register(&self, source: &MediaSource) -> String {
        let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        *next += 1;
        // Unguessable enough for a page that only the browser can open.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let token = format!("{:x}{:x}", nanos, *next);
        self.sources.lock().unwrap_or_else(PoisonError::into_inner).insert(token.clone(), source.clone());
        token
    }

    pub fn forget(&self, token: &str) {
        self.sources.lock().unwrap_or_else(PoisonError::into_inner).remove(token);
    }
}

/// The address of a media view for a registered source.
pub fn view_url(token: &str) -> String {
    format!("axion://{}/?s={token}", media_view::HOST)
}

/// The prefix every address of a media view starts with.
pub fn view_prefix() -> String {
    format!("axion://{}/", media_view::HOST)
}

fn token_of(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| pair.strip_prefix("s="))
}

/// The JSON `/api/source` answers with.
pub fn source_json(source: &MediaSource) -> String {
    let kind = match source.kind {
        StreamKind::Hls => "hls",
        StreamKind::File | StreamKind::Dash => "file",
    };
    serde_json::json!({ "kind": kind, "url": source.url }).to_string()
}

impl InternalPages for MediaService {
    fn serve(&self, request: &InternalRequest<'_>) -> Option<InternalResponse> {
        if request.host != media_view::HOST || request.private {
            return None;
        }
        Some(match request.path {
            "/" | "/index.html" => {
                InternalResponse::ok("text/html", media_view::HTML).with_csp(media_view::CSP)
            }
            "/media.css" => InternalResponse::ok("text/css", media_view::CSS),
            "/media.js" => InternalResponse::ok("text/javascript", media_view::JS),
            "/hls.min.js" => InternalResponse::ok("text/javascript", media_view::HLS_JS),
            "/bridge.js" => {
                let script = self.bridge.read().unwrap_or_else(PoisonError::into_inner).clone();
                InternalResponse::ok("text/javascript", script)
            }
            "/api/source" => {
                let sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
                let source = sources.get(token_of(request.query)?)?;
                InternalResponse::ok("application/json", source_json(source))
            }
            _ => return None,
        })
    }
}

/// Asks several services in turn (the first that knows the host answers).
pub struct Chain(pub Vec<Arc<dyn InternalPages>>);

impl InternalPages for Chain {
    fn serve(&self, request: &InternalRequest<'_>) -> Option<InternalResponse> {
        self.0.iter().find_map(|service| service.serve(request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(path: &'a str, query: &'a str, own: bool) -> InternalRequest<'a> {
        InternalRequest { host: "media", path, query, private: false, from_own_page: own }
    }

    #[test]
    fn serves_the_page_and_the_registered_source_only() {
        let service = MediaService::default();
        service.set_bridge("/*bridge*/".into());
        let source = MediaSource::new("https://cdn.example/a.m3u8", StreamKind::Hls, "https://h.example/e");
        let token = service.register(&source);
        let page = service.serve(&request("/", "", true)).unwrap();
        assert_eq!(page.mime, "text/html");
        assert!(page.csp.as_deref().unwrap().contains("media-src http: https: blob:"));
        assert_eq!(service.serve(&request("/bridge.js", "", true)).unwrap().body, b"/*bridge*/");
        let json = service.serve(&request("/api/source", &format!("s={token}"), true)).unwrap();
        assert!(String::from_utf8(json.body).unwrap().contains(r#""kind":"hls""#));
        // Unknown tokens and forgotten sources, other hosts and private views get nothing.
        assert!(service.serve(&request("/api/source", "s=nope", true)).is_none());
        service.forget(&token);
        assert!(service.serve(&request("/api/source", &format!("s={token}"), true)).is_none());
        let other = InternalRequest { host: "home", ..request("/", "", true) };
        assert!(service.serve(&other).is_none());
        let private = InternalRequest { private: true, ..request("/", "", true) };
        assert!(service.serve(&private).is_none());
        assert_eq!(view_url("ab"), "axion://media/?s=ab");
        assert_eq!(token_of("x=1&s=ab"), Some("ab"));
    }

    #[test]
    fn tokens_differ() {
        let service = MediaService::default();
        let s = MediaSource::new("https://a/b.mp4", StreamKind::File, "");
        assert_ne!(service.register(&s), service.register(&s));
    }
}
