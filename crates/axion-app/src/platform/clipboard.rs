//! Reading the desktop clipboard for a paste into a page, where the engine can't reach the
//! clipboard itself (see `Engine::has_system_clipboard`: off-screen Chromium on Wayland).
//!
//! The clipboard is read on a thread of its own: the app that owns the clipboard sends its
//! data through a pipe and may take a while (a large screenshot), and the UI must never wait.
//! On Wayland it is read through the data-control protocol, which works without keyboard
//! focus; text falls back to `arboard` where that isn't available.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use axion_engine::{PasteData, PastedFile, ViewId};

/// Plain text, best first.
const TEXT_TYPES: &[&str] = &["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];
/// Images a page can take, best first, and the file name each gets (as in Chrome).
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("image/png", "image.png"),
    ("image/jpeg", "image.jpg"),
    ("image/webp", "image.webp"),
    ("image/gif", "image.gif"),
    ("image/bmp", "image.bmp"),
];
/// Copied files (a file manager's copy).
const URI_LIST: &str = "text/uri-list";
const HTML: &str = "text/html";

/// Pending clipboard reads, one per paste, each for a target `T`: a page (`ViewId`) or a field
/// of the chrome (its node).
pub struct ClipboardReader<T = ViewId> {
    pending: Vec<Receiver<(T, PasteData)>>,
}

impl<T> Default for ClipboardReader<T> {
    fn default() -> Self {
        Self { pending: Vec::new() }
    }
}

impl<T: Send + 'static> ClipboardReader<T> {
    /// Starts reading the clipboard for a paste into `target`; the result comes from
    /// [`ClipboardReader::poll`]. `plain` reads only the text. Images and files over
    /// `max_bytes` are left out.
    pub fn read(&mut self, target: T, plain: bool, max_bytes: u64) {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("clipboard".into()).spawn(move || {
            let data = read_clipboard(plain, max_bytes);
            // The browser may be gone by now; nothing to do then.
            let _ = tx.send((target, data));
        });
        match spawned {
            Ok(_) => self.pending.push(rx),
            Err(e) => tracing::warn!("reading the clipboard failed: {e}"),
        }
    }

    /// A finished read, in the order they were started.
    pub fn poll(&mut self) -> Option<(T, PasteData)> {
        let first = self.pending.first()?;
        match first.try_recv() {
            Ok(result) => {
                self.pending.remove(0);
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending.remove(0);
                None
            }
        }
    }
}

fn read_clipboard(plain: bool, max_bytes: u64) -> PasteData {
    match read_wayland(plain, max_bytes) {
        Ok(data) => data,
        Err(e) => {
            tracing::debug!("no Wayland clipboard access ({e}); reading text only");
            let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok();
            PasteData { text, ..Default::default() }
        }
    }
}

fn read_wayland(plain: bool, max_bytes: u64) -> Result<PasteData, wl_clipboard_rs::paste::Error> {
    use wl_clipboard_rs::paste::{ClipboardType, Error, Seat, get_mime_types};

    let offered = match get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
        Ok(types) => types,
        Err(Error::ClipboardEmpty | Error::NoSeats) => return Ok(PasteData::default()),
        Err(e) => return Err(e),
    };
    let mut data = PasteData::default();
    if let Some(mime) = TEXT_TYPES.iter().find(|t| offered.contains(**t)) {
        data.text = read_type(mime, max_bytes).map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    if plain {
        return Ok(data);
    }
    if offered.contains(HTML) {
        data.html = read_type(HTML, max_bytes).map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    if offered.contains(URI_LIST)
        && let Some(list) = read_type(URI_LIST, max_bytes)
    {
        let mut budget = max_bytes;
        for path in file_paths(&String::from_utf8_lossy(&list)) {
            match read_file(&path, budget) {
                Some(file) => {
                    budget = budget.saturating_sub(file.bytes.len() as u64);
                    data.files.push(file);
                }
                None => tracing::debug!(?path, "not pasting a file: unreadable or too large"),
            }
        }
    }
    if data.files.is_empty()
        && let Some((mime, name)) = IMAGE_TYPES.iter().find(|(t, _)| offered.contains(*t))
    {
        if let Some(bytes) = read_type(mime, max_bytes) {
            data.files.push(PastedFile { name: (*name).to_owned(), mime: (*mime).to_owned(), bytes });
        } else {
            tracing::debug!(mime, "not pasting the clipboard image: unreadable or too large");
        }
    }
    Ok(data)
}

/// The clipboard's data in `mime`, if it is at most `max_bytes`.
fn read_type(mime: &str, max_bytes: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};

    let (pipe, _) = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(mime))
        .inspect_err(|e| tracing::debug!(mime, "reading the clipboard failed: {e}"))
        .ok()?;
    let mut bytes = Vec::new();
    pipe.take(max_bytes.saturating_add(1)).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max_bytes).then_some(bytes)
}

fn read_file(path: &std::path::Path, max_bytes: u64) -> Option<PastedFile> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some(PastedFile { mime: mime_for(&name).to_owned(), name, bytes })
}

/// Local files in a `text/uri-list` (`file://` URIs, one per line, `#` comments).
fn file_paths(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|uri| {
            let rest = uri.strip_prefix("file://")?;
            // `file:///path` or `file://localhost/path`.
            let path = rest.strip_prefix("localhost").unwrap_or(rest);
            path.starts_with('/').then(|| PathBuf::from(percent_decode(path)))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(&h), Some(&l)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A MIME type for a file name, for the common kinds pages accept (Chromium decides for the
/// rest by itself).
fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_become_paths() {
        let list =
            "# copied\r\nfile:///home/a/My%20File.png\r\nfile://localhost/tmp/x.txt\nhttps://example.com/y\n";
        assert_eq!(file_paths(list), vec![PathBuf::from("/home/a/My File.png"), PathBuf::from("/tmp/x.txt")]);
    }

    #[test]
    fn percent_decoding_keeps_utf8() {
        assert_eq!(percent_decode("/t/%C3%BCber%2"), "/t/über%2");
    }

    #[test]
    fn mime_types_from_names() {
        assert_eq!(mime_for("Shot.PNG"), "image/png");
        assert_eq!(mime_for("a.tar.gz"), "application/octet-stream");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }
}
