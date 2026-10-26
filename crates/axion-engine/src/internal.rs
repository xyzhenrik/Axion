//! Axion's own pages (`axion://<host>/…`, e.g. the start page), served by the browser itself
//! without any server, and the privileged channel between them and the browser.
//!
//! The scheme is registered with the engine as *standard*, *secure*, *display-isolated* and
//! *fetch-enabled*, but not CORS-enabled: web pages can't link to, frame, navigate to or fetch
//! `axion://` URLs; only the browser opens them (typed, homepage, new tab).
//!
//! The channel has two directions:
//! - **Page → browser**: `fetch("/api/message", {method: "POST", body})` from the page's own
//!   top-level document. The engine checks that the request comes from a view's *main frame*
//!   whose current document has the origin `axion://<host>` and turns the body into an
//!   [`crate::EngineEvent::InternalMessage`]; requests from any other frame are refused (403).
//! - **Browser → page**: data is pulled by the page from `/api/…` paths, which the engine only
//!   serves under the same check, so nothing the browser hands out reaches another origin.
//!   To ask an open page to pull again the browser runs a script that carries no data.

/// The scheme of Axion's own pages.
pub const INTERNAL_SCHEME: &str = "axion";

/// Path prefix of the privileged endpoints (only served to the host's own page).
pub const API_PREFIX: &str = "/api/";

/// The endpoint the page posts its messages to.
pub const MESSAGE_PATH: &str = "/api/message";

/// Largest message body accepted from a page.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// One request for an `axion://` resource.
#[derive(Clone, Copy, Debug)]
pub struct InternalRequest<'a> {
    /// `home` for `axion://home/…`.
    pub host: &'a str,
    /// Starts with `/`; without query or fragment.
    pub path: &'a str,
    /// After the `?` (empty without one), still URL-encoded.
    pub query: &'a str,
    /// The request comes from a private view (separate in-memory profile).
    pub private: bool,
    /// The request comes from the host's own top-level page. Always true for [`API_PREFIX`]
    /// paths (the engine refuses those otherwise).
    pub from_own_page: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalResponse {
    pub status: u16,
    pub mime: String,
    pub body: Vec<u8>,
    /// A `Content-Security-Policy` for an HTML page instead of the strict default (own files
    /// only). For pages that must load media or streams from the network.
    pub csp: Option<String>,
}

impl InternalResponse {
    pub fn ok(mime: &str, body: impl Into<Vec<u8>>) -> Self {
        Self { status: 200, mime: mime.to_owned(), body: body.into(), csp: None }
    }

    pub fn not_found() -> Self {
        Self { status: 404, mime: "text/plain".into(), body: b"Not found".to_vec(), csp: None }
    }

    pub fn forbidden() -> Self {
        Self { status: 403, mime: "text/plain".into(), body: b"Forbidden".to_vec(), csp: None }
    }

    /// The page under its own policy.
    pub fn with_csp(mut self, csp: &str) -> Self {
        self.csp = Some(csp.to_owned());
        self
    }
}

/// What serves Axion's own pages. Called on the engine's network thread, so it must be
/// thread-safe and quick (keep data ready, don't block on the browser's main thread).
pub trait InternalPages: Send + Sync {
    /// The resource, or `None` for "not found".
    fn serve(&self, request: &InternalRequest<'_>) -> Option<InternalResponse>;
}

/// Splits `axion://host/path?query#fragment` into host, path and query.
pub fn split_internal_url(url: &str) -> Option<(&str, &str, &str)> {
    let rest = url.strip_prefix(INTERNAL_SCHEME)?.strip_prefix("://")?;
    let rest = rest.split('#').next().unwrap_or_default();
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    (!host.is_empty()).then_some((host, path, query))
}

/// The origin (`axion://host`) of an internal URL, lowercased host.
pub fn internal_origin(url: &str) -> Option<String> {
    let (host, _, _) = split_internal_url(url)?;
    Some(format!("{INTERNAL_SCHEME}://{}", host.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        assert_eq!(split_internal_url("axion://home/"), Some(("home", "/", "")));
        assert_eq!(split_internal_url("axion://home"), Some(("home", "/", "")));
        assert_eq!(
            split_internal_url("axion://home/api/favicon?host=a.b#x"),
            Some(("home", "/api/favicon", "host=a.b"))
        );
        assert_eq!(split_internal_url("https://home/"), None);
        assert_eq!(split_internal_url("axion:///x"), None);
        assert_eq!(internal_origin("axion://Home/x").as_deref(), Some("axion://home"));
    }
}
