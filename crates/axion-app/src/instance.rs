//! One Axion per account: a second `axion` started on a profile that is already open (a link
//! clicked in another app, `axion URL`, the application menu) hands its URLs to the running
//! window and exits.
//!
//! Why this exists: Chromium keeps a process singleton per profile. A second Chromium process
//! on the same profile does not start; it passes its command line to the first one, whose
//! default answer is to open a browser window of its own (Chromium's UI, not Axion's), and the
//! second process fails to initialize. Axion claims the profile first, before the window or
//! Chromium exist: the first process listens on a Unix socket, later ones connect to it,
//! send the URLs and exit. (`on_already_running_app_relaunch` in the engine is the net below
//! this for launches that reach Chromium anyway.)
//!
//! The socket is named after the profile directory, so another account, or another
//! `AXION_PROFILE_DIR` (test runs), is a separate instance. It lives in `$XDG_RUNTIME_DIR`
//! (private to the user) or else in a private directory under the temp dir. A socket left by a
//! crashed process refuses connections and is replaced.
//!
//! Wire format (one connection per launch, UTF-8 lines): `AXION-OPEN 1`, `focus 0|1`, any
//! number of `url <text>` (`%`, CR and LF percent-encoded), `end`; the answer is `ok`.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axion_engine::cef::Waker;

const HELLO: &str = "AXION-OPEN 1";
const ANSWER: &str = "ok";
/// A connection that says nothing for this long is dropped, so one stuck client never blocks
/// the listener; a client waits this long for the running window's answer.
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// Most URLs one launch may hand over.
const MAX_URLS: usize = 256;
/// Longest line accepted (bytes).
const MAX_LINE: u64 = 64 * 1024;
/// Unix socket paths are limited to about 108 bytes.
const MAX_SOCKET_PATH: usize = 100;

/// What a launch asks the running window to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub urls: Vec<String>,
    /// Bring the window to the front (the launching process was not a test run).
    pub focus: bool,
}

/// The URLs among a launch's arguments, as the running window should open them: existing paths
/// (relative to the launching directory `cwd`) become `file://` URLs, everything else (URLs,
/// `about:`, bare hostnames) stays as typed. Switches (`--…`) are not URLs.
pub fn urls_from_args(args: &[String], cwd: &Path) -> Vec<String> {
    args.iter().filter(|a| !a.starts_with('-')).map(|a| url_for_arg(a, cwd)).collect()
}

fn url_for_arg(arg: &str, cwd: &Path) -> String {
    let has_scheme = arg.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    if has_scheme {
        return arg.to_owned();
    }
    let path = cwd.join(arg);
    if path.exists() { file_url(&path) } else { arg.to_owned() }
}

/// `path` as a `file://` URL (percent-encoded; lexically normalised).
pub fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut parts: Vec<&[u8]> = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(part) => parts.push(part.as_bytes()),
            std::path::Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    let mut out = String::from("file://");
    for part in parts {
        out.push('/');
        for &b in part {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    if out == "file://" { "file:///".to_owned() } else { out }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("%0A", "\n").replace("%0D", "\r").replace("%25", "%")
}

/// The request as it goes over the socket.
pub fn encode(request: &Request) -> String {
    let mut out = format!("{HELLO}\nfocus {}\n", u8::from(request.focus));
    for url in request.urls.iter().take(MAX_URLS) {
        out.push_str(&format!("url {}\n", escape(url)));
    }
    out.push_str("end\n");
    out
}

/// Reads one request; `None` when the peer did not speak this protocol.
pub fn decode(reader: &mut impl BufRead) -> io::Result<Option<Request>> {
    use std::io::Read as _;
    let mut request = Request::default();
    let mut first = true;
    loop {
        let mut line = String::new();
        if reader.by_ref().take(MAX_LINE).read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if first {
            if line != HELLO {
                return Ok(None);
            }
            first = false;
        } else if line == "end" {
            return Ok(Some(request));
        } else if let Some(flag) = line.strip_prefix("focus ") {
            request.focus = flag == "1";
        } else if let Some(url) = line.strip_prefix("url ")
            && request.urls.len() < MAX_URLS
        {
            request.urls.push(unescape(url));
        }
    }
}

/// FNV-1a: stable across runs and builds (unlike `DefaultHasher`).
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The socket of the instance running on `profile`.
pub fn socket_path(profile: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(profile)?;
    let canonical = std::fs::canonicalize(profile)?;
    let name = format!("axion-{:016x}.sock", fnv(canonical.to_string_lossy().as_bytes()));
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = match runtime {
        Some(dir) if dir.join(&name).as_os_str().len() <= MAX_SOCKET_PATH => dir,
        _ => {
            let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("axion-run-{uid}"));
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
            dir
        }
    };
    Ok(dir.join(name))
}

/// Requests that arrived from other launches, waiting for the browser to act on them.
pub struct Inbox {
    queue: Arc<Mutex<Vec<Request>>>,
    path: PathBuf,
}

