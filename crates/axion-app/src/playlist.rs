//! Axion Playlist: videos saved for later, optionally downloaded for offline use.
//!
//! This module is the model: what can be added from a page (candidates from the media
//! bridge's report), how a video's source decides whether it can be downloaded, the state of
//! downloads, playback position and the "watched" flag, and the playback order. Storage is
//! `axion-store`'s `playlist` table; the page is `playlist_page`, the glue to the browser
//! `browser/playlist.rs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use axion_compositor::Icon;
use axion_config::{Config, Playlist as PlaylistConfig, PlaylistSort};
use axion_engine::{DownloadInfo, DownloadState, ViewId};
use axion_media::{Capture, Kind, PageMedia, Player, Sniffed, format_time};
use axion_store::{NewPlaylistItem, PlaylistItem, Store, dl};

use axion_i18n::{tr, tr_fmt, tr_mark, tr_n};

use crate::url::display_host;

/// Where a video's bytes come from, which decides if it can be saved for offline use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A plain media file at this address: downloadable.
    Direct(String),
    /// A file on this computer already.
    Local(PathBuf),
    /// A stream (HLS, DASH, a `blob:` the page assembles itself): online only. The text says
    /// why, for people.
    Streaming(&'static str),
}

/// The kind of source a media address is.
pub fn classify(media_url: &str) -> Source {
    let url = media_url.trim();
    if let Some(path) = path_from_file_url(url) {
        return Source::Local(path);
    }
    let scheme_ok = url.starts_with("http://") || url.starts_with("https://");
    if !scheme_ok {
        return Source::Streaming(tr_mark(
            "The page streams this video itself, so it can only be watched online.",
        ));
    }
    let path = url.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    if path.ends_with(".m3u8") || path.ends_with(".m3u") {
        return Source::Streaming(tr_mark("HLS streams (.m3u8) can only be watched online for now."));
    }
    if path.ends_with(".mpd") || path.ends_with(".ism/manifest") {
        return Source::Streaming(tr_mark("DASH streams can only be watched online for now."));
    }
    Source::Direct(url.to_owned())
}

/// What the Add button may offer for one video of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The bridge's id of the player, to find its picture later.
    pub player: String,
    pub title: String,
    pub page_url: String,
    /// The plain file's address (never a `blob:`).
    pub media_url: Option<String>,
    pub source: Source,
    /// A picture the page itself offers (poster, artwork, preview image).
    pub thumb_url: Option<String>,
    pub duration: f64,
    pub position: f64,
    /// Where the video is on the page (CSS px), for taking its picture, and the CSS → device
    /// pixel factor.
    pub rect: (f32, f32, f32, f32),
    pub dpr: f32,
    /// The real media behind the page's player, found by the bridge and the network (what the
    /// Playlist plays instead of the page).
    pub capture: Capture,
}

/// The videos of a page worth saving: every `<video>`, and audio with a known length.
pub fn candidates(
    page_url: &str,
    tab_title: &str,
    media: &PageMedia,
    sniffed: &[Sniffed],
    max_title: usize,
) -> Vec<Candidate> {
    if !(page_url.starts_with("http://")
        || page_url.starts_with("https://")
        || page_url.starts_with("file://"))
    {
        return Vec::new();
    }
    let session = media.session.as_ref();
    let session_title = session.map(|s| s.title.trim()).filter(|t| !t.is_empty());
    let page_title = tab_title.trim();
    let base = session_title
        .or(Some(page_title).filter(|t| !t.is_empty()))
        .map_or_else(|| display_host(page_url), str::to_owned);
    let usable: Vec<&Player> = media
        .players
        .iter()
        .filter(|p| p.kind == Kind::Video || p.duration > 0.0)
        .filter(|p| !p.src.is_empty() || p.duration > 0.0 || p.area() > 0.0)
        .collect();
    let many = usable.len() > 1;
    usable
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let source = if p.src.is_empty() { classify("") } else { classify(&p.src) };
            let media_url = match &source {
                Source::Direct(url) => Some(url.clone()),
                Source::Local(_) => Some(p.src.clone()),
                Source::Streaming(_) => None,
            };
            let artwork = session.map(|s| s.artwork.as_str()).filter(|a| a.starts_with("http"));
            let thumb_url = [p.poster.as_str(), artwork.unwrap_or_default(), media.image.as_str()]
                .into_iter()
                .find(|u| u.starts_with("http"))
                .map(str::to_owned);
            let title = if many { format!("{base} ({})", i + 1) } else { base.clone() };
            Candidate {
                player: p.id.clone(),
                title: truncate_chars(&title, max_title),
                page_url: page_url.to_owned(),
                media_url,
                source,
                thumb_url,
                duration: p.duration,
                position: p.time,
                rect: (p.x, p.y, p.w, p.h),
                dpr: media.dpr,
                capture: axion_media::capture(&p.src, &p.frame, page_url, sniffed),
            }
        })
        .collect()
}

/// `text` cut to `max` characters, with an ellipsis when it was longer.
pub fn truncate_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// What adding did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    New(i64),
    /// Already in the list.
    Existing(i64),
    /// The list holds `playlist.max_items` already.
    Full,
    Failed,
}

