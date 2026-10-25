//! The hook through which the browser decides what pages may load (Shields).

/// What a request is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// The top-level page.
    Document,
    /// An iframe.
    Subdocument,
    Stylesheet,
    /// Scripts and workers.
    Script,
    Image,
    Font,
    Media,
    Object,
    Xhr,
    Ping,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct ResourceRequest<'a> {
    pub url: &'a str,
    /// The top-level page of the view that made the request.
    pub page_url: &'a str,
    pub kind: ResourceKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestVerdict {
    Allow,
    Block,
    /// Load this URL instead.
    Redirect(String),
}

/// A top-level navigation.
#[derive(Clone, Copy, Debug)]
pub struct NavigationRequest<'a> {
    /// The page being left (empty in a new view).
    pub from: &'a str,
    pub to: &'a str,
    /// Caused by a click or key press in the page.
    pub user_gesture: bool,
    /// An HTTP redirect of a navigation already under way.
    pub server_redirect: bool,
    /// Typed, bookmark, back/forward or reload, rather than started by the page.
    pub browser_initiated: bool,
}

/// Decides what pages may do. Requests are checked on Chromium's network thread, so the
/// policy must be thread-safe and quick (it runs for every request).
pub trait RequestPolicy: Send + Sync {
    fn check_request(&self, request: &ResourceRequest<'_>) -> RequestVerdict;
    /// Whether a request may send and store cookies (any thread).
    fn allow_cookies(&self, url: &str, page_url: &str) -> bool;
    /// Whether a top-level navigation may happen (main thread).
    fn allow_navigation(&self, navigation: &NavigationRequest<'_>) -> bool;
    /// Whether a frame of origin `initiator` may navigate the whole tab from `page_url` to
    /// `url` (any thread). Iframes of other sites redirecting the tab is the classic ad trick.
    fn allow_navigation_initiator(&self, page_url: &str, initiator: &str, url: &str) -> bool;
    /// JavaScript to run in each document of `url` as it starts loading (main thread).
    fn page_script(&self, url: &str) -> Option<String>;
}
