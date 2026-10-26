//! Downloads (Safari's Downloads list): running downloads from the engine, finished ones from
//! the store, shown in the sidebar panel grouped by day; the toolbar's progress indicator;
//! opening files and their folder; the download folder; and clearing browsing data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axion_compositor::Icon;
use axion_config::{ClearRange, Config, RemoveDownloads};
use axion_engine::{ClearData, DownloadAction, DownloadInfo, DownloadOptions, DownloadState, Engine};
use axion_store::{Store, StoredDownload};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedValue, Value};

use axion_i18n::{tr, tr_fmt};

use crate::chrome::PanelItem;
use crate::history::{day_label, local_date};
use crate::url::display_host;

const DAY_SECONDS: i64 = 24 * 60 * 60;
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
/// Chromium's HTTP cache ("cached images and files": what clearing the cache removes).
const CACHE_DIRS: [&str; 2] = ["Default/Cache", "Cache"];
/// Extensions of files that are "safe" to open after downloading (Safari: pictures, movies,
/// sounds, PDF and text). Not executables, scripts, archives, HTML or SVG.
const SAFE_EXTENSIONS: [&str; 22] = [
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "heic", "mp4", "webm", "mkv", "mov", "m4v", "mp3",
    "m4a", "ogg", "opus", "flac", "wav", "pdf", "txt", "md",
];

/// A row of the Downloads panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// A running (or paused) download, by engine id.
    Running(u32),
    /// A finished one, by store id.
    Stored(i64),
}

/// A row's trailing buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowButton {
    Pause,
    Resume,
    Cancel,
    ShowInFolder,
}

impl RowButton {
    fn icon(self) -> Icon {
        match self {
            RowButton::Pause => Icon::Pause,
            RowButton::Resume => Icon::Play,
            RowButton::Cancel => Icon::Stop,
            RowButton::ShowInFolder => Icon::Folder,
        }
    }
}

/// Results of background work (folder chooser, cache measurement), picked up by `poll`.
#[derive(Default)]
struct Jobs {
    cache_size: Option<u64>,
    folder: Option<PathBuf>,
    playlist_folder: Option<PathBuf>,
}

/// What background work finished.
pub enum Event {
    CacheSize(u64),
    FolderChosen(PathBuf),
    /// The folder chosen for the Playlist's downloads.
    PlaylistFolderChosen(PathBuf),
}

/// Where download rows are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    /// The sidebar's Downloads panel (everything, by day).
    Panel,
    /// The toolbar button's popover (the latest few).
    Popover,
}

pub struct Downloads {
    /// Downloads that haven't finished, in start order.
    running: Vec<DownloadInfo>,
    /// A download started in this session (the toolbar button shows from then on).
    any_this_session: bool,
    /// Each list's rows and their buttons, by row index.
    rows: [Vec<(Row, Vec<RowButton>)>; 2],
    /// Finished downloads of the shown lists, by store id.
    stored: HashMap<i64, StoredDownload>,
    jobs: Arc<Mutex<Jobs>>,
    profile: PathBuf,
}