/// Saves a candidate; the download (if wanted) is started by the caller.
pub fn add(store: &Store, config: &Config, c: &Candidate, now: i64) -> Added {
    let cfg = &config.playlist;
    match store.playlist_len() {
        Ok(n) if n >= cfg.max_items as usize => {
            // A video that is in the list already is not "new": fall through to find it.
            let existing = find_existing(store, cfg, c);
            return existing.map_or(Added::Full, Added::Existing);
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("reading the Playlist failed: {e}");
            return Added::Failed;
        }
    }
    let (dl_state, dl_path, dl_note) = match &c.source {
        Source::Direct(_) => (dl::NONE, String::new(), String::new()),
        Source::Local(path) => (dl::DONE, path.to_string_lossy().into_owned(), String::new()),
        Source::Streaming(why) => (dl::STREAMING, String::new(), (*why).to_owned()),
    };
    let dl_size = match &c.source {
        Source::Local(path) => std::fs::metadata(path).map_or(0, |m| m.len() as i64),
        _ => 0,
    };
    let new = NewPlaylistItem {
        list: cfg.list_name.clone(),
        title: c.title.clone(),
        page_url: c.page_url.clone(),
        media_url: c.media_url.clone(),
        thumbnail: String::new(),
        duration: c.duration,
        position: if c.position >= f64::from(cfg.min_resume_seconds) { c.position } else { 0.0 },
        dl_state: dl_state.to_owned(),
        dl_path,
        dl_size,
        dl_note,
        source: match &c.capture {
            Capture::Found(source) => source.to_json(),
            _ => String::new(),
        },
    };
    match store.add_playlist_item(&new, now) {
        Ok((id, true)) => Added::New(id),
        Ok((id, false)) => Added::Existing(id),
        Err(e) => {
            tracing::error!("adding to the Playlist failed: {e}");
            Added::Failed
        }
    }
}

fn find_existing(store: &Store, cfg: &PlaylistConfig, c: &Candidate) -> Option<i64> {
    store.playlist_items(&cfg.list_name).ok()?.into_iter().find(|i| same_video(i, c)).map(|i| i.id)
}

/// Whether a stored item is the candidate's video.
pub fn same_video(item: &PlaylistItem, c: &Candidate) -> bool {
    // The downloaded copy of an item is that item.
    if let Source::Local(path) = &c.source
        && item.dl_state == dl::DONE
        && Path::new(&item.dl_path) == path
    {
        return true;
    }
    match (&item.media_url, &c.media_url) {
        (Some(a), Some(b)) => a == b,
        (None, None) => item.page_url == c.page_url,
        _ => false,
    }
}

/// The wording around downloads, in one place: the action is always called "Download"; what a
/// row says about a stream it can't fetch is its state, and the toast on a click says why.
pub const DOWNLOAD_ACTION: &str = tr_mark("Download");
/// A row's state for an item that can only be watched online.
pub const STATE_NOT_DOWNLOADED: &str = tr_mark("Not downloaded");
/// The menu row's secondary text when the Download action can't succeed.
pub const DOWNLOAD_UNAVAILABLE_DETAIL: &str = tr_mark("can’t download");
/// The toast when Download is chosen for an item that can't be fetched.
pub const DOWNLOAD_UNAVAILABLE_TOAST: &str = tr_mark("Can’t be downloaded — it can only be streamed");

/// What a download of an item would be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadPlan {
    /// Fetch this file.
    Start(String),
    /// The file is on disk already.
    Done,
    /// It can't be downloaded; the text says why.
    Unavailable(String),
}

pub fn plan_download(item: &PlaylistItem) -> DownloadPlan {
    if item.dl_state == dl::DONE && Path::new(&item.dl_path).exists() {
        return DownloadPlan::Done;
    }
    let Some(media) = item.media_url.as_deref() else {
        let note = if item.dl_note.is_empty() {
            tr_mark("This video is a stream the page assembles itself, so it can only be watched online.")
        } else {
            item.dl_note.as_str()
        };
        return DownloadPlan::Unavailable(note.to_owned());
    };
    match classify(media) {
        Source::Direct(url) => DownloadPlan::Start(url),
        Source::Local(_) => DownloadPlan::Done,
        Source::Streaming(why) => DownloadPlan::Unavailable(why.to_owned()),
    }
}

/// What an engine download update means for an item: (state, path, size, note).
pub fn download_update(info: &DownloadInfo) -> (&'static str, String, i64, String) {
    let path = info.path.to_string_lossy().into_owned();
    let size = info.total_bytes.max(info.received_bytes).max(0);
    match info.state {
        DownloadState::InProgress | DownloadState::Paused => (dl::DOWNLOADING, path, size, String::new()),
        DownloadState::Complete => (dl::DONE, path, size, String::new()),
        DownloadState::Cancelled => {
            (dl::NONE, String::new(), 0, tr_mark("The download was cancelled.").to_owned())
        }
        DownloadState::Failed => (
            dl::FAILED,
            String::new(),
            0,
            tr_mark("The download failed. The site may not allow it.").to_owned(),
        ),
    }
}

/// The folder downloads go to: `playlist.directory`, else `default_dir` (the account's own
/// `<profile>/playlist`, never the Downloads folder).
pub fn download_directory(config: &Config, default_dir: &Path) -> PathBuf {
    let configured = config.playlist.directory.trim();
    if configured.is_empty() {
        return default_dir.to_path_buf();
    }
    match (configured.strip_prefix("~/"), directories::BaseDirs::new()) {
        (Some(rest), Some(dirs)) => dirs.home_dir().join(rest),
        _ => PathBuf::from(configured),
    }
}

/// A volume control moved on a Playlist video: the Playlist's one shared volume (and mute
/// state) follows. Returns whether it changed (so it is saved).
pub fn remember_volume(config: &mut PlaylistConfig, action: &axion_media::Action) -> bool {
    match *action {
        axion_media::Action::Volume(v) => {
            let v = (v as f32).clamp(0.0, 1.0);
            let changed = (config.volume - v).abs() > f32::EPSILON;
            config.volume = v;
            changed
        }
        axion_media::Action::Mute(m) => {
            let changed = config.muted != m;
            config.muted = m;
            changed
        }
        _ => false,
    }
}

