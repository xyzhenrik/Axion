//! Finding the real media behind a page: which address a `<video>` plays (or, for `blob:`
//! streams the page assembles itself, which manifest or file the page fetched), how it must
//! be requested (Referer / Origin), and what kind of source it is. The Playlist stores a
//! [`MediaSource`] with each item and plays it in a view of its own, without the page.

use serde::{Deserialize, Serialize};

/// How a source is played.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    /// A plain file the media element plays by itself (mp4, webm, mp3…).
    File,
    /// An HLS playlist (`.m3u8`), played with the bundled hls.js.
    Hls,
    /// A DASH manifest (`.mpd`): recognised, but not playable without the page's player.
    Dash,
}

/// A captured media source with what its server needs to see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSource {
    pub url: String,
    pub kind: StreamKind,
    /// The `Referer` the page sent (empty: none). Hosters often check it. (The `Origin` can't
    /// be replaced: Chromium's network stack refuses a request whose Origin was changed.)
    #[serde(default)]
    pub referer: String,
    /// The stream is live (a channel, not a recording). Its address is signed for a short time
    /// (Twitch…), so a saved one is never reused: playing looks at the page again.
    #[serde(default)]
    pub live: bool,
}

impl MediaSource {
    pub fn new(url: &str, kind: StreamKind, referer: &str) -> Self {
        let referer = referer.trim();
        Self {
            url: url.trim().to_owned(),
            kind,
            referer: if referer.starts_with("http") { referer.to_owned() } else { String::new() },
            live: false,
        }
    }

    pub fn with_live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    /// Whether the dedicated media view can play it.
    pub fn playable(&self) -> bool {
        matches!(self.kind, StreamKind::File | StreamKind::Hls)
    }

    /// The request headers to send for this source's requests (what the page's own requests
    /// carried that its host may check).
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if !self.referer.is_empty() {
            out.push(("Referer".to_owned(), self.referer.clone()));
        }
        out
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// `None` for empty or malformed text.
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok().filter(|s: &Self| s.url.starts_with("http"))
    }
}

/// `https://host:port` of an address (empty for anything that has no origin).
pub fn origin_of(url: &str) -> String {
    let url = url.trim();
    let Some(scheme_end) = url.find("://") else { return String::new() };
    let scheme = &url[..scheme_end];
    if scheme != "http" && scheme != "https" {
        return String::new();
    }
    let rest = &url[scheme_end + 3..];
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // `user:pass@host` is not part of an origin.
    let host = host.rsplit('@').next().unwrap_or_default();
    if host.is_empty() { String::new() } else { format!("{scheme}://{host}") }
}

/// A media-looking response a page's network traffic showed (reported by the engine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sniffed {
    pub url: String,
    pub mime: String,
    /// The page or frame that requested it.
    pub referer: String,
}

fn path_of(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase()
}

/// What a network response is, if it is a media manifest or file worth capturing; `None` for
/// segments, subtitles, images and everything else.
pub fn classify_response(url: &str, mime: &str) -> Option<StreamKind> {
    let path = path_of(url);
    let mime = mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    // Pieces of a stream are not the stream.
    const PIECES: [&str; 9] = [".ts", ".m4s", ".aac", ".cmfv", ".cmfa", ".vtt", ".srt", ".m4a.part", ".key"];
    if PIECES.iter().any(|p| path.ends_with(p)) || mime == "video/mp2t" || mime == "video/iso.segment" {
        return None;
    }
    if mime.contains("mpegurl") || path.ends_with(".m3u8") || path.ends_with(".m3u") {
        return Some(StreamKind::Hls);
    }
    if mime == "application/dash+xml" || path.ends_with(".mpd") {
        return Some(StreamKind::Dash);
    }
    const FILES: [&str; 8] = [".mp4", ".webm", ".m4v", ".mov", ".ogv", ".mp3", ".ogg", ".opus"];
    if mime.starts_with("video/") || FILES.iter().any(|e| path.ends_with(e)) {
        return Some(StreamKind::File);
    }
    None
}

