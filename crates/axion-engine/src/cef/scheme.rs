//! The `axion://` scheme: registration in every Chromium process, and the handler that serves
//! Axion's own pages from the browser's [`InternalPages`] (see [`crate::internal`] for the
//! security model). Handlers run on Chromium's IO thread, so they only hold `Send + Sync`
//! state (`Arc<Network>`).

use std::sync::{Arc, Mutex, PoisonError};

use cef::sys::cef_scheme_options_t;
use cef::*;

use super::network::Network;
use crate::EngineEvent;
use crate::internal::{
    API_PREFIX, INTERNAL_SCHEME, InternalRequest, InternalResponse, MAX_MESSAGE_BYTES, MESSAGE_PATH,
    internal_origin, split_internal_url,
};

/// Registers `axion` as a standard, secure, display-isolated scheme with Fetch support (but no
/// CORS: other origins can't request it). Must run in every process (`OnRegisterCustomSchemes`).
pub(super) fn register_scheme(registrar: &mut SchemeRegistrar) {
    use cef_scheme_options_t::*;
    let options = CEF_SCHEME_OPTION_STANDARD as i32
        | CEF_SCHEME_OPTION_SECURE as i32
        | CEF_SCHEME_OPTION_DISPLAY_ISOLATED as i32
        | CEF_SCHEME_OPTION_FETCH_ENABLED as i32;
    registrar.add_custom_scheme(Some(&INTERNAL_SCHEME.into()), options);
}

/// Installs the handler for every `axion://` host in a profile (`None`: the normal one).
pub(super) fn install(net: &Arc<Network>, context: Option<&RequestContext>, private: bool) {
    let mut factory = FactoryBuilder::new(Arc::clone(net), private);
    let ok = match context {
        Some(context) => {
            context.register_scheme_handler_factory(Some(&INTERNAL_SCHEME.into()), None, Some(&mut factory))
        }
        None => register_scheme_handler_factory(Some(&INTERNAL_SCHEME.into()), None, Some(&mut factory)),
    };
    if ok != 1 {
        tracing::error!(private, "registering the axion:// handler failed");
    }
}

/// Whether `frame` is the top-level document of a page with `url`'s origin.
fn from_own_page(frame: Option<&Frame>, url: &str) -> bool {
    let Some(frame) = frame.filter(|f| f.is_main() == 1) else { return false };
    let page = CefString::from(&frame.url()).to_string();
    let (Some(page), Some(target)) = (internal_origin(&page), internal_origin(url)) else { return false };
    page == target
}

/// The body of a POST request (bytes elements only).
fn post_body(request: &Request) -> Option<Vec<u8>> {
    let data = request.post_data()?;
    // The binding reads as many elements as the vector is long.
    let mut elements = vec![None; data.element_count()];
    data.elements(Some(&mut elements));
    let mut body = Vec::new();
    for element in elements.into_iter().flatten() {
        let size = element.bytes_count();
        if body.len() + size > MAX_MESSAGE_BYTES {
            return None;
        }
        let start = body.len();
        body.resize(start + size, 0);
        // `bytes` copies at most `size` bytes into the slice we just made room for.
        let copied = element.bytes(size, body[start..].as_mut_ptr());
        body.truncate(start + copied);
    }
    Some(body)
}

wrap_scheme_handler_factory! {
    pub(super) struct FactoryBuilder {
        net: Arc<Network>,
        private: bool,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let request = request?;
            let url = CefString::from(&request.url()).to_string();
            let Some((host, path, query)) = split_internal_url(&url) else {
                return Some(handler(InternalResponse::not_found()));
            };
            let host = host.to_ascii_lowercase();
            let frame = frame.map(|f| &*f);
            let own = from_own_page(frame, &url);
            if path.starts_with(API_PREFIX) && !own {
                let from = frame.map(|f| (f.is_main() == 1, CefString::from(&f.url()).to_string()));
                tracing::warn!(%url, ?from, "refused an axion:// API request from another page");
                return Some(handler(InternalResponse::forbidden()));
            }
            if path == MESSAGE_PATH {
                let method = CefString::from(&request.method()).to_string();
                let browser_id = browser.map(|b| b.identifier());
                let view = browser_id.and_then(|id| self.net.view_of(id));
                let (Some(view), Some(browser_id), true) = (view, browser_id, method == "POST") else {
                    return Some(handler(InternalResponse::forbidden()));
                };
                let Some(body) = post_body(request) else {
                    return Some(handler(InternalResponse::forbidden()));
                };
                let message = String::from_utf8_lossy(&body).into_owned();
                self.net.emit_for(browser_id, EngineEvent::InternalMessage { view, host, message });
                return Some(handler(InternalResponse { status: 204, mime: "text/plain".into(), body: Vec::new() }));
            }
            let request = InternalRequest { host: &host, path, query, private: self.private, from_own_page: own };
            let response = self.net.internal_pages().and_then(|pages| pages.serve(&request));
            Some(handler(response.unwrap_or_else(InternalResponse::not_found)))
        }
    }
}

fn handler(response: InternalResponse) -> ResourceHandler {
    BytesHandlerBuilder::new(Arc::new(response), Arc::new(Mutex::new(0)))
}

wrap_resource_handler! {
    pub(super) struct BytesHandlerBuilder {
        response: Arc<InternalResponse>,
        // Bytes of the body already read.
        offset: Arc<Mutex<usize>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            // Everything is ready: continue at once.
            if let Some(handle_request) = handle_request {
                *handle_request = 1;
            }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(response) = response else { return };
            let r = &self.response;
            response.set_status(i32::from(r.status));
            response.set_mime_type(Some(&r.mime.as_str().into()));
            if r.mime.starts_with("text/") || r.mime == "application/json" || r.mime.ends_with("javascript") {
                response.set_charset(Some(&"utf-8".into()));
            }
            response.set_header_by_name(Some(&"Cache-Control".into()), Some(&"no-store".into()), 1);
            response.set_header_by_name(Some(&"X-Content-Type-Options".into()), Some(&"nosniff".into()), 1);
            if r.mime == "text/html" {
                // Only the page's own files; images also from data: URLs.
                response.set_header_by_name(
                    Some(&"Content-Security-Policy".into()),
                    Some(&"default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'".into()),
                    1,
                );
            }
            if let Some(length) = response_length {
                *length = r.body.len() as i64;
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
            let mut offset = self.offset.lock().unwrap_or_else(PoisonError::into_inner);
            let body = &self.response.body;
            let n = capacity.min(body.len().saturating_sub(*offset));
            if n == 0 {
                // Done.
                return 0;
            }
            // SAFETY: CEF passes a writable buffer of `bytes_to_read` bytes that lives for this
            // call; `n` is at most that, and the source range is inside `body`.
            unsafe { std::ptr::copy_nonoverlapping(body.as_ptr().add(*offset), data_out, n) };
            *offset += n;
            *bytes_read = n as ::std::os::raw::c_int;
            1
        }
    }
}
