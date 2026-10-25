//! Downloads (CEF's download handler) and browsing data (cache, cookies). Everything here
//! runs on the main thread.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::*;

use crate::download::{DownloadInfo, DownloadOptions, DownloadState, unique_path};

/// Download state shared by every view's handler and the engine.
#[derive(Default)]
pub(super) struct Downloads {
    pub options: RefCell<Option<DownloadOptions>>,
    /// Updates since the last pump.
    pub updates: RefCell<Vec<DownloadInfo>>,
    /// The latest control callback of each running download.
    pub controls: RefCell<HashMap<u32, DownloadItemCallback>>,
    /// Cookie hosts and counts, filled by a cookie visit (see `visit_cookies`).
    pub cookie_sites: RefCell<BTreeMap<String, usize>>,
    /// A cookie visit finished and its result wasn't reported yet.
    pub cookie_visit_done: Cell<bool>,
    /// When the running cookie visit started, and whether the visitor was called yet.
    pub cookie_visit_started: Cell<Option<Instant>>,
    pub cookie_visit_seen: Cell<bool>,
}

/// CEF never calls the visitor when there are no cookies; after this long without a call
/// the visit counts as finished and empty.
const EMPTY_VISIT: Duration = Duration::from_millis(1000);

impl Downloads {
    /// A cookie visit that found nothing (see [`EMPTY_VISIT`]).
    pub fn cookie_visit_was_empty(&self) -> bool {
        !self.cookie_visit_seen.get()
            && self.cookie_visit_started.get().is_some_and(|t| t.elapsed() > EMPTY_VISIT)
    }
}

fn string(s: &CefStringUserfree) -> String {
    CefString::from(s).to_string()
}

fn info(item: &DownloadItem) -> DownloadInfo {
    let state = if item.is_complete() != 0 {
        DownloadState::Complete
    } else if item.is_canceled() != 0 {
        DownloadState::Cancelled
    } else if item.is_interrupted() != 0 {
        DownloadState::Failed
    } else if item.is_paused() != 0 {
        DownloadState::Paused
    } else {
        DownloadState::InProgress
    };
    let path = std::path::PathBuf::from(string(&item.full_path()));
    let mut file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if file_name.is_empty() {
        file_name = string(&item.suggested_file_name());
    }
    DownloadInfo {
        id: item.id(),
        url: string(&item.url()),
        path,
        file_name,
        mime_type: string(&item.mime_type()),
        received_bytes: item.received_bytes(),
        total_bytes: item.total_bytes(),
        speed: item.current_speed(),
        state,
    }
}

wrap_download_handler! {
    pub(super) struct DownloadHandlerBuilder {
        downloads: Rc<Downloads>,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else { return 0 };
            let options = self.downloads.options.borrow().clone();
            let Some(options) = options else {
                // Not configured: let CEF decide (its default directory).
                callback.cont(None, 0);
                return 1;
            };
            let name = suggested_name.map(|n| n.to_string()).unwrap_or_default();
            if let Err(e) = std::fs::create_dir_all(&options.directory) {
                tracing::warn!(dir = %options.directory.display(), "can't create the download folder: {e}");
            }
            let path = unique_path(&options.directory, &name);
            tracing::info!(path = %path.display(), url = %string(&item.url()), "download starting");
            let path_string: CefString = path.to_string_lossy().as_ref().into();
            callback.cont(Some(&path_string), options.ask as _);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            if item.is_valid() == 0 {
                return;
            }
            let info = info(item);
            let mut controls = self.downloads.controls.borrow_mut();
            match callback {
                Some(callback) if !info.state.is_finished() => {
                    controls.insert(info.id, callback.clone());
                }
                _ => {
                    controls.remove(&info.id);
                }
            }
            drop(controls);
            self.downloads.updates.borrow_mut().push(info);
        }
    }
}

pub(super) fn build_download_handler(downloads: &Rc<Downloads>) -> DownloadHandler {
    DownloadHandlerBuilder::new(downloads.clone())
}

wrap_cookie_visitor! {
    pub(super) struct CookieVisitorBuilder {
        downloads: Rc<Downloads>,
    }

    impl CookieVisitor {
        fn visit(
            &self,
            cookie: Option<&Cookie>,
            count: ::std::os::raw::c_int,
            total: ::std::os::raw::c_int,
            _delete_cookie: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            self.downloads.cookie_visit_seen.set(true);
            if let Some(cookie) = cookie {
                let domain = cookie.domain.to_string();
                let host = domain.trim_start_matches('.').trim_start_matches("www.").to_ascii_lowercase();
                if !host.is_empty() {
                    *self.downloads.cookie_sites.borrow_mut().entry(host).or_default() += 1;
                }
            }
            if count + 1 >= total {
                self.downloads.cookie_visit_done.set(true);
            }
            1
        }
    }
}

/// Starts listing the profile's cookies by site; the result arrives through `Downloads`.
pub(super) fn visit_cookies(context: &RequestContext, downloads: &Rc<Downloads>) {
    downloads.cookie_sites.borrow_mut().clear();
    downloads.cookie_visit_done.set(false);
    downloads.cookie_visit_seen.set(false);
    downloads.cookie_visit_started.set(Some(Instant::now()));
    let Some(manager) = context.cookie_manager(None) else {
        downloads.cookie_visit_done.set(true);
        return;
    };
    let mut visitor = CookieVisitorBuilder::new(downloads.clone());
    // No cookie store: nothing to wait for.
    if manager.visit_all_cookies(Some(&mut visitor)) == 0 {
        downloads.cookie_visit_done.set(true);
    }
}

/// Deletes the cookies of `host` (and its subdomains' cookies set for it), or all cookies.
pub(super) fn delete_cookies(context: &RequestContext, host: Option<&str>) {
    let Some(manager) = context.cookie_manager(None) else { return };
    match host {
        None => {
            manager.delete_cookies(None, None, Some(&mut CookiesDeletedBuilder::new()));
        }
        Some(host) => {
            for url in [format!("https://{host}/"), format!("http://{host}/"), format!("https://www.{host}/")]
            {
                let mut done = CookiesDeletedBuilder::new();
                manager.delete_cookies(Some(&url.as_str().into()), None, Some(&mut done));
            }
        }
    }
}

wrap_completion_callback! {
    pub(super) struct CacheClearedBuilder;

    impl CompletionCallback {
        fn on_complete(&self) {
            tracing::info!("HTTP cache cleared");
        }
    }
}

wrap_delete_cookies_callback! {
    pub(super) struct CookiesDeletedBuilder;

    impl DeleteCookiesCallback {
        fn on_complete(&self, num_deleted: ::std::os::raw::c_int) {
            tracing::info!(num_deleted, "cookies deleted");
        }
    }
}

/// Clears the HTTP cache. (CEF crashes when the completion callback is left out.)
pub(super) fn clear_cache(context: &RequestContext) {
    context.clear_http_cache(Some(&mut CacheClearedBuilder::new()));
}
