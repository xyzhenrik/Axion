//! The media view's way to its stream. The view lives on `axion://media`, an origin no hoster
//! or CDN lists in its CORS headers (so hls.js could never read a manifest or a segment
//! itself), and cross-site to every host (so Chromium would not send the host's cookies with
//! a plain `<video src>` either). The view's requests for them are answered here instead: the
//! browser fetches the very address the view asked for (Shields judged the request before it
//! got here) with the Referer / Origin the source needs and the profile's cookies for that
//! host, and streams the response to the view, marked readable. Ranges are passed on as asked
//! and the body is streamed through a small buffer, so a film is never held in memory.
//! Nothing is stored.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cef::*;

const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36";
/// The most the fetch reads ahead of the view (it waits when the view lags behind).
const READ_AHEAD: usize = 4 * 1024 * 1024;
const PIECE: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a fetch waits for the cookie store to list the cookies of the address.
const COOKIE_WAIT: Duration = Duration::from_millis(500);

/// What the fetch tells about the response before its body.
#[derive(Default, Clone)]
struct Head {
    status: i32,
    mime: String,
    /// The size of the whole resource (-1: unknown).
    total: i64,
    /// Where the body we got starts in the resource (not 0 when the host answered a range).
    start: u64,
}

/// `(first byte, size of the whole resource)` of a `Content-Range: bytes 10-99/1000` value.
fn parse_content_range(value: &str) -> Option<(u64, i64)> {
    let rest = value.trim().strip_prefix("bytes")?.trim();
    let (span, total) = rest.split_once('/')?;
    let (first, _) = span.split_once('-')?;
    Some((first.trim().parse().ok()?, total.trim().parse().unwrap_or(-1)))
}

/// The read the view is waiting on (its buffer stays valid until it is answered).
struct Waiting {
    out: usize,
    capacity: usize,
    callback: ResourceReadCallback,
}

#[derive(Default)]
struct Stream {
    head: Head,
    queue: VecDeque<Vec<u8>>,
    queued: usize,
    /// The body is complete (or the fetch failed).
    done: bool,
    /// The view gave up.
    cancelled: bool,
    /// Bytes at the start of the body the view asked to skip (a range the host ignored).
    skip: u64,
    waiting: Option<Waiting>,
}

type Shared = Arc<Mutex<Stream>>;

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, Stream> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

wrap_cookie_visitor! {
    struct CollectorBuilder {
        jar: Arc<Mutex<Vec<String>>>,
        // Dropped with the visitor (Chromium releases it once it has listed everything), which
        // tells the waiting fetch that the list is complete even when it was empty.
        _done: Sender<()>,
    }

    impl CookieVisitor {
        fn visit(
            &self,
            cookie: Option<&Cookie>,
            _count: ::std::os::raw::c_int,
            _total: ::std::os::raw::c_int,
            _delete_cookie: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(cookie) = cookie {
                let pair = format!("{}={}", cookie.name, cookie.value);
                self.jar.lock().unwrap_or_else(PoisonError::into_inner).push(pair);
            }
            1
        }
    }
}

type Pending = Option<(Receiver<()>, Arc<Mutex<Vec<String>>>)>;

/// Starts listing the cookies for `url` (Chromium answers on its own thread).
fn ask_cookies(manager: Option<&CookieManager>, url: &str) -> Pending {
    let manager = manager?;
    let (tx, rx) = channel();
    let jar = Arc::new(Mutex::new(Vec::new()));
    let mut visitor = CollectorBuilder::new(jar.clone(), tx);
    (manager.visit_url_cookies(Some(&url.into()), 1, Some(&mut visitor)) != 0).then_some((rx, jar))
}

/// The cookies as a `Cookie` header value (empty: none); waits (bounded) for the store.
fn cookie_header(pending: Pending) -> String {
    let Some((done, jar)) = pending else { return String::new() };
    // The only message is the sender's disconnect.
    let _ = done.recv_timeout(COOKIE_WAIT);
    jar.lock().unwrap_or_else(PoisonError::into_inner).join("; ")
}