/// The commands that bring a freshly loaded Playlist video to the shared volume and mute state.
pub fn volume_actions(config: &PlaylistConfig) -> [axion_media::Action; 2] {
    [
        axion_media::Action::Volume(f64::from(config.volume.clamp(0.0, 1.0))),
        axion_media::Action::Mute(config.muted),
    ]
}

#[cfg(test)]
mod volume_tests {
    use super::*;
    use axion_media::Action;

    #[test]
    fn controls_set_the_shared_volume() {
        let mut c = PlaylistConfig::default();
        assert!(remember_volume(&mut c, &Action::Volume(0.4)));
        assert!((c.volume - 0.4).abs() < 1e-6);
        // The same value again changes nothing (nothing to save).
        assert!(!remember_volume(&mut c, &Action::Volume(0.4)));
        assert!(remember_volume(&mut c, &Action::Mute(true)));
        assert!(c.muted && (c.volume - 0.4).abs() < 1e-6);
        assert!(!remember_volume(&mut c, &Action::Skip(10.0)));
        // Out-of-range values are clamped.
        remember_volume(&mut c, &Action::Volume(3.0));
        assert_eq!(c.volume, 1.0);
    }

    #[test]
    fn a_new_video_starts_at_the_shared_volume() {
        let c = PlaylistConfig { volume: 0.25, muted: true, ..PlaylistConfig::default() };
        let [volume, mute] = volume_actions(&c);
        assert!(matches!(volume, Action::Volume(v) if (v - 0.25).abs() < 1e-6));
        assert!(matches!(mute, Action::Mute(true)));
    }
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    #[test]
    fn download_directory_defaults_to_the_profile() {
        let mut config = Config::default();
        let default = Path::new("/profiles/a/playlist");
        assert_eq!(download_directory(&config, default), default);
        config.playlist.directory = "  ".into();
        assert_eq!(download_directory(&config, default), default);
        config.playlist.directory = "/mnt/videos".into();
        assert_eq!(download_directory(&config, default), Path::new("/mnt/videos"));
    }
}

/// The position to save and whether the video counts as watched, for a player reporting
/// `time` of `duration` seconds. Finished videos start over next time.
pub fn progress(time: f64, duration: f64, cfg: &PlaylistConfig) -> (f64, bool) {
    if duration <= 0.0 {
        return (time.max(0.0), false);
    }
    let fraction = time / duration;
    let watched = fraction >= f64::from(cfg.watched_fraction);
    let finished = duration - time < 1.0;
    (if finished { 0.0 } else { time.max(0.0) }, watched)
}

/// The sort choices: mode, menu label, icon. The menu and Settings list them in this order.
pub const SORTS: [(PlaylistSort, &str, Icon); 3] = [
    (PlaylistSort::Newest, tr_mark("Newest First"), Icon::ChevronDown),
    (PlaylistSort::Oldest, tr_mark("Oldest First"), Icon::ChevronUp),
    (PlaylistSort::Manual, tr_mark("Custom Order"), Icon::Menu),
];

/// Whether an item counts as finished: watched up to `watched_fraction`, or watched and
/// reset to the start. Finished videos start over, and are not "continue watching".
pub fn is_finished(item: &PlaylistItem, cfg: &PlaylistConfig) -> bool {
    let fraction = if item.duration > 0.0 { item.position / item.duration } else { 0.0 };
    fraction >= f64::from(cfg.watched_fraction)
        || (item.watched && item.position < f64::from(cfg.min_resume_seconds))
}

/// The list in the order the page shows it: `cfg.sort` for everything, except that with
/// `continue_on_top` the most recently played unfinished item (the one to continue watching)
/// goes first. Ties keep the stored order (`items` arrives in the store's manual order).
pub fn order_items(mut items: Vec<PlaylistItem>, cfg: &PlaylistConfig) -> Vec<PlaylistItem> {
    match cfg.sort {
        PlaylistSort::Newest => items.sort_by(|a, b| b.added_at.cmp(&a.added_at).then(b.id.cmp(&a.id))),
        PlaylistSort::Oldest => items.sort_by(|a, b| a.added_at.cmp(&b.added_at).then(a.id.cmp(&b.id))),
        PlaylistSort::Manual => {}
    }
    if cfg.continue_on_top {
        let top = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.played_at > 0 && !is_finished(i, cfg))
            .max_by_key(|(_, i)| i.played_at)
            .map(|(n, _)| n);
        if let Some(n) = top {
            let item = items.remove(n);
            items.insert(0, item);
        }
    }
    items
}

/// Where playing an item starts: where it stopped (a little before), unless that is not
/// worth resuming or the video was finished.
pub fn resume_position(item: &PlaylistItem, cfg: &PlaylistConfig) -> f64 {
    if item.position < f64::from(cfg.min_resume_seconds) || is_finished(item, cfg) {
        return 0.0;
    }
    (item.position - f64::from(cfg.resume_rewind_seconds)).max(0.0)
}

/// "44 items 17h 17min" for the list's header.
pub fn summary(items: &[PlaylistItem]) -> String {
    let count = items.len();
    let words = tr_n("{n} item", "{n} items", count);
    let total: f64 = items.iter().map(|i| i.duration.max(0.0)).sum();
    let minutes = (total / 60.0).round() as u64;
    match (minutes / 60, minutes % 60) {
        (0, 0) => words,
        (0, m) => tr_fmt!("{items} {m}min", items = words, m = m),
        (h, m) => tr_fmt!("{items} {h}h {m}min", items = words, h = h, m = m),
    }
}

/// A duration for a row ("1:05", "1:02:03"); empty while unknown.
pub fn duration_label(seconds: f64) -> String {
    if seconds <= 0.0 { String::new() } else { format_time(seconds) }
}

