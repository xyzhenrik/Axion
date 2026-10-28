//! The media view's way to its stream. The view lives on `axion://media`, an origin no hoster
//! or CDN lists in its CORS headers, so hls.js could never read a manifest or a segment
//! itself. Its requests for them are answered here instead: the browser fetches the very
//! address the view asked for (with the Referer the source needs; Shields judged the request
//! before it got here) and hands the response to the view, marked readable. Nothing is
//! stored: the response goes to the player and is dropped.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cef::*;

/// The most one response may hold (a segment is a few MB; this only stops runaway bodies).
const MAX_BODY: u64 = 96 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36";

#[derive(Default)]
struct Fetched {
    status: i32,
    mime: String,
    content_range: String,
    body: Vec<u8>,
    offset: usize,
}

type Shared = Arc<Mutex<Fetched>>;

fn fetch(url: &str, referer: &str, range: Option<&str>) -> Fetched {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into();
    let mut request = agent.get(url);
    if referer.starts_with("http") {
        request = request.header("Referer", referer);
    }
    if let Some(range) = range {
        request = request.header("Range", range);
    }
    let mut response = match request.call() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%url, "media view fetch failed: {e}");
            return Fetched { status: 502, ..Default::default() };
        }
    };
    let header = |name: &str| {
        response.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
    };
    let status = i32::from(response.status().as_u16());
    let (mime, content_range) = (header("content-type"), header("content-range"));
    let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec();
    match body {
        Ok(body) => {
            tracing::debug!(%url, status, bytes = body.len(), "media view fetch");
            Fetched { status, mime, content_range, body, offset: 0 }
        }
        Err(e) => {
            tracing::warn!(%url, "reading a media view response failed: {e}");
            Fetched { status: 502, ..Default::default() }
        }
    }
}

/// A resource handler answering `url` through [`fetch`].
pub(super) fn handler(url: String, referer: String, range: Option<String>) -> ResourceHandler {
    ProxyHandlerBuilder::new(url, referer, range, Arc::new(Mutex::new(Fetched::default())))
}

wrap_resource_handler! {
    pub(super) struct ProxyHandlerBuilder {
        url: String,
        referer: String,
        range: Option<String>,
        state: Shared,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            let (Some(handle_request), Some(callback)) = (handle_request, callback) else { return 0 };
            // The download happens off Chromium's IO thread; `cont` tells it when it is there.
            *handle_request = 0;
            let (url, referer, range) = (self.url.clone(), self.referer.clone(), self.range.clone());
            let (state, callback) = (self.state.clone(), callback.clone());
            let spawned = std::thread::Builder::new().name("media-view-fetch".into()).spawn(move || {
                let fetched = fetch(&url, &referer, range.as_deref());
                *state.lock().unwrap_or_else(PoisonError::into_inner) = fetched;
                callback.cont();
            });
            match spawned {
                Ok(_) => 1,
                Err(e) => {
                    tracing::warn!("media view fetch thread failed: {e}");
                    0
                }
            }
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(response) = response else { return };
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            response.set_status(state.status);
            let mime = if state.mime.is_empty() { "application/octet-stream" } else { state.mime.as_str() };
            response.set_mime_type(Some(&mime.into()));
            let set = |name: &str, value: &str| {
                response.set_header_by_name(Some(&name.into()), Some(&value.into()), 1);
            };
            // The point of the proxy: the view may read this response.
            set("Access-Control-Allow-Origin", "*");
            set("Access-Control-Expose-Headers", "*");
            set("Cache-Control", "no-store");
            if !state.content_range.is_empty() {
                set("Content-Range", &state.content_range);
            }
            if let Some(length) = response_length {
                *length = state.body.len() as i64;
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(bytes_read) = bytes_read else { return 0 };
            *bytes_read = 0;
            let Ok(capacity) = usize::try_from(bytes_to_read) else { return 0 };
            if data_out.is_null() || capacity == 0 {
                return 0;
            }
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let n = capacity.min(state.body.len().saturating_sub(state.offset));
            if n == 0 {
                return 0;
            }
            // SAFETY: CEF passes a writable buffer of `bytes_to_read` bytes that lives for this
            // call; `n` is at most that, and the source range is inside `body`.
            unsafe { std::ptr::copy_nonoverlapping(state.body.as_ptr().add(state.offset), data_out, n) };
            state.offset += n;
            *bytes_read = n as ::std::os::raw::c_int;
            1
        }
    }
}
