//! The request hooks behind [`RequestPolicy`]. Unlike the other handlers these run on
//! Chromium's IO thread (except `on_before_browse`), so they only hold thread-safe state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use cef::sys::{cef_resource_type_t, cef_return_value_t, cef_transition_type_t};
use cef::*;

use crate::{
    EngineEvent, INTERNAL_SCHEME, InternalPages, NavigationRequest, RequestPolicy, RequestVerdict,
    ResourceKind, ResourceRequest, ViewId,
};

/// State shared by every view's network hooks.
#[derive(Default)]
pub(super) struct Network {
    pub policy: RwLock<Option<Arc<dyn RequestPolicy>>>,
    /// Events raised off the main thread, tagged with the CEF browser like [`super::Shared`]'s.
    pub events: Mutex<Vec<(i32, EngineEvent)>>,
    /// What serves the `axion://` pages.
    pub internal: RwLock<Option<Arc<dyn InternalPages>>>,
    /// The view of each live CEF browser (by its identifier).
    pub browsers: Mutex<HashMap<i32, ViewId>>,
}

impl Network {
    pub fn policy(&self) -> Option<Arc<dyn RequestPolicy>> {
        self.policy.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn internal_pages(&self) -> Option<Arc<dyn InternalPages>> {
        self.internal.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn view_of(&self, browser_id: i32) -> Option<ViewId> {
        self.browsers.lock().unwrap_or_else(PoisonError::into_inner).get(&browser_id).copied()
    }

    pub fn set_browser(&self, browser_id: i32, view: Option<ViewId>) {
        let mut browsers = self.browsers.lock().unwrap_or_else(PoisonError::into_inner);
        match view {
            Some(view) => browsers.insert(browser_id, view),
            None => browsers.remove(&browser_id),
        };
    }

    fn emit(&self, browser: Option<&Browser>, event: EngineEvent) {
        self.emit_for(browser.map_or(-1, |b| b.identifier()), event);
    }

    pub fn emit_for(&self, browser_id: i32, event: EngineEvent) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).push((browser_id, event));
    }

    pub fn take_events(&self) -> Vec<(i32, EngineEvent)> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn resource_kind(kind: ResourceType) -> ResourceKind {
    use cef_resource_type_t::*;
    match *kind.as_ref() {
        RT_MAIN_FRAME | RT_NAVIGATION_PRELOAD_MAIN_FRAME => ResourceKind::Document,
        RT_SUB_FRAME | RT_NAVIGATION_PRELOAD_SUB_FRAME => ResourceKind::Subdocument,
        RT_STYLESHEET => ResourceKind::Stylesheet,
        RT_SCRIPT | RT_WORKER | RT_SHARED_WORKER | RT_SERVICE_WORKER => ResourceKind::Script,
        RT_IMAGE | RT_FAVICON => ResourceKind::Image,
        RT_FONT_RESOURCE => ResourceKind::Font,
        RT_MEDIA => ResourceKind::Media,
        RT_OBJECT | RT_PLUGIN_RESOURCE => ResourceKind::Object,
        RT_XHR => ResourceKind::Xhr,
        RT_PING | RT_CSP_REPORT => ResourceKind::Ping,
        _ => ResourceKind::Other,
    }
}

fn main_frame_url(browser: Option<&Browser>) -> String {
    browser.and_then(|b| b.main_frame()).map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default()
}

// Refuses every cookie of the request it is attached to.
wrap_cookie_access_filter! {
    pub(super) struct DenyCookiesBuilder {}

    impl CookieAccessFilter {
        fn can_send_cookie(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _cookie: Option<&Cookie>,
        ) -> ::std::os::raw::c_int {
            0
        }

        fn can_save_cookie(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _response: Option<&mut Response>,
            _cookie: Option<&Cookie>,
        ) -> ::std::os::raw::c_int {
            0
        }
    }
}

wrap_resource_request_handler! {
    pub(super) struct ResourceRequestHandlerBuilder {
        net: Arc<Network>,
        view: ViewId,
        deny_cookies: CookieAccessFilter,
    }

    impl ResourceRequestHandler {
        fn cookie_access_filter(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
        ) -> Option<CookieAccessFilter> {
            let (policy, request) = (self.net.policy()?, request?);
            let url = CefString::from(&request.url()).to_string();
            let page_url = match resource_kind(request.resource_type()) {
                ResourceKind::Document => url.clone(),
                _ => main_frame_url(browser.map(|b| &*b)),
            };
            (!policy.allow_cookies(&url, &page_url)).then(|| self.deny_cookies.clone())
        }

        fn on_before_resource_load(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let proceed = cef_return_value_t::RV_CONTINUE.into();
            let (Some(policy), Some(request)) = (self.net.policy(), request) else { return proceed };
            let browser = browser.map(|b| &*b);
            let url = CefString::from(&request.url()).to_string();
            let kind = resource_kind(request.resource_type());
            let page_url = if kind == ResourceKind::Document { url.clone() } else { main_frame_url(browser) };
            match policy.check_request(&ResourceRequest { url: &url, page_url: &page_url, kind }) {
                RequestVerdict::Allow => proceed,
                RequestVerdict::Redirect(to) => {
                    request.set_url(Some(&to.as_str().into()));
                    proceed
                }
                RequestVerdict::Block => {
                    self.net.emit(browser, EngineEvent::RequestBlocked { view: self.view, url });
                    cef_return_value_t::RV_CANCEL.into()
                }
            }
        }
    }
}

// Cancels the request it is attached to (a blocked top-level navigation).
wrap_resource_request_handler! {
    pub(super) struct CancelBuilder {}

    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            cef_return_value_t::RV_CANCEL.into()
        }
    }
}