/// `text` broken into at most `max_lines` lines of about `max_chars` characters at word
/// boundaries; the last line ends with an ellipsis when text was left over. (Labels are one
/// line, so rows lay long titles out with several.)
pub fn wrap_title(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let max_chars = max_chars.max(4);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        // A word longer than a line is cut.
        let mut word = word.to_owned();
        while word.chars().count() > max_chars {
            let head: String = word.chars().take(max_chars).collect();
            word = word.chars().skip(max_chars).collect();
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            lines.push(head);
        }
        let joined = if current.is_empty() { word.clone() } else { format!("{current} {word}") };
        if joined.chars().count() <= max_chars {
            current = joined;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word;
        }
        if words.peek().is_none() {
            lines.push(std::mem::take(&mut current));
        }
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            *last = truncate_chars(&format!("{last} …"), max_chars);
            if !last.ends_with('…') {
                last.push('…');
            }
        }
    }
    lines
}

/// Remembers a media response of a view's traffic: no repeats (range requests ask for the
/// same file again and again), at most `limit` kept (the oldest go).
pub fn remember_sniffed(list: &mut Vec<Sniffed>, new: Sniffed, limit: usize) {
    if list.iter().any(|s| s.url == new.url) {
        return;
    }
    list.push(new);
    if list.len() > limit.max(1) {
        list.remove(0);
    }
}

/// The source of a page being resolved: the first of its videos (biggest first) that
/// yields one; unplayable reasons only count when nothing else was found.
pub fn resolve_capture(media: Option<&PageMedia>, page_url: &str, sniffed: &[Sniffed]) -> Capture {
    let mut videos: Vec<&Player> =
        media.map(|m| m.players.iter().filter(|p| p.kind == Kind::Video).collect()).unwrap_or_default();
    videos.sort_by(|a, b| b.area().total_cmp(&a.area()));
    let mut fallback = Capture::Nothing;
    for p in videos {
        match axion_media::capture(&p.src, &p.frame, page_url, sniffed) {
            found @ Capture::Found(_) => return found,
            unsupported @ Capture::Unsupported(_) => fallback = unsupported,
            Capture::Nothing => {}
        }
    }
    match axion_media::capture("", "", page_url, sniffed) {
        found @ Capture::Found(_) => found,
        _ => fallback,
    }
}

/// The wording for why a video can't be played outside its page (the reasons come from
/// `axion_media` in English).
pub fn unsupported_text(why: &'static str) -> &'static str {
    if why == axion_media::WHY_DASH {
        tr("This video uses a DASH stream, which Axion can only play on its page.")
    } else if why == axion_media::WHY_BLOB {
        tr("This video is assembled by its page and can only be played there.")
    } else {
        why
    }
}

/// What the player area says when the media view reported a failure.
pub fn failure_text(code: &str) -> &'static str {
    match code {
        "hls" => tr("This stream couldn’t be loaded. Its server may refuse playback outside the page."),
        "source" => tr("The video’s address is no longer available."),
        _ => tr("This video can’t be played outside its page."),
    }
}

/// What happens when a video ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    /// Start again from the first after the last.
    All,
    /// The same video again.
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// The index to play after (or before) `current` among `len` rows. `random` picks among the
/// others when shuffling. `None`: the list ends here.
pub fn step_index(
    len: usize,
    current: Option<usize>,
    forward: bool,
    shuffle: bool,
    repeat: Repeat,
    random: u64,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current.filter(|&c| c < len) else { return Some(0) };
    if repeat == Repeat::One && forward {
        return Some(current);
    }
    if shuffle && len > 1 {
        let pick = (random % (len as u64 - 1)) as usize;
        return Some(if pick >= current { pick + 1 } else { pick });
    }
    if forward {
        if current + 1 < len {
            Some(current + 1)
        } else if repeat == Repeat::All {
            Some(0)
        } else {
            None
        }
    } else {
        // Before the first: the first again (it starts over).
        Some(current.saturating_sub(1))
    }
}

/// When the sleep timer pauses playback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sleep {
    #[default]
    Off,
    At(Instant),
    EndOfVideo,
}

/// A sleep timer's choice for the menu (index into `playlist.sleep_timer_minutes`).
pub fn sleep_label(minutes: u32) -> String {
    match minutes {
        m if m >= 60 && m % 60 == 0 => tr_n("{n} hour", "{n} hours", (m / 60) as usize),
        m => tr_n("{n} minute", "{n} minutes", m as usize),
    }
}

/// A tab opened for a saved video's page, waiting for its player.
#[derive(Clone, Copy, Debug)]
pub struct Pending {
    pub position: f64,
}

/// What a row's menu, the lists menu, the speed menu, the sleep timer and the page menu's
/// Playlist entries do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Play(i64),
    Download(i64),
    RemoveDownload(i64),
    OpenPage(i64),
    Reveal(i64),
    Remove(i64),
    /// Speed `playback_speeds[i]`.
    Speed(usize),
    SleepOff,
    SleepEnd,
    /// The timer `sleep_timer_minutes[i]`.
    SleepAfter(usize),
    /// Add the page's video (the page menu).
    Add,
    /// Open the Playlist page (the page menu).
    Show,
    /// Order the list this way (the sort menu).
    Sort(PlaylistSort),
}