impl Inbox {
    /// Takes what has arrived.
    pub fn drain(&self) -> Vec<Request> {
        std::mem::take(&mut *self.queue.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A handle other threads add requests through (Chromium's relaunch hook).
    pub fn pusher(&self, wake: Option<Waker>) -> impl Fn(Request) + Send + Sync + 'static {
        let queue = self.queue.clone();
        move |request| {
            queue.lock().unwrap_or_else(PoisonError::into_inner).push(request);
            if let Some(wake) = &wake {
                wake.wake();
            }
        }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // Only the owner removes it; a failure leaves a stale socket, which the next launch replaces.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The outcome of [`claim`].
pub enum Claim {
    /// This process is the instance: other launches arrive in the inbox.
    Primary(Inbox),
    /// Another instance owns the profile and has the request; this process should exit.
    Forwarded,
}

/// Becomes the instance of `profile`, or hands `request` to the one that is.
pub fn claim(profile: &Path, request: &Request, wake: Option<Waker>) -> io::Result<Claim> {
    claim_at(&socket_path(profile)?, request, move || {
        if let Some(wake) = &wake {
            wake.wake();
        }
    })
}

/// [`claim`] with the socket named (tests use their own).
pub fn claim_at(
    path: &Path,
    request: &Request,
    wake: impl Fn() + Send + Sync + 'static,
) -> io::Result<Claim> {
    // Twice: a stale socket is removed after the first refusal.
    for _ in 0..2 {
        match UnixListener::bind(path) {
            Ok(listener) => return Ok(Claim::Primary(listen(listener, path, wake))),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => match send(path, request) {
                Ok(()) => return Ok(Claim::Forwarded),
                Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound) => {
                    // Nobody listens: left behind by a process that died.
                    let _ = std::fs::remove_file(path);
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AddrInUse, "could not take over the instance socket"))
}

fn send(path: &Path, request: &Request) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(encode(request).as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    if answer.trim() == ANSWER {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "the running Axion did not accept the request"))
    }
}

fn listen(listener: UnixListener, path: &Path, wake: impl Fn() + Send + Sync + 'static) -> Inbox {
    let queue = Arc::new(Mutex::new(Vec::new()));
    let inbox = Inbox { queue: queue.clone(), path: path.to_owned() };
    let spawned = std::thread::Builder::new().name("instance-socket".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(request) = serve(stream) {
                queue.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                wake();
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("single instance: no listener thread: {e}");
    }
    inbox
}

fn serve(stream: UnixStream) -> Option<Request> {
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    let request = decode(&mut BufReader::new(&stream)).ok()??;
    // Answered before it is acted on: the launching process can exit.
    (&stream).write_all(format!("{ANSWER}\n").as_bytes()).ok()?;
    Some(request)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn requests_survive_the_wire() {
        let request = Request {
            urls: vec![
                "https://a.example/?q=50%25&x=1".into(),
                "line\nbreak\r%".into(),
                "file:///tmp/a b.html".into(),
            ],
            focus: true,
        };
        let text = encode(&request);
        assert_eq!(text.lines().count(), 6, "one line per URL: {text}");
        let back = decode(&mut text.as_bytes()).expect("reads").expect("speaks the protocol");
        assert_eq!(back, request);
    }

    #[test]
    fn strangers_are_refused() {
        assert_eq!(decode(&mut "GET / HTTP/1.1\r\n\r\n".as_bytes()).expect("reads"), None);
        assert_eq!(decode(&mut "".as_bytes()).expect("reads"), None);
        // Cut off before `end`.
        assert_eq!(decode(&mut format!("{HELLO}\nurl x\n").as_bytes()).expect("reads"), None);
    }

    #[test]
    fn arguments_become_urls() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("page one.html"), "x").expect("file");
        let args: Vec<String> =
            ["--capture=a.png", "https://x.example/a", "page one.html", "example.org", "about:blank", "-h"]
                .map(String::from)
                .into();
        let urls = urls_from_args(&args, dir.path());
        let expected_file = format!("file://{}/page%20one.html", dir.path().to_string_lossy());
        assert_eq!(urls, ["https://x.example/a", expected_file.as_str(), "example.org", "about:blank"]);
        assert_eq!(file_url(Path::new("/a/b/../c d")), "file:///a/c%20d");
    }

    #[test]
    fn the_second_launch_hands_its_urls_to_the_first() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("i.sock");
        let woke = Arc::new(AtomicUsize::new(0));
        let counter = woke.clone();
        let first = match claim_at(&path, &Request::default(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("first claim")
        {
            Claim::Primary(inbox) => inbox,
            Claim::Forwarded => panic!("the first launch is the instance"),
        };
        let request = Request { urls: vec!["https://example.com/".into()], focus: true };
        assert!(matches!(claim_at(&path, &request, || {}).expect("second claim"), Claim::Forwarded));
        // The answer comes before the queue is filled: wait for it.
        for _ in 0..100 {
            if woke.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(first.drain(), [request]);
        assert!(first.drain().is_empty());
        drop(first);
        assert!(!path.exists(), "the owner removes its socket");
    }

    #[test]
    fn a_dead_instances_socket_is_replaced() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("i.sock");
        // A socket file nobody listens on (the listener was dropped without cleanup).
        drop(UnixListener::bind(&path).expect("bind"));
        assert!(path.exists());
        assert!(matches!(claim_at(&path, &Request::default(), || {}).expect("claim"), Claim::Primary(_)));
    }

    #[test]
    fn profiles_have_their_own_sockets() {
        let a = tempfile::tempdir().expect("a");
        let b = tempfile::tempdir().expect("b");
        let (pa, pb) = (socket_path(a.path()).expect("a"), socket_path(b.path()).expect("b"));
        assert_ne!(pa, pb);
        assert_eq!(pa, socket_path(a.path()).expect("again"));
        assert!(pa.as_os_str().len() <= MAX_SOCKET_PATH + 10);
    }
}