wrap_request_handler! {
    pub(super) struct RequestHandlerBuilder {
        net: Arc<Network>,
        view: ViewId,
        resources: ResourceRequestHandler,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            user_gesture: ::std::os::raw::c_int,
            is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let (Some(frame), Some(request)) = (frame, request) else { return 0 };
            if refuse_internal_navigation(frame, request) {
                return 1;
            }
            let Some(policy) = self.net.policy() else { return 0 };
            if frame.is_main() == 0 {
                return 0;
            }
            use cef_transition_type_t::*;
            // The transition is a source value plus qualifier bits, so compare raw values.
            let transition = *request.transition_type().as_ref() as u32;
            let source = transition & TT_SOURCE_MASK as u32;
            let browser_initiated = transition & TT_FORWARD_BACK_FLAG as u32 != 0
                || [TT_EXPLICIT, TT_AUTO_BOOKMARK, TT_GENERATED, TT_RELOAD, TT_KEYWORD, TT_KEYWORD_GENERATED]
                    .iter()
                    .any(|t| *t as u32 == source);
            let from = CefString::from(&frame.url()).to_string();
            let to = CefString::from(&request.url()).to_string();
            let navigation = NavigationRequest {
                from: &from,
                to: &to,
                user_gesture: user_gesture != 0,
                server_redirect: is_redirect != 0,
                browser_initiated,
            };
            if policy.allow_navigation(&navigation) {
                return 0;
            }
            tracing::info!(%from, %to, "blocked a redirect");
            self.net.emit(browser.map(|b| &*b), EngineEvent::NavigationBlocked { view: self.view, url: to });
            1
        }

        fn resource_request_handler(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            let top_navigation = is_navigation != 0 && frame.is_some_and(|f| f.is_main() == 1);
            if let (true, Some(policy), Some(request), Some(initiator)) =
                (top_navigation, self.net.policy(), request, request_initiator)
            {
                let browser = browser.map(|b| &*b);
                let initiator = initiator.to_string();
                let url = CefString::from(&request.url()).to_string();
                let page = main_frame_url(browser);
                if !policy.allow_navigation_initiator(&page, &initiator, &url) {
                    tracing::info!(%page, %initiator, %url, "blocked a redirect by a frame of another site");
                    self.net.emit(browser, EngineEvent::NavigationBlocked { view: self.view, url });
                    return Some(CancelBuilder::new());
                }
            }
            Some(self.resources.clone())
        }
    }
}

/// Axion's own pages open only in a top-level frame, and only by the browser or from another
/// Axion page (the scheme is display-isolated anyway; this is the second lock).
fn refuse_internal_navigation(frame: &Frame, request: &Request) -> bool {
    let internal = |url: &str| url.strip_prefix(INTERNAL_SCHEME).is_some_and(|r| r.starts_with(':'));
    let to = CefString::from(&request.url()).to_string();
    if !internal(&to) {
        return false;
    }
    if frame.is_main() == 0 {
        tracing::warn!(%to, "refused an axion:// page in a frame");
        return true;
    }
    use cef_transition_type_t::*;
    let transition = *request.transition_type().as_ref() as u32;
    let source = transition & TT_SOURCE_MASK as u32;
    let browser_initiated = transition & TT_FORWARD_BACK_FLAG as u32 != 0
        || [
            TT_EXPLICIT,
            TT_AUTO_BOOKMARK,
            TT_GENERATED,
            TT_RELOAD,
            TT_KEYWORD,
            TT_KEYWORD_GENERATED,
            TT_AUTO_TOPLEVEL,
        ]
        .iter()
        .any(|t| *t as u32 == source);
    let from = CefString::from(&frame.url()).to_string();
    let refuse = !browser_initiated && !internal(&from) && !from.is_empty();
    if refuse {
        tracing::warn!(%from, %to, "refused a page's navigation to an axion:// page");
    }
    refuse
}

pub(super) fn build_request_handler(net: &Arc<Network>, view: ViewId) -> RequestHandler {
    let resources = ResourceRequestHandlerBuilder::new(Arc::clone(net), view, DenyCookiesBuilder::new());
    RequestHandlerBuilder::new(Arc::clone(net), view, resources)
}