/// The video of the Playlist page: its view (not a tab) and what the bridge reports.
pub struct Playback {
    pub item: i64,
    pub view: ViewId,
    pub media: Option<PageMedia>,
    /// Seek here once the video is loaded.
    pub seek: Option<f64>,
    /// Start playing once loaded (local files; pages play on their own).
    pub autoplay: bool,
    pub opened: Instant,
    /// The thumbnail was taken from the picture (or didn't need to be).
    pub thumb_done: bool,
    /// A saved copy plays (a media document, not the media view's page).
    pub local: bool,
    /// The token `axion://media/` serves the source under.
    pub token: Option<String>,
    /// The source was found just now (a saved one may have expired).
    pub fresh: bool,
    /// The seek, the speed and the autoplay were sent.
    pub started: bool,
    /// When the position was last saved.
    pub saved_at: Instant,
    pub ended_handled: bool,
    /// The player was playing at the last look (a change to paused saves the position).
    pub was_playing: bool,
}

impl Playback {
    pub fn player(&self) -> Option<&Player> {
        self.media.as_ref()?.primary()
    }
}

/// An item whose video source is being looked for: its page loads hidden (and muted) just
/// long enough for the bridge and the network to show where the video comes from.
pub struct Resolving {
    pub item: i64,
    /// The hidden view.
    pub view: ViewId,
    pub started: Instant,
    pub media: Option<PageMedia>,
    /// When the page's players last looked unplayable (`Capture::Unsupported`), and why.
    pub unsupported: Option<(Instant, &'static str)>,
    /// When the page's player was last clicked (to start site players), and how often.
    pub clicked_at: Option<Instant>,
}

/// Why an item couldn't be played in the Playlist's player.
pub struct Failure {
    pub item: i64,
    pub message: String,
}

/// A tab showing a saved video, whose position is saved as it plays.
pub struct Tracked {
    pub item: i64,
    pub saved_at: Instant,
    pub last_time: f64,
}

/// A thumbnail being fetched from the network (the address Chromium is asked for).
pub struct ThumbJob {
    pub item: i64,
    pub view: ViewId,
    pub url: String,
    pub started: Instant,
}

/// Everything the Playlist keeps in memory.
pub struct Playlist {
    /// The list's rows, in order.
    pub items: Vec<PlaylistItem>,
    /// Rows changed: the page redraws them.
    pub dirty: bool,
    /// Downloads we started, by the address they were started with.
    pub downloads: HashMap<String, i64>,
    /// Items downloading now, with their progress (0–1; `None` while unknown).
    pub progress: HashMap<i64, Option<f32>>,
    pub thumb_jobs: Vec<ThumbJob>,
    pub tracked: HashMap<ViewId, Tracked>,
    pub playback: Option<Playback>,
    /// A source being looked for in the background.
    pub resolving: Option<Resolving>,
    /// The last item that couldn't play, shown in the player area with *Open page*.
    pub failure: Option<Failure>,
    /// Axion's fullscreen player shows the playback.
    pub fullscreen: bool,
    /// Media-looking responses of each view's network traffic, oldest first.
    pub sniffed: HashMap<ViewId, Vec<Sniffed>>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub sleep: Sleep,
    /// The speed of the Playlist page's player.
    pub rate: f64,
    /// Tabs opened for a saved video, waiting for their player.
    pub pending: HashMap<ViewId, Pending>,
    pub editing: bool,
    pub selected: HashSet<i64>,
    rng: u64,
    /// `<profile>/playlist/thumbs`.
    pub thumb_dir: PathBuf,
    /// `<profile>/playlist`: where downloads go unless `playlist.directory` says otherwise.
    pub download_dir: PathBuf,
}

impl Playlist {
    pub fn new(profile: &Path) -> Self {
        Self {
            items: Vec::new(),
            dirty: true,
            downloads: HashMap::new(),
            progress: HashMap::new(),
            thumb_jobs: Vec::new(),
            tracked: HashMap::new(),
            playback: None,
            resolving: None,
            failure: None,
            fullscreen: false,
            sniffed: HashMap::new(),
            shuffle: false,
            repeat: Repeat::Off,
            sleep: Sleep::Off,
            rate: 1.0,
            pending: HashMap::new(),
            editing: false,
            selected: HashSet::new(),
            rng: 0x9E37_79B9_7F4A_7C15
                ^ std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |d| d.as_nanos() as u64),
            thumb_dir: profile.join("playlist").join("thumbs"),
            download_dir: profile.join("playlist"),
        }
    }

    /// Reads the list from the store.
    pub fn reload(&mut self, store: Option<&Store>, config: &Config) {
        let Some(store) = store else { return };
        match store.playlist_items(&config.playlist.list_name) {
            Ok(items) => {
                self.items = order_items(items, &config.playlist);
                self.dirty = true;
            }
            Err(e) => tracing::error!("reading the Playlist failed: {e}"),
        }
        let ids: HashSet<i64> = self.items.iter().map(|i| i.id).collect();
        self.selected.retain(|id| ids.contains(id));
    }

    pub fn item(&self, id: i64) -> Option<&PlaylistItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The saved item a page address belongs to: the page of a streamed video, or the file of
    /// a downloaded one.
    pub fn item_for_url(&self, url: &str) -> Option<&PlaylistItem> {
        self.items.iter().find(|i| {
            i.page_url == url
                || i.media_url.as_deref() == Some(url)
                || (i.dl_state == dl::DONE
                    && !i.dl_path.is_empty()
                    && path_from_file_url(url).is_some_and(|p| p == Path::new(&i.dl_path)))
        })
    }

    /// The item for a candidate, if it is in the list.
    pub fn contains(&self, c: &Candidate) -> Option<i64> {
        self.items.iter().find(|i| same_video(i, c)).map(|i| i.id)
    }

    /// A pseudo-random number (xorshift; for shuffling, no quality needed).
    pub fn random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// Items matching the search, in list order (the words must all appear in title or site).
    pub fn matching(&self, query: &str) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let hay = format!("{} {}", item.title, display_host(&item.page_url)).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

/// `file:///home/me/My%20Video.mp4` → the path.
pub fn path_from_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// A path as a `file://` address.
pub fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for &b in path.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axion_media::{MediaSource, SessionInfo};