/// What capturing found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capture {
    Found(MediaSource),
    /// A source exists but can't be played outside the page; the text says why.
    Unsupported(&'static str),
    /// Nothing yet (the page may still be loading).
    Nothing,
}

pub const WHY_DASH: &str =
    axion_i18n::tr_mark("This video uses a DASH stream, which Axion can only play on its page.");
pub const WHY_BLOB: &str =
    axion_i18n::tr_mark("This video is assembled by its page and can only be played there.");

/// The source of a video: the address its element plays (`player_src`, from the frame
/// `player_frame`), else — for `blob:` streams and elements without a source yet — the best
/// of the manifests and files the page's network traffic showed.
pub fn capture(player_src: &str, player_frame: &str, page_url: &str, sniffed: &[Sniffed]) -> Capture {
    let src = player_src.trim();
    if src.starts_with("http://") || src.starts_with("https://") {
        let kind = classify_response(src, "").unwrap_or(StreamKind::File);
        let referer = sniffed
            .iter()
            .find(|s| s.url == src)
            .map(|s| s.referer.as_str())
            .filter(|r| !r.is_empty())
            .or(Some(player_frame).filter(|f| f.starts_with("http")))
            .unwrap_or(page_url);
        let source = MediaSource::new(src, kind, referer);
        return if source.playable() {
            Capture::Found(source)
        } else {
            Capture::Unsupported(axion_i18n::tr(WHY_DASH))
        };
    }
    // A manifest beats the files (a master playlist is requested before its variants, so
    // the earliest one is the master); a plain file is the most recent one.
    let hls = sniffed.iter().find(|s| classify_response(&s.url, &s.mime) == Some(StreamKind::Hls));
    let file = sniffed.iter().rev().find(|s| classify_response(&s.url, &s.mime) == Some(StreamKind::File));
    let dash = sniffed.iter().any(|s| classify_response(&s.url, &s.mime) == Some(StreamKind::Dash));
    let found = hls.map(|s| (s, StreamKind::Hls)).or_else(|| file.map(|s| (s, StreamKind::File)));
    match found {
        Some((s, kind)) => {
            let referer = if s.referer.is_empty() { page_url } else { s.referer.as_str() };
            Capture::Found(MediaSource::new(&s.url, kind, referer))
        }
        None if dash => Capture::Unsupported(axion_i18n::tr(WHY_DASH)),
        None if src.starts_with("blob:") => Capture::Unsupported(axion_i18n::tr(WHY_BLOB)),
        None => Capture::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sniffed(url: &str, mime: &str, referer: &str) -> Sniffed {
        Sniffed { url: url.into(), mime: mime.into(), referer: referer.into() }
    }

    #[test]
    fn responses_are_classified() {
        assert_eq!(classify_response("https://x/v.mp4?t=1", ""), Some(StreamKind::File));
        assert_eq!(classify_response("https://x/blob", "video/webm; codecs=vp9"), Some(StreamKind::File));
        assert_eq!(classify_response("https://x/m.M3U8", ""), Some(StreamKind::Hls));
        assert_eq!(
            classify_response("https://x/play", "application/vnd.apple.mpegurl"),
            Some(StreamKind::Hls)
        );
        assert_eq!(classify_response("https://x/m.mpd", ""), Some(StreamKind::Dash));
        assert_eq!(classify_response("https://x/seg1.ts", "video/mp2t"), None);
        assert_eq!(classify_response("https://x/a.m4s", "video/mp4"), None);
        assert_eq!(classify_response("https://x/a.png", "image/png"), None);
        assert_eq!(classify_response("https://x/a.js", "application/javascript"), None);
    }

    #[test]
    fn origins() {
        assert_eq!(origin_of("https://voe.sx:8443/e/abc?x=1"), "https://voe.sx:8443");
        assert_eq!(origin_of("http://u:p@host/x"), "http://host");
        assert_eq!(origin_of("blob:https://x/1"), "");
        assert_eq!(origin_of(""), "");
    }

    #[test]
    fn sources_carry_their_headers_and_round_trip() {
        let s = MediaSource::new("https://cdn/a.m3u8", StreamKind::Hls, "https://host.example/embed/1?x");
        assert_eq!(s.headers(), [("Referer".to_owned(), "https://host.example/embed/1?x".to_owned())]);
        assert_eq!(MediaSource::from_json(&s.to_json()), Some(s));
        assert!(MediaSource::from_json("").is_none());
        assert!(MediaSource::from_json(r#"{"url":"file:///a","kind":"file"}"#).is_none());
        assert!(MediaSource::new("https://a/b.mp4", StreamKind::File, "").headers().is_empty());
    }

    #[test]
    fn a_plain_source_is_taken_as_it_is() {
        let list = [sniffed("https://cdn/v.mp4", "video/mp4", "https://hoster/e/1")];
        let Capture::Found(s) = capture("https://cdn/v.mp4", "https://hoster/e/2", "https://site/p", &list)
        else {
            panic!()
        };
        // The request the page made knows its Referer best.
        assert_eq!((s.kind, s.referer.as_str()), (StreamKind::File, "https://hoster/e/1"));
        // Without that, the frame the video lives in; without a frame, the page.
        let Capture::Found(s) = capture("https://cdn/v.mp4", "https://hoster/e/2", "https://site/p", &[])
        else {
            panic!()
        };
        assert_eq!(s.referer, "https://hoster/e/2");
        let Capture::Found(s) = capture("https://cdn/v.mp4", "", "https://site/p", &[]) else { panic!() };
        assert_eq!(s.referer, "https://site/p");
        let Capture::Found(s) = capture("https://cdn/index.m3u8", "", "https://site/p", &[]) else {
            panic!()
        };
        assert_eq!(s.kind, StreamKind::Hls);
        assert_eq!(capture("https://cdn/m.mpd", "", "https://site/p", &[]), Capture::Unsupported(WHY_DASH));
    }

    #[test]
    fn blob_streams_use_what_the_network_showed() {
        let seg = sniffed("https://cdn/s1.ts", "video/mp2t", "https://h/e");
        let master = sniffed("https://cdn/master.m3u8", "application/x-mpegurl", "https://h/e");
        let variant = sniffed("https://cdn/720/index.m3u8", "", "https://h/e");
        let mp4 = sniffed("https://cdn/clip.mp4", "video/mp4", "https://h/e");
        let list = [seg.clone(), master.clone(), variant, mp4.clone()];
        // The master playlist, not a variant, not the file.
        let Capture::Found(s) = capture("blob:https://h/1", "https://h/e", "https://site/p", &list) else {
            panic!()
        };
        assert_eq!((s.url.as_str(), s.kind), ("https://cdn/master.m3u8", StreamKind::Hls));
        // Only files: the latest one.
        let list = [mp4, sniffed("https://cdn/clip2.webm", "video/webm", "")];
        let Capture::Found(s) = capture("", "", "https://site/p", &list) else { panic!() };
        assert_eq!(s.url, "https://cdn/clip2.webm");
        assert_eq!(s.referer, "https://site/p");
        // Segments alone are no source; neither is nothing at all.
        assert_eq!(capture("blob:https://h/1", "", "https://site/p", &[seg]), Capture::Unsupported(WHY_BLOB));
        assert_eq!(capture("", "", "https://site/p", &[]), Capture::Nothing);
        let dash = [sniffed("https://cdn/m.mpd", "application/dash+xml", "")];
        assert_eq!(capture("blob:x", "", "https://site/p", &dash), Capture::Unsupported(WHY_DASH));
    }
}