/// Delivers what is queued to the waiting read, if any.
fn feed(state: &Shared) {
    let mut s = lock(state);
    let Some(waiting) = s.waiting.take() else { return };
    let n = take_into(&mut s, waiting.out as *mut u8, waiting.capacity);
    if n == 0 && !s.done && !s.cancelled {
        s.waiting = Some(waiting);
        return;
    }
    drop(s);
    waiting.callback.cont(n as ::std::os::raw::c_int);
}

/// Moves queued bytes into the view's buffer; returns how many.
fn take_into(s: &mut Stream, out: *mut u8, capacity: usize) -> usize {
    let mut written = 0;
    while written < capacity {
        let Some(front) = s.queue.front_mut() else { break };
        let n = front.len().min(capacity - written);
        // SAFETY: the view gave `out` for `capacity` bytes and keeps it valid until the read is
        // answered; `written + n <= capacity`, and `front` holds at least `n` bytes.
        unsafe { std::ptr::copy_nonoverlapping(front.as_ptr(), out.add(written), n) };
        written += n;
        s.queued -= n;
        if n == front.len() {
            s.queue.pop_front();
        } else {
            front.drain(..n);
        }
    }
    written
}

/// Runs the request and feeds the response into `state` (on its own thread).
fn run(state: &Shared, url: &str, referer: &str, range: Option<&str>, cookies: Pending, opened: &Callback) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
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
    // What a hoster checks of a player on its own page: where it is embedded, and its cookies.
    let origin = axion_media::origin_of(referer);
    if !origin.is_empty() {
        request = request.header("Origin", origin);
    }
    let cookie = cookie_header(cookies);
    let sent_cookies = !cookie.is_empty();
    if sent_cookies {
        request = request.header("Cookie", cookie);
    }
    let started = std::time::Instant::now();
    let response = match request.call() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%url, "media view fetch failed: {e}");
            lock(state).head = Head { status: 502, total: 0, ..Default::default() };
            lock(state).done = true;
            opened.cont();
            return;
        }
    };
    let header = |name: &str| {
        response.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
    };
    let status = i32::from(response.status().as_u16());
    let content_range = header("content-range");
    let (start, total) = match (status, parse_content_range(&content_range)) {
        (206, Some((start, total))) => (start, total),
        _ => (0, header("content-length").parse().unwrap_or(-1)),
    };
    let head = Head { status, mime: header("content-type"), total, start };
    tracing::debug!(
        %url, status, mime = %head.mime, range = range.unwrap_or(""), %content_range, total,
        start, cookies = sent_cookies, ms = started.elapsed().as_millis() as u64, "media view fetch"
    );
    lock(state).head = head;
    opened.cont();
    let mut body = response.into_body().into_reader();
    let mut total = 0usize;
    let mut piece = vec![0u8; PIECE];
    loop {
        // The view lags behind: wait for it (or for it to give up).
        loop {
            let s = lock(state);
            if s.cancelled {
                return;
            }
            if s.queued < READ_AHEAD {
                break;
            }
            drop(s);
            std::thread::sleep(Duration::from_millis(15));
        }
        match body.read(&mut piece) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                let mut s = lock(state);
                // A range the host ignored: the view's skip drops the start of the body.
                let drop_n = usize::try_from(s.skip).unwrap_or(usize::MAX).min(n);
                s.skip -= drop_n as u64;
                if drop_n == n {
                    continue;
                }
                s.queue.push_back(piece[drop_n..n].to_vec());
                let n = n - drop_n;
                s.queued += n;
                drop(s);
                feed(state);
            }
            Err(e) => {
                tracing::warn!(%url, total, "reading a media view response failed: {e}");
                break;
            }
        }
    }
    tracing::debug!(%url, total, ms = started.elapsed().as_millis() as u64, "media view fetch done");
    lock(state).done = true;
    feed(state);
}