    fn item(id: i64, media: Option<&str>, page: &str) -> PlaylistItem {
        PlaylistItem {
            id,
            list: "Playlist".into(),
            sort: 0,
            title: format!("Video {id}"),
            page_url: page.into(),
            media_url: media.map(str::to_owned),
            thumbnail: String::new(),
            duration: 600.0,
            added_at: 0,
            position: 0.0,
            watched: false,
            dl_state: dl::NONE.into(),
            dl_path: String::new(),
            dl_size: 0,
            dl_note: String::new(),
            source: String::new(),
            played_at: 0,
        }
    }

    #[test]
    fn sources_are_classified() {
        assert_eq!(
            classify("https://x.org/a/v.mp4?token=1"),
            Source::Direct("https://x.org/a/v.mp4?token=1".into())
        );
        assert!(matches!(classify("https://x.org/live/master.M3U8?x=1"), Source::Streaming(_)));
        assert!(matches!(classify("https://x.org/m.mpd"), Source::Streaming(_)));
        assert!(matches!(classify("blob:https://x.org/uuid"), Source::Streaming(_)));
        assert!(matches!(classify(""), Source::Streaming(_)));
        assert_eq!(classify("file:///tmp/a%20b.mp4"), Source::Local(PathBuf::from("/tmp/a b.mp4")));
    }

    #[test]
    fn file_urls_round_trip() {
        let p = PathBuf::from("/home/me/Videos/Tom & Jerry (1).mp4");
        assert_eq!(path_from_file_url(&file_url(&p)), Some(p));
        assert_eq!(file_url(Path::new("/a b")), "file:///a%20b");
        assert!(path_from_file_url("https://x/").is_none());
    }

    #[test]
    fn candidates_come_from_the_bridge_report() {
        let video = Player {
            id: "a:1".into(),
            kind: Kind::Video,
            w: 640.0,
            h: 360.0,
            duration: 120.0,
            time: 30.0,
            src: "https://cdn.x/v.mp4".into(),
            poster: "https://cdn.x/p.jpg".into(),
            ..Default::default()
        };
        let stream = Player { id: "a:2".into(), src: "blob:https://x/1".into(), ..video.clone() };
        let media = PageMedia {
            players: vec![video.clone()],
            image: "https://x/og.png".into(),
            session: Some(SessionInfo { title: "  Song ".into(), ..Default::default() }),
            ..Default::default()
        };
        let list = candidates("https://x/watch", "Tab title", &media, &[], 200);
        assert_eq!(list.len(), 1);
        let c = &list[0];
        assert_eq!(c.title, "Song");
        assert_eq!(c.media_url.as_deref(), Some("https://cdn.x/v.mp4"));
        assert_eq!(c.thumb_url.as_deref(), Some("https://cdn.x/p.jpg"));
        assert_eq!(c.position, 30.0);
        // Streams have no file; two videos get numbered titles; internal pages offer nothing.
        let media = PageMedia { players: vec![video, stream], ..Default::default() };
        let list = candidates("https://x/watch", "Tab title", &media, &[], 200);
        assert_eq!(list[0].title, "Tab title (1)");
        assert_eq!(list[1].media_url, None);
        assert!(matches!(list[1].source, Source::Streaming(_)));
        assert!(candidates("axion://home/", "", &media, &[], 200).is_empty());
        assert_eq!(truncate_chars("abcdefgh", 5), "abcd…");
    }

    #[test]
    fn adding_respects_duplicates_and_the_limit() {
        let store = Store::in_memory().unwrap();
        let mut config = Config::default();
        config.playlist.max_items = 2;
        let c = |url: &str| Candidate {
            player: "a:1".into(),
            title: "T".into(),
            page_url: "https://x/".into(),
            media_url: Some(url.into()),
            source: classify(url),
            thumb_url: None,
            duration: 10.0,
            position: 3.0,
            rect: (0.0, 0.0, 1.0, 1.0),
            dpr: 1.0,
            capture: Capture::Nothing,
        };
        let Added::New(a) = add(&store, &config, &c("https://x/1.mp4"), 1) else { panic!() };
        // A captured source is stored with the item.
        let mut with_source = c("https://x/0.mp4");
        with_source.capture =
            Capture::Found(MediaSource::new("https://x/0.mp4", axion_media::StreamKind::File, "https://x/"));
        let Added::New(z) = add(&store, &config, &with_source, 0) else { panic!() };
        let stored = store.playlist_item(z).unwrap().unwrap();
        assert_eq!(MediaSource::from_json(&stored.source).unwrap().referer, "https://x/");
        store.remove_playlist_item(z).unwrap();
        assert_eq!(add(&store, &config, &c("https://x/1.mp4"), 2), Added::Existing(a));
        assert!(matches!(add(&store, &config, &c("https://x/2.mp4"), 3), Added::New(_)));
        assert_eq!(add(&store, &config, &c("https://x/3.mp4"), 4), Added::Full);
        // Already there when the list is full: not "full".
        assert_eq!(add(&store, &config, &c("https://x/1.mp4"), 5), Added::Existing(a));
        // 3 s is not worth resuming from.
        assert_eq!(store.playlist_item(a).unwrap().unwrap().position, 0.0);
        // A stream is marked streaming-only with a reason, never silently downloadable.
        let mut stream = c("blob:https://x/9");
        stream.media_url = None;
        stream.source = classify("blob:https://x/9");
        config.playlist.max_items = 10;
        let Added::New(s) = add(&store, &config, &stream, 6) else { panic!() };
        let stored = store.playlist_item(s).unwrap().unwrap();
        assert_eq!(stored.dl_state, dl::STREAMING);
        assert!(!stored.dl_note.is_empty());
        assert!(matches!(plan_download(&stored), DownloadPlan::Unavailable(_)));
    }