impl Downloads {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            running: Vec::new(),
            any_this_session: false,
            rows: [Vec::new(), Vec::new()],
            stored: HashMap::new(),
            jobs: Arc::default(),
            profile,
        }
    }

    /// Where the engine saves downloads.
    pub fn options(config: &Config) -> DownloadOptions {
        DownloadOptions { directory: download_dir(config), ask: config.downloads.ask_where_to_save }
    }

    /// A download changed. Finished ones go to the store (and open, if they're safe and the
    /// setting says so).
    /// `open_when_done` is false for downloads someone else keeps (the Playlist's).
    pub fn update(
        &mut self,
        mut info: DownloadInfo,
        store: Option<&Store>,
        config: &Config,
        now: i64,
        open_when_done: bool,
    ) {
        // The first update comes before the name is decided.
        if info.file_name.is_empty() {
            let path = info.url.split(['?', '#']).next().unwrap_or_default();
            info.file_name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("download").to_owned();
        }
        let known = self.running.iter().position(|d| d.id == info.id);
        if !info.state.is_finished() {
            match known {
                Some(i) => self.running[i] = info,
                None => {
                    tracing::info!(file = %info.file_name, "download started");
                    self.any_this_session = true;
                    self.running.push(info);
                }
            }
            return;
        }
        // An item CEF finishes before reporting it (e.g. a dialog cancelled) still counts.
        if let Some(i) = known {
            self.running.remove(i);
        }
        tracing::info!(file = %info.file_name, state = ?info.state, "download finished");
        let state = match info.state {
            DownloadState::Complete => "complete",
            DownloadState::Cancelled => "cancelled",
            _ => "failed",
        };
        // The save dialog was cancelled: nothing to list.
        if info.state == DownloadState::Cancelled && info.path.as_os_str().is_empty() {
            return;
        }
        if let Some(store) = store {
            let record = StoredDownload {
                id: 0,
                url: info.url.clone(),
                path: info.path.to_string_lossy().into_owned(),
                file_name: info.file_name.clone(),
                mime_type: info.mime_type.clone(),
                size: info.total_bytes.max(info.received_bytes),
                state: state.into(),
                finished: now,
            };
            if let Err(e) = store.add_download(&record) {
                tracing::error!("saving a download failed: {e}");
            }
        }
        if open_when_done
            && info.state == DownloadState::Complete
            && config.downloads.open_safe_files
            && is_safe(&info.path)
        {
            open_path(&info.path);
        }
    }

    /// The toolbar button: shown once something was downloaded in this session; its bar
    /// shows the running downloads' combined progress (`None`: nothing running, or sizes
    /// unknown).
    pub fn indicator(&self) -> (bool, Option<f32>) {
        if self.running.is_empty() {
            return (self.any_this_session, None);
        }
        let known: Vec<&DownloadInfo> = self.running.iter().filter(|d| d.total_bytes > 0).collect();
        if known.is_empty() {
            return (true, None);
        }
        let received: i64 = known.iter().map(|d| d.received_bytes).sum();
        let total: i64 = known.iter().map(|d| d.total_bytes).sum();
        (true, Some(received as f32 / total.max(1) as f32))
    }

    /// The rows of `list`: running downloads first, then finished ones, newest first, each
    /// day under its heading; at most `limit`.
    pub fn items(
        &mut self,
        list: List,
        store: Option<&Store>,
        config: &Config,
        now: i64,
        limit: usize,
    ) -> Vec<PanelItem> {
        let tz = TimeZone::system();
        let today = local_date(now, &tz);
        let stored_limit =
            (config.downloads.list_limit as usize).min(limit.saturating_sub(self.running.len()));
        let stored = store.and_then(|s| s.downloads(stored_limit).ok()).unwrap_or_default();
        let rows = &mut self.rows[list as usize];
        rows.clear();
        if list == List::Panel {
            self.stored.clear();
        }
        let mut items = Vec::new();
        let mut current: Option<Option<Date>> = None;
        let mut section = |day: Option<Date>| {
            let heading = (current != Some(day))
                .then(|| day.map_or_else(|| tr("Earlier").to_owned(), |d| day_label(d, today)));
            current = Some(day);
            heading
        };
        for d in self.running.iter().rev().take(limit) {
            let buttons = if d.state == DownloadState::Paused {
                vec![RowButton::Resume, RowButton::Cancel]
            } else {
                vec![RowButton::Pause, RowButton::Cancel]
            };
            items.push(PanelItem {
                title: d.file_name.clone(),
                url: d.url.clone(),
                detail: running_detail(d),
                section: section(today),
                deletable: false,
                icon: Some(Icon::Download),
                progress: (d.total_bytes > 0).then(|| d.received_bytes as f32 / d.total_bytes as f32),
                buttons: buttons.iter().map(|b| b.icon()).collect(),
            });
            rows.push((Row::Running(d.id), buttons));
        }
        for d in stored {
            let path = Path::new(&d.path);
            let exists = path.exists();
            let buttons =
                if d.state == "complete" && exists { vec![RowButton::ShowInFolder] } else { Vec::new() };
            let host = display_host(&d.url);
            let detail = match d.state.as_str() {
                "complete" if exists => format!("{} · {host}", human_size(d.size)),
                "complete" => tr_fmt!("Moved or deleted · {host}", host = host),
                "cancelled" => tr_fmt!("Cancelled · {host}", host = host),
                _ => tr_fmt!("Failed · {host}", host = host),
            };
            items.push(PanelItem {
                title: d.file_name.clone(),
                url: d.url.clone(),
                detail,
                section: section(local_date(d.finished, &tz)),
                deletable: true,
                icon: Some(Icon::File),
                progress: None,
                buttons: buttons.iter().map(|b| b.icon()).collect(),
            });
            rows.push((Row::Stored(d.id), buttons));
            self.stored.insert(d.id, d);
        }
        items
    }

    /// Whether the list has finished entries ("Clear List" only shows then).
    pub fn has_finished(&self) -> bool {
        self.rows[List::Panel as usize].iter().any(|(row, _)| matches!(row, Row::Stored(_)))
    }

    /// A click on a row: opens a finished file.
    pub fn open_row(&self, list: List, index: usize) {
        if let Some((Row::Stored(id), _)) = self.rows[list as usize].get(index)
            && let Some(d) = self.stored.get(id)
            && d.state == "complete"
        {
            let path = Path::new(&d.path);
            if path.exists() {
                open_path(path);
            }
        }
    }

    /// A row's `k`th button.
    pub fn row_button(&self, list: List, index: usize, k: usize, engine: &mut impl Engine) {
        let Some((row, buttons)) = self.rows[list as usize].get(index) else { return };
        let Some(&button) = buttons.get(k) else { return };
        match (*row, button) {
            (Row::Running(id), RowButton::Pause) => engine.download_action(id, DownloadAction::Pause),
            (Row::Running(id), RowButton::Resume) => engine.download_action(id, DownloadAction::Resume),
            (Row::Running(id), RowButton::Cancel) => engine.download_action(id, DownloadAction::Cancel),
            (Row::Stored(id), RowButton::ShowInFolder) => {
                if let Some(d) = self.stored.get(&id) {
                    show_in_folder(Path::new(&d.path));
                }
            }
            _ => {}
        }
    }

    /// A row's ×: removes a finished download from the list (the file stays).
    pub fn remove_row(&self, index: usize, store: Option<&Store>) {
        if let (Some((Row::Stored(id), _)), Some(store)) = (self.rows[List::Panel as usize].get(index), store)
            && let Err(e) = store.remove_download(*id)
        {
            tracing::error!("removing a download from the list failed: {e}");
        }
    }

    /// "Clear List": every finished download (running ones stay).
    pub fn clear_list(store: Option<&Store>) {
        if let Some(store) = store
            && let Err(e) = store.clear_downloads_since(0)
        {
            tracing::error!("clearing the downloads list failed: {e}");
        }
    }

    /// Drops list entries older than a day when the setting says so. Call now and then.
    pub fn prune(store: Option<&Store>, config: &Config, now: i64) {
        if config.downloads.remove_list_items == RemoveDownloads::AfterOneDay
            && let Some(store) = store
            && let Err(e) = store.clear_downloads_before(now - DAY_SECONDS)
        {
            tracing::error!("pruning the downloads list failed: {e}");
        }
    }

    /// Axion is quitting (or starting): what the settings remove then.
    pub fn on_quit(store: Option<&Store>, config: &Config, engine: &mut impl Engine) {
        if config.downloads.remove_list_items == RemoveDownloads::OnQuit {
            Self::clear_list(store);
        }
        if config.data.clear_cookies_on_quit {
            tracing::info!("clearing cookies and site data (clear on quit)");
            engine.clear_data(&ClearData { all_site_data: true, ..ClearData::default() });
        }
    }

    /// Measures the cache on a background thread; the size arrives as [`Event::CacheSize`].
    pub fn measure_cache(&self) {
        let jobs = Arc::clone(&self.jobs);
        let profile = self.profile.clone();
        spawn("cache-size", move || {
            let size = CACHE_DIRS.iter().map(|d| dir_size(&profile.join(d))).sum();
            if let Ok(mut jobs) = jobs.lock() {
                jobs.cache_size = Some(size);
            }
        });
    }

    /// Asks for a download folder (the desktop portal's folder chooser); the choice arrives
    /// as [`Event::FolderChosen`].
    pub fn choose_folder(&self) {
        let jobs = Arc::clone(&self.jobs);
        spawn("choose-folder", move || match choose_portal(tr("Download Folder"), Choose::Folder) {
            Ok(Some(folder)) => {
                if let Ok(mut jobs) = jobs.lock() {
                    jobs.folder = Some(folder);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("folder chooser unavailable: {e}"),
        });
    }

    /// Like [`Self::choose_folder`], for the Playlist's downloads.
    pub fn choose_playlist_folder(&self) {
        let jobs = Arc::clone(&self.jobs);
        spawn("choose-playlist-folder", move || match choose_portal(tr("Playlist Folder"), Choose::Folder) {
            Ok(Some(folder)) => {
                if let Ok(mut jobs) = jobs.lock() {
                    jobs.playlist_folder = Some(folder);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("folder chooser unavailable: {e}"),
        });
    }

    /// Results of background work since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let Ok(mut jobs) = self.jobs.lock() else { return Vec::new() };
        let mut events = Vec::new();
        if let Some(size) = jobs.cache_size.take() {
            events.push(Event::CacheSize(size));
        }
        if let Some(folder) = jobs.folder.take() {
            events.push(Event::FolderChosen(folder));
        }
        if let Some(folder) = jobs.playlist_folder.take() {
            events.push(Event::PlaylistFolderChosen(folder));
        }
        events
    }
}

/// "Clear Browsing Data" with the choices in `config.data`.
pub fn clear_browsing_data(config: &Config, store: Option<&Store>, engine: &mut impl Engine, now: i64) {
    let data = &config.data;
    let since = range_start(data.clear_range, now);
    // Without a time range all site data goes; otherwise that of the sites visited since
    // (Chromium can't clear cookies by time). Read before the history is cleared.
    let sites = if data.clear_cookies && since > 0 {
        store.and_then(|s| s.hosts_visited_since(since).ok()).unwrap_or_default()
    } else {
        Vec::new()
    };
    if let Some(store) = store {
        if data.clear_history
            && let Err(e) = store.clear_history_since(since)
        {
            tracing::error!("clearing history failed: {e}");
        }
        if data.clear_downloads
            && let Err(e) = store.clear_downloads_since(since)
        {
            tracing::error!("clearing the downloads list failed: {e}");
        }
    }
    tracing::info!(since, sites = sites.len(), "clearing browsing data");
    engine.clear_data(&ClearData {
        cache: data.clear_cache,
        all_site_data: data.clear_cookies && since == 0,
        sites,
    });
}

/// The first second a time range covers (0: all time).
pub fn range_start(range: ClearRange, now: i64) -> i64 {
    match range {
        ClearRange::LastHour => now - 60 * 60,
        ClearRange::Today => {
            let tz = TimeZone::system();
            Timestamp::from_second(now)
                .ok()
                .and_then(|t| t.to_zoned(tz).start_of_day().ok())
                .map_or(now - DAY_SECONDS, |z| z.timestamp().as_second())
        }
        ClearRange::LastWeek => now - 7 * DAY_SECONDS,
        ClearRange::AllTime => 0,
    }
}

/// The configured download folder, or the desktop's Downloads folder.
pub fn download_dir(config: &Config) -> PathBuf {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let configured = config.downloads.directory.trim();
    if !configured.is_empty() {
        return match (configured.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(configured),
        };
    }
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(Path::to_path_buf))
        .or_else(|| home.map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// `path` with the home folder written as `~` (for Settings).
pub fn short_path(path: &Path) -> String {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    match home.and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `short_path`, cut to its last two folders when long ("…/Shared/Downloads").
pub fn display_path(path: &Path) -> String {
    const MAX_CHARS: usize = 36;
    let short = short_path(path);
    if short.chars().count() <= MAX_CHARS {
        return short;
    }
    let tail: Vec<&str> = short.rsplit('/').filter(|s| !s.is_empty()).take(2).collect();
    format!("…/{}", tail.into_iter().rev().collect::<Vec<_>>().join("/"))
}

/// "7.9 of 40.0 MB · 1.0 MB/s" (the sidebar is narrow: the unit once where both share it).
fn running_detail(d: &DownloadInfo) -> String {
    let amount = if d.total_bytes > 0 {
        let (received, total) = (human_size(d.received_bytes), human_size(d.total_bytes));
        match (received.split_once(' '), total.split_once(' ')) {
            (Some((r, unit)), Some((_, total_unit))) if unit == total_unit => {
                tr_fmt!("{received} of {total}", received = r, total = total)
            }
            _ => tr_fmt!("{received} of {total}", received = received, total = total),
        }
    } else {
        human_size(d.received_bytes)
    };
    match d.state {
        DownloadState::Paused => tr_fmt!("Paused · {amount}", amount = amount),
        _ if d.speed > 0 => format!("{amount} · {}/s", human_size(d.speed)),
        _ => amount,
    }
}

/// 1536 → "1.5 KB", 3 145 728 → "3.0 MB".
pub fn human_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    match b {
        b if b < 1000.0 => tr_fmt!("{n} bytes", n = format!("{b:.0}")),
        b if b < 1000.0 * 1024.0 => format!("{:.1} KB", b / 1024.0),
        b if b < 1000.0 * 1024.0 * 1024.0 => format!("{:.1} MB", b / (1024.0 * 1024.0)),
        b => format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0)),
    }
}

fn is_safe(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SAFE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(work) {
        tracing::warn!("could not start {name}: {e}");
    }
}

/// Opens a file or folder in its default application.
pub(crate) fn open_path(path: &Path) {
    let path = path.to_path_buf();
    spawn("xdg-open", move || match std::process::Command::new("xdg-open").arg(&path).status() {
        Ok(status) if !status.success() => {
            tracing::warn!(path = %path.display(), "xdg-open failed: {status}")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("xdg-open unavailable: {e}"),
    });
}

/// Shows the file selected in the file manager, or at least opens its folder.
pub(crate) fn show_in_folder(path: &Path) {
    let path = path.to_path_buf();
    spawn("show-in-folder", move || {
        let shown = Connection::session().and_then(|c| {
            c.call_method(
                Some(FILE_MANAGER),
                FILE_MANAGER_PATH,
                Some(FILE_MANAGER),
                "ShowItems",
                &(vec![file_uri(&path)], ""),
            )
            .map(|_| ())
        });
        if let Err(e) = shown {
            tracing::debug!("FileManager1 unavailable ({e}); opening the folder");
            if let Some(dir) = path.parent() {
                open_path(dir);
            }
        }
    });
}

/// `file://` URI of an absolute path.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The path of a `file://` URI.
fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = rest.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// What [`choose_portal`] asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choose {
    Folder,
    Image,
    /// Pictures only (profile photos).
    Photo,
    /// Bookmark files to import (HTML or JSON).
    Bookmarks,
}

/// The XDG desktop portal's file chooser (`FileChooser.OpenFile`, for a folder with
/// `directory`). Blocks until the user picks one (`Some`) or cancels (`None`).
pub fn choose_portal(title: &str, what: Choose) -> zbus::Result<Option<PathBuf>> {
    let connection = Connection::session()?;
    let token = format!("axion_choose_{}", std::process::id());
    let sender =
        connection.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    let handle = format!("{PORTAL_PATH}/request/{sender}/{token}");
    // Subscribe before asking, so the answer can't be missed.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")?
        .member("Response")?
        .path(handle.as_str())?
        .build();
    let mut responses = MessageIterator::for_match_rule(rule, &connection, Some(1))?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    match what {
        Choose::Folder => {
            options.insert("directory", Value::from(true));
        }
        Choose::Photo => {
            let filters = vec![(tr("Images").to_owned(), vec![(1u32, "image/*".to_owned())])];
            options.insert("filters", Value::from(filters));
        }
        Choose::Bookmarks => {
            // (name, [(0 = glob pattern / 1 = MIME type, pattern)])
            let filters = vec![(
                tr("Bookmark files").to_owned(),
                vec![
                    (1u32, "text/html".to_owned()),
                    (1, "application/json".to_owned()),
                    (0, "*.html".to_owned()),
                    (0, "*.htm".to_owned()),
                    (0, "*.json".to_owned()),
                    (0, "Bookmarks".to_owned()),
                ],
            )];
            options.insert("filters", Value::from(filters));
        }
        Choose::Image => {
            // (name, [(1 = MIME type, pattern)])
            let filters = vec![(
                tr("Images and videos").to_owned(),
                vec![(1u32, "image/*".to_owned()), (1, "video/mp4".to_owned()), (1, "video/webm".to_owned())],
            )];
            options.insert("filters", Value::from(filters));
        }
    }
    options.insert("modal", Value::from(true));
    connection.call_method(
        Some(PORTAL),
        PORTAL_PATH,
        Some("org.freedesktop.portal.FileChooser"),
        "OpenFile",
        &("", title, options),
    )?;
    // The first (only) answer: 0 = chosen, otherwise cancelled.
    let Some(message) = responses.next() else { return Ok(None) };
    let (code, results): (u32, HashMap<String, OwnedValue>) = message?.body().deserialize()?;
    if code != 0 {
        return Ok(None);
    }
    let Some(uris) = results.get("uris") else { return Ok(None) };
    let Value::Array(uris) = &**uris else { return Ok(None) };
    Ok(uris.iter().find_map(|uri| match uri {
        Value::Str(uri) => uri_path(uri.as_str()),
        _ => None,
    }))
}

/// Total size of the files under `dir` (0 if it doesn't exist).
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_size(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 bytes");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn uris_round_trip() {
        let path = Path::new("/home/a b/Ünï%.txt");
        let uri = file_uri(path);
        assert_eq!(uri, "file:///home/a%20b/%C3%9Cn%C3%AF%25.txt");
        assert_eq!(uri_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn safe_files() {
        assert!(is_safe(Path::new("/x/photo.JPG")));
        assert!(is_safe(Path::new("/x/doc.pdf")));
        assert!(!is_safe(Path::new("/x/setup.sh")));
        assert!(!is_safe(Path::new("/x/page.html")));
        assert!(!is_safe(Path::new("/x/README")));
    }

    #[test]
    fn ranges() {
        let now = 1_800_000_000;
        assert_eq!(range_start(ClearRange::AllTime, now), 0);
        assert_eq!(range_start(ClearRange::LastHour, now), now - 3600);
        let today = range_start(ClearRange::Today, now);
        assert!(today <= now && now - today < DAY_SECONDS + 3600);
    }

    #[test]
    fn groups_downloads_by_day() {
        let store = Store::in_memory().unwrap();
        let now = 1_800_000_000;
        for (name, at) in [("a.zip", now - 10), ("b.zip", now - 20), ("c.zip", now - 3 * DAY_SECONDS)] {
            let d = StoredDownload {
                id: 0,
                url: format!("https://example.com/{name}"),
                path: format!("/nonexistent/{name}"),
                file_name: name.into(),
                mime_type: String::new(),
                size: 10,
                state: "complete".into(),
                finished: at,
            };
            store.add_download(&d).unwrap();
        }
        let mut downloads = Downloads::new(PathBuf::new());
        let items = downloads.items(List::Panel, Some(&store), &Config::default(), now, usize::MAX);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["a.zip", "b.zip", "c.zip"]);
        assert!(items[0].section.is_some() && items[1].section.is_none() && items[2].section.is_some());
        assert!(items[0].detail.starts_with("Moved or deleted"));
        assert!(downloads.has_finished());
    }
}