/// A resource handler answering `url` through [`run`].
pub(super) fn handler(
    url: String,
    referer: String,
    range: Option<String>,
    cookies: Option<CookieManager>,
) -> ResourceHandler {
    ProxyHandlerBuilder::new(url, referer, range, cookies, Arc::new(Mutex::new(Stream::default())))
}

wrap_resource_handler! {
    pub(super) struct ProxyHandlerBuilder {
        url: String,
        referer: String,
        range: Option<String>,
        cookies: Option<CookieManager>,
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
            let cookies = ask_cookies(self.cookies.as_ref(), &url);
            let (state, callback) = (self.state.clone(), callback.clone());
            let spawned = std::thread::Builder::new().name("media-view-fetch".into()).spawn(move || {
                run(&state, &url, &referer, range.as_deref(), cookies, &callback);
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
            let head = lock(&self.state).head.clone();
            // The host's answer, as it is: a ranged request gets the 206 and its Content-Range.
            // (Chromium asks `skip` to drop the bytes before the range; the host started
            // there already, see `skip`.)
            response.set_status(head.status);
            let mime = if head.mime.is_empty() { "application/octet-stream" } else { head.mime.as_str() };
            response.set_mime_type(Some(&mime.into()));
            let set = |name: &str, value: &str| {
                response.set_header_by_name(Some(&name.into()), Some(&value.into()), 1);
            };
            // The point of the proxy: the view may read this response.
            set("Access-Control-Allow-Origin", "*");
            set("Access-Control-Expose-Headers", "*");
            set("Cache-Control", "no-store");
            set("Accept-Ranges", "bytes");
            if head.status == 206 {
                let end = head.total - 1;
                set("Content-Range", &format!("bytes {}-{}/{}", head.start, end, head.total));
                set("Content-Length", &(head.total - head.start as i64).to_string());
            } else if head.total >= 0 {
                set("Content-Length", &head.total.to_string());
            }
            if let Some(length) = response_length {
                *length = -1;
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(bytes_read) = bytes_read else { return 0 };
            *bytes_read = 0;
            let Ok(capacity) = usize::try_from(bytes_to_read) else { return 0 };
            if data_out.is_null() || capacity == 0 {
                return 0;
            }
            let mut s = lock(&self.state);
            let n = take_into(&mut s, data_out, capacity);
            if n > 0 {
                *bytes_read = n as ::std::os::raw::c_int;
                return 1;
            }
            if s.done || s.cancelled {
                // The end of the body.
                return 0;
            }
            // Nothing yet: the fetch answers the read when data arrives.
            let Some(callback) = callback else { return 0 };
            s.waiting = Some(Waiting { out: data_out as usize, capacity, callback: callback.clone() });
            1
        }

        fn skip(
            &self,
            bytes_to_skip: i64,
            bytes_skipped: Option<&mut i64>,
            _callback: Option<&mut ResourceSkipCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(bytes_skipped) = bytes_skipped else { return 0 };
            let mut s = lock(&self.state);
            let wanted = u64::try_from(bytes_to_skip).unwrap_or(0);
            // The host started at `start` already: those bytes are skipped.
            let rest = wanted.saturating_sub(s.head.start);
            // More to drop (a range the host ignored): what is queued goes, then the fetch
            // drops the rest as it arrives.
            let mut remaining = rest;
            {
                let Stream { queue, queued, .. } = &mut *s;
                while remaining > 0 {
                    let Some(front) = queue.front_mut() else { break };
                    let n = usize::try_from(remaining).unwrap_or(usize::MAX).min(front.len());
                    front.drain(..n);
                    *queued -= n;
                    remaining -= n as u64;
                    if front.is_empty() {
                        queue.pop_front();
                    }
                }
            }
            s.skip += remaining;
            *bytes_skipped = bytes_to_skip.max(1);
            1
        }

        fn cancel(&self) {
            let mut s = lock(&self.state);
            s.cancelled = true;
            s.waiting = None;
        }
    }
}