    #[test]
    fn download_plans_and_updates() {
        let direct = item(1, Some("https://x/v.mp4"), "https://x/");
        assert_eq!(plan_download(&direct), DownloadPlan::Start("https://x/v.mp4".into()));
        let hls = item(2, Some("https://x/v.m3u8"), "https://x/");
        assert!(matches!(plan_download(&hls), DownloadPlan::Unavailable(_)));
        let mut done = item(3, Some("https://x/v.mp4"), "https://x/");
        done.dl_state = dl::DONE.into();
        done.dl_path = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        assert_eq!(plan_download(&done), DownloadPlan::Done);
        // A finished download whose file was deleted can be fetched again.
        done.dl_path = "/nonexistent/v.mp4".into();
        assert!(matches!(plan_download(&done), DownloadPlan::Start(_)));

        let info = |state, received, total| DownloadInfo {
            id: 1,
            url: "https://x/v.mp4".into(),
            original_url: "https://x/v.mp4".into(),
            path: PathBuf::from("/tmp/v.mp4"),
            file_name: "v.mp4".into(),
            mime_type: String::new(),
            received_bytes: received,
            total_bytes: total,
            speed: 0,
            state,
        };
        assert_eq!(download_update(&info(DownloadState::InProgress, 5, 10)).0, dl::DOWNLOADING);
        let (state, path, size, _) = download_update(&info(DownloadState::Complete, 10, 10));
        assert_eq!((state, path.as_str(), size), (dl::DONE, "/tmp/v.mp4", 10));
        let (state, _, _, note) = download_update(&info(DownloadState::Failed, 0, -1));
        assert_eq!(state, dl::FAILED);
        assert!(!note.is_empty());
        assert_eq!(download_update(&info(DownloadState::Cancelled, 0, -1)).0, dl::NONE);
    }

    #[test]
    fn progress_resume_and_watched() {
        let cfg = PlaylistConfig::default();
        assert_eq!(progress(30.0, 100.0, &cfg), (30.0, false));
        assert_eq!(progress(91.0, 100.0, &cfg), (91.0, true));
        // Finished: watched, and next time from the start.
        assert_eq!(progress(99.5, 100.0, &cfg), (0.0, true));
        assert_eq!(progress(12.0, 0.0, &cfg), (12.0, false));
        let mut i = item(1, None, "https://x/");
        i.position = 3.0;
        assert_eq!(resume_position(&i, &cfg), 0.0);
        i.position = 60.0;
        assert_eq!(resume_position(&i, &cfg), 58.0);
        // Past the watched fraction counts as finished: start over.
        i.position = 95.0 * 6.0;
        assert_eq!(resume_position(&i, &cfg), 0.0);
    }

    fn played(id: i64, added: i64, played_at: i64, position: f64, watched: bool) -> PlaylistItem {
        let mut i = item(id, None, "https://x/");
        i.added_at = added;
        i.played_at = played_at;
        i.position = position;
        i.watched = watched;
        i
    }

    fn ids(items: &[PlaylistItem]) -> Vec<i64> {
        items.iter().map(|i| i.id).collect()
    }

    #[test]
    fn sorting_newest_oldest_and_manual() {
        let mut cfg = PlaylistConfig { continue_on_top: false, ..Default::default() };
        // Stored (manual) order: 3, 1, 2.
        let items =
            vec![played(3, 30, 0, 0.0, false), played(1, 10, 0, 0.0, false), played(2, 20, 0, 0.0, false)];
        assert_eq!(ids(&order_items(items.clone(), &cfg)), [3, 2, 1]);
        cfg.sort = PlaylistSort::Oldest;
        assert_eq!(ids(&order_items(items.clone(), &cfg)), [1, 2, 3]);
        cfg.sort = PlaylistSort::Manual;
        assert_eq!(ids(&order_items(items, &cfg)), [3, 1, 2]);
    }

    #[test]
    fn continue_watching_goes_on_top() {
        let mut cfg = PlaylistConfig::default();
        let items = vec![
            played(4, 40, 0, 0.0, false),
            played(3, 30, 500, 0.0, true), // finished, played last: not continued
            played(2, 20, 300, 120.0, false), // in progress
            played(1, 10, 100, 60.0, false), // in progress, played earlier
        ];
        // The most recently played unfinished item is first; the rest follow the sort.
        assert_eq!(ids(&order_items(items.clone(), &cfg)), [2, 4, 3, 1]);
        cfg.sort = PlaylistSort::Oldest;
        assert_eq!(ids(&order_items(items.clone(), &cfg)), [2, 1, 3, 4]);
        // Off: only the sort.
        cfg.continue_on_top = false;
        assert_eq!(ids(&order_items(items.clone(), &cfg)), [1, 2, 3, 4]);
        // Nothing unfinished: nothing is pinned.
        cfg.continue_on_top = true;
        let done = vec![played(2, 20, 300, 0.0, true), played(1, 10, 100, 0.0, true)];
        assert_eq!(ids(&order_items(done, &cfg)), [1, 2]);
        // A video watched to the fraction (not reset yet) counts as finished too.
        assert!(is_finished(&played(1, 1, 5, 560.0, false), &cfg));
        assert!(!is_finished(&played(1, 1, 5, 300.0, false), &cfg));
    }

    #[test]
    fn sniffed_responses_are_kept_once_and_limited() {
        let s = |u: &str| Sniffed { url: u.into(), mime: String::new(), referer: String::new() };
        let mut list = Vec::new();
        for u in ["a", "b", "a", "c", "d"] {
            remember_sniffed(&mut list, s(u), 3);
        }
        let urls: Vec<&str> = list.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(urls, ["b", "c", "d"]);
    }

    #[test]
    fn resolving_prefers_a_players_own_source() {
        let video = |src: &str, w: f32| Player {
            id: "a:1".into(),
            kind: Kind::Video,
            w,
            h: w,
            src: src.into(),
            frame: "https://hoster/e/1".into(),
            ..Default::default()
        };
        let page = "https://site/p";
        let seen =
            [Sniffed { url: "https://cdn/m.m3u8".into(), mime: String::new(), referer: String::new() }];
        // A blob player takes the manifest the network showed, with the frame's Referer.
        let media = PageMedia { players: vec![video("blob:https://hoster/1", 10.0)], ..Default::default() };
        let Capture::Found(found) = resolve_capture(Some(&media), page, &seen) else { panic!() };
        assert_eq!(found.url, "https://cdn/m.m3u8");
        // A bigger player with a plain file wins over a small blob one.
        let media = PageMedia {
            players: vec![video("blob:https://hoster/1", 10.0), video("https://cdn/v.mp4", 100.0)],
            ..Default::default()
        };
        let Capture::Found(found) = resolve_capture(Some(&media), page, &[]) else { panic!() };
        assert_eq!((found.url.as_str(), found.referer.as_str()), ("https://cdn/v.mp4", "https://hoster/e/1"));
        // Only a blob and no network traffic: unplayable, with the reason; nothing at all: not yet.
        let media = PageMedia { players: vec![video("blob:https://hoster/1", 10.0)], ..Default::default() };
        assert_eq!(resolve_capture(Some(&media), page, &[]), Capture::Unsupported(axion_media::WHY_BLOB));
        assert_eq!(resolve_capture(None, page, &[]), Capture::Nothing);
        assert!(failure_text("hls").contains("stream"));
        assert!(failure_text("whatever").contains("outside its page"));
    }

    #[test]
    fn header_summary() {
        let mut items: Vec<PlaylistItem> = (0..44).map(|i| item(i, None, "https://x/")).collect();
        for i in &mut items {
            i.duration = 1410.0;
        }
        items[0].duration = 1410.0 + 2.0 * 60.0;
        assert_eq!(summary(&items), "44 items 17h 16min");
        assert_eq!(summary(&items[..1]), "1 item 26min");
        assert_eq!(summary(&[]), "0 items");
    }

    #[test]
    fn titles_wrap_to_three_lines() {
        let lines = wrap_title("The quick brown fox jumps over the lazy dog again and again", 16, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines[2].ends_with('…'), "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 16));
        assert_eq!(wrap_title("Short", 16, 3), ["Short"]);
        let long = wrap_title("Supercalifragilisticexpialidocious", 10, 2);
        assert_eq!(long.len(), 2);
        assert!(wrap_title("", 10, 3).is_empty());
    }

    #[test]
    fn playback_order() {
        use Repeat::*;
        assert_eq!(step_index(3, Some(0), true, false, Off, 0), Some(1));
        assert_eq!(step_index(3, Some(2), true, false, Off, 0), None);
        assert_eq!(step_index(3, Some(2), true, false, All, 0), Some(0));
        assert_eq!(step_index(3, Some(1), true, false, One, 0), Some(1));
        assert_eq!(step_index(3, Some(0), false, false, Off, 0), Some(0));
        assert_eq!(step_index(3, Some(2), false, false, Off, 0), Some(1));
        assert_eq!(step_index(3, None, true, false, Off, 0), Some(0));
        assert_eq!(step_index(0, None, true, false, Off, 0), None);
        // Shuffle never repeats the current one.
        for r in 0..50 {
            assert_ne!(step_index(5, Some(2), true, true, Off, r), Some(2));
        }
        assert_eq!(Off.next().next().next(), Off);
    }

    #[test]
    fn urls_find_their_items_and_search_matches() {
        let mut p = Playlist::new(Path::new("/tmp/none"));
        let mut a = item(1, Some("https://x/a.mp4"), "https://x/page");
        a.title = "Rust Conference Talk".into();
        a.dl_state = dl::DONE.into();
        a.dl_path = "/tmp/a b.mp4".into();
        let b = item(2, None, "https://y/stream");
        p.items = vec![a, b];
        assert_eq!(p.item_for_url("https://x/page").map(|i| i.id), Some(1));
        assert_eq!(p.item_for_url("file:///tmp/a%20b.mp4").map(|i| i.id), Some(1));
        assert_eq!(p.item_for_url("https://y/stream").map(|i| i.id), Some(2));
        assert!(p.item_for_url("https://z/").is_none());
        assert_eq!(p.matching("rust talk"), [0]);
        assert_eq!(p.matching("y"), [1]);
        assert_eq!(p.matching(""), [0, 1]);
        // A tab playing the downloaded copy shows the item as saved.
        let copy = Candidate {
            player: "a:1".into(),
            title: "T".into(),
            page_url: "file:///tmp/a%20b.mp4".into(),
            media_url: Some("file:///tmp/a%20b.mp4".into()),
            source: classify("file:///tmp/a%20b.mp4"),
            thumb_url: None,
            duration: 1.0,
            position: 0.0,
            rect: (0.0, 0.0, 1.0, 1.0),
            dpr: 1.0,
            capture: Capture::Nothing,
        };
        assert_eq!(p.contains(&copy), Some(1));
        assert_ne!(p.random(), p.random());
        assert_eq!(sleep_label(15), "15 minutes");
        assert_eq!(sleep_label(60), "1 hour");
        assert_eq!(sleep_label(120), "2 hours");
    }
}
