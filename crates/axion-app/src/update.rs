//! Updates in the browser: the glue between `axion-update` and the interface.
//!
//! Checks and installs run on background threads and report through a channel that
//! [`Updater::poll`] drains every frame, so nothing here blocks the UI. Settings › About
//! shows [`AboutInfo`]; the confirmation and the progress of an install are an
//! [`AlertView`] in the glass alert. Everything stays inert until `updates.repository` or
//! `updates.manifest_url` is set (see `axion-update`).

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use axion_compositor::Icon;
use axion_config::Config;
use axion_i18n::{tr, tr_fmt};
use axion_update::{
    CheckReport, Clock, Http, InstallKind, Progress, Release, Source, Step, SystemClock, UpdateError,
    Version, plain_notes,
};

use crate::home::Image;
use crate::update_alert::{AlertView, Tone};

/// The running version (`Cargo.toml`) and, when built in a Git checkout, `git describe`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD: &str = env!("AXION_GIT_DESCRIBE");

/// Output lines kept of a running command (the alert shows the newest few).
const LOG_KEPT: usize = 400;
/// Paths in the alert longer than this are shortened.
const SHORT_PATH_CHARS: usize = 34;
/// Release notes longer than this are cut (the page shows them in a scrolling box).
const NOTES_MAX_CHARS: usize = 8000;

/// What the last check found.
#[derive(Clone, Debug)]
enum Status {
    /// Not checked in this run.
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Error(String),
}

/// A running install's state.
#[derive(Clone, Debug, Default)]
struct Running {
    step: Option<Step>,
    download: Option<(u64, Option<u64>)>,
    log: VecDeque<String>,
}

/// What a finished install left behind.
#[derive(Clone, Debug)]
enum Installed {
    AppImage { path: PathBuf, verified: bool },
    Source,
}

#[derive(Clone, Debug)]
enum Op {
    Idle,
    /// The confirmation alert is up.
    Confirm,
    Running(Running),
    Done(Installed),
    Failed {
        message: String,
        step: Option<Step>,
        log: Vec<String>,
    },
}

enum Event {
    Checked { manual: bool, result: Result<Box<CheckReport>, UpdateError> },
    Progress(Progress),
    Installed(Result<Installed, UpdateError>),
}

/// What [`Updater::poll`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A check finished; `update` is the newer release (if the check worked and found one).
    Checked { manual: bool, ok: bool, update: Option<Release> },
    /// The install's progress or result changed (the alert and the About page redraw).
    Changed,
}

/// Where an install stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Waiting for the answer to the confirmation.
    Confirm,
    Running,
    Done,
    Failed,
}

/// How this installation updates, for the About page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMode {
    AppImage,
    Source,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AboutState {
    /// Neither a repository nor a manifest address is set.
    NoSource,
    NeverChecked,
    Checking,
    UpToDate,
    Available {
        version: String,
        prerelease: bool,
    },
    /// An update was installed in this run; relaunching starts it.
    Installed {
        version: String,
    },
    Error(String),
}

/// Everything Settings › About shows.
#[derive(Clone)]
pub struct AboutInfo {
    pub version: String,
    /// `git describe` of the build (empty outside a checkout).
    pub build: String,
    pub icon: Option<Image>,
    pub mode: InstallMode,
    pub state: AboutState,
    /// Formatted time of the last check, empty when there was none.
    pub last_checked: String,
    /// The release notes of an available update as plain text.
    pub notes: String,
    /// The available update is one the user chose to skip.
    pub skipped: bool,
    /// An install is running.
    pub busy: bool,
}

impl Default for AboutInfo {
    fn default() -> Self {
        Self {
            version: VERSION.to_owned(),
            build: BUILD.to_owned(),
            icon: None,
            mode: InstallMode::Other,
            state: AboutState::NoSource,
            last_checked: String::new(),
            notes: String::new(),
            skipped: false,
            busy: false,
        }
    }
}

pub struct Updater {
    status: Status,
    op: Op,
    events: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Event>,
    kind: InstallKind,
    /// The executable as it was when Axion started (what a relaunch runs after a source update).
    exe: Option<PathBuf>,
    current: Version,
    cancel: Arc<AtomicBool>,
}

impl Updater {
    pub fn new() -> Self {
        Self::with_kind(axion_update::detect_current())
    }

    /// An updater for an installation of this kind (tests pick one; `new` detects it).
    pub fn with_kind(kind: InstallKind) -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            status: Status::Idle,
            op: Op::Idle,
            events,
            sender,
            kind,
            exe: axion_update::install::current_exe(),
            current: axion_update::running_version(VERSION),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn mode(&self) -> InstallMode {
        match self.kind {
            InstallKind::AppImage { .. } => InstallMode::AppImage,
            InstallKind::SourceCheckout { .. } => InstallMode::Source,
            InstallKind::Other => InstallMode::Other,
        }
    }

    /// The newer release a check found.
    pub fn available(&self) -> Option<&Release> {
        match &self.status {
            Status::Available(release) => Some(release),
            _ => None,
        }
    }

    pub fn is_checking(&self) -> bool {
        matches!(self.status, Status::Checking)
    }

    /// An install is running or waiting for confirmation (no second one may start).
    pub fn is_busy(&self) -> bool {
        matches!(self.op, Op::Running(_))
    }

    /// Whether a notice (badge, toast) about the available update is welcome: it is not
    /// skipped and no "remind me later" period is running.
    pub fn should_badge(&self, config: &Config) -> bool {
        self.available().is_some_and(|r| axion_update::may_notify(&config.updates, r, &SystemClock))
    }

    /// Starts a check on a background thread. Without a source this does nothing at all.
    pub fn check(&mut self, config: &Config, manual: bool) {
        if self.is_checking() || !config.updates.has_source() {
            return;
        }
        self.status = Status::Checking;
        let updates = config.updates.clone();
        let current = self.current.clone();
        let sender = self.sender.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let result = axion_update::check(&updates, &current).map(Box::new);
            // Fails only once the browser is gone.
            let _ = sender.send(Event::Checked { manual, result });
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the update check: {e}");
            self.status = Status::Error(e.to_string());
        }
    }

    /// Takes what the worker threads reported since the last call.
    pub fn poll(&mut self) -> Vec<Notice> {
        let mut notices = Vec::new();
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Checked { manual, result } => match result {
                    Ok(report) => {
                        self.status = match &report.update {
                            Some(release) => Status::Available(release.clone()),
                            None => Status::UpToDate,
                        };
                        notices.push(Notice::Checked { manual, ok: true, update: report.update });
                    }
                    Err(e) => {
                        tracing::info!("update check failed: {e}");
                        self.status = Status::Error(describe(&e));
                        notices.push(Notice::Checked { manual, ok: false, update: None });
                    }
                },
                Event::Progress(progress) => {
                    if let Op::Running(running) = &mut self.op {
                        match progress {
                            Progress::Step(step) => running.step = Some(step),
                            Progress::Line(line) => {
                                if running.log.len() >= LOG_KEPT {
                                    running.log.pop_front();
                                }
                                running.log.push_back(line);
                            }
                            Progress::Download { done, total } => running.download = Some((done, total)),
                        }
                    }
                    changed = true;
                }
                Event::Installed(result) => {
                    let (step, log) = match &self.op {
                        Op::Running(r) => (r.step, r.log.iter().cloned().collect()),
                        _ => (None, Vec::new()),
                    };
                    self.op = match result {
                        Ok(installed) => Op::Done(installed),
                        Err(UpdateError::Cancelled) => Op::Idle,
                        Err(e) => Op::Failed { message: describe(&e), step, log },
                    };
                    changed = true;
                }
            }
        }
        if changed {
            notices.push(Notice::Changed);
        }
        notices
    }

    /// The text of the confirmation alert, and the install waits for the answer. `None` when
    /// there is nothing to install here (no update, or a package-managed installation).
    pub fn prepare_install(&mut self) -> Option<AlertView> {
        let release = self.available()?.clone();
        if self.mode() == InstallMode::Other || self.is_busy() {
            return None;
        }
        self.op = Op::Confirm;
        self.alert_view(&release)
    }

    /// The user confirmed: downloads or pulls and builds in the background.
    pub fn start_install(&mut self, config: &Config) {
        let Some(release) = self.available().cloned() else { return };
        if !matches!(self.op, Op::Confirm) {
            return;
        }
        self.op = Op::Running(Running::default());
        self.cancel.store(false, Ordering::Relaxed);
        let (kind, updates, sender, cancel) =
            (self.kind.clone(), config.updates.clone(), self.sender.clone(), Arc::clone(&self.cancel));
        let spawned = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            let progress_sender = sender.clone();
            let mut progress = move |p: Progress| {
                let _ = progress_sender.send(Event::Progress(p));
            };
            let result = match kind {
                InstallKind::AppImage { path } => {
                    let http = Http::new(&updates, &axion_update::user_agent(VERSION));
                    axion_update::install_appimage(
                        &release,
                        &path,
                        std::env::consts::ARCH,
                        &http,
                        &cancel,
                        &mut progress,
                    )
                    .map(|done| Installed::AppImage { path: done.path, verified: done.verified })
                }
                InstallKind::SourceCheckout { root } => {
                    // The checkout's remote must be the configured repository: `repository`,
                    // even when a manifest decides which version is current.
                    axion_update::parse_repository(&updates.repository)
                        .map_err(|_| UpdateError::NoSource)
                        .and_then(|repository| {
                            axion_update::update_checkout(
                                &root,
                                &repository,
                                &axion_update::Tools::default(),
                                &mut progress,
                            )
                        })
                        .map(|()| Installed::Source)
                }
                InstallKind::Other => Err(UpdateError::NotSelfUpdating),
            };
            let _ = sender.send(Event::Installed(result));
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the update: {e}");
            self.op = Op::Failed { message: e.to_string(), step: None, log: Vec::new() };
        }
    }

    /// Stops a download; a source update cannot be stopped half way.
    pub fn cancel(&mut self) {
        if matches!(&self.op, Op::Running(r) if r.step != Some(Step::Pulling) && r.step != Some(Step::Building))
            && matches!(self.kind, InstallKind::AppImage { .. })
        {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The alert is gone (declined, or the result was read).
    pub fn dismiss(&mut self) {
        if !matches!(self.op, Op::Running(_)) {
            self.op = Op::Idle;
        }
    }

    /// The program to start after the update: the new AppImage, or the rebuilt binary.
    pub fn relaunch_exe(&self) -> Option<PathBuf> {
        match &self.op {
            Op::Done(Installed::AppImage { path, .. }) => Some(path.clone()),
            Op::Done(Installed::Source) => self.exe.clone(),
            _ => None,
        }
    }

    pub fn phase(&self) -> Phase {
        match self.op {
            Op::Idle => Phase::Idle,
            Op::Confirm => Phase::Confirm,
            Op::Running(_) => Phase::Running,
            Op::Done(_) => Phase::Done,
            Op::Failed { .. } => Phase::Failed,
        }
    }

    /// Forgets what the last check found (the source changed).
    pub fn reset(&mut self) {
        if !self.is_checking() && !self.is_busy() {
            self.status = Status::Idle;
        }
    }

    /// The alert for the current state of the install (`None` when idle).
    pub fn current_alert(&self) -> Option<AlertView> {
        let release = self.available()?;
        self.alert_view(release)
    }

    fn alert_view(&self, release: &Release) -> Option<AlertView> {
        let version = release.version.to_string();
        Some(match &self.op {
            Op::Idle => return None,
            Op::Confirm => self.confirm_view(release, &version),
            Op::Running(running) => self.running_view(running),
            Op::Done(installed) => {
                let mut lines = vec![tr("Relaunch Axion to start using it.").to_owned()];
                let title = match installed {
                    Installed::AppImage { path, verified } => {
                        lines.insert(
                            0,
                            if *verified {
                                tr("The download matched its published checksum.").to_owned()
                            } else {
                                tr("This release publishes no checksum, so the download could not be verified.")
                                    .to_owned()
                            },
                        );
                        let _ = path;
                        tr_fmt!("Axion {version} is installed", version = version)
                    }
                    Installed::Source => tr("Axion was updated from Git").to_owned(),
                };
                AlertView {
                    icon: Some(Icon::Check),
                    title,
                    lines,
                    primary: Some((tr("Relaunch").to_owned(), Tone::Accent)),
                    secondary: Some(tr("Later").to_owned()),
                    dismissable: true,
                    ..Default::default()
                }
            }
            Op::Failed { message, step, log } => {
                let mut lines = vec![message.clone()];
                match step {
                    Some(Step::Building) => lines.push(
                        tr("The source was updated, but building it failed. The previous build is still in place.")
                            .to_owned(),
                    ),
                    Some(Step::Pulling | Step::CheckingTree) => {
                        lines.push(tr("Nothing was changed.").to_owned());
                    }
                    _ => lines.push(tr("The current version was left as it was.").to_owned()),
                }
                AlertView {
                    icon: Some(Icon::Close),
                    title: tr("Update failed").to_owned(),
                    lines,
                    log: log.clone(),
                    show_log: !log.is_empty(),
                    primary: Some((tr("Close").to_owned(), Tone::Plain)),
                    error: true,
                    dismissable: true,
                    ..Default::default()
                }
            }
        })
    }

    fn confirm_view(&self, release: &Release, version: &str) -> AlertView {
        match &self.kind {
            InstallKind::AppImage { path } => {
                let asset = release.appimage_asset(std::env::consts::ARCH);
                let file = asset.map_or_else(|| tr("the AppImage").to_owned(), |a| a.name.clone());
                let size = asset
                    .and_then(|a| a.size)
                    .map_or_else(|| tr("size unknown").to_owned(), |s| crate::downloads::human_size(s as i64));
                let backup = format!(
                    "{}.old",
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                );
                let checksum = release.checksums_asset().is_some()
                    || asset.is_some_and(|a| a.sha256.is_some());
                AlertView {
                    icon: Some(Icon::Download),
                    title: tr_fmt!("Update Axion to {version}?", version = version),
                    lines: vec![
                        tr_fmt!(
                            "Axion downloads {file} ({size}) and replaces {path}. The current version is kept next to it as {backup}.",
                            file = file,
                            size = size,
                            path = short_path(path),
                            backup = backup
                        ),
                        if checksum {
                            tr("The download is checked against the release's published checksum first.")
                                .to_owned()
                        } else {
                            tr("This release publishes no checksum, so the download can't be verified.")
                                .to_owned()
                        },
                    ],
                    primary: Some((tr("Update").to_owned(), Tone::Accent)),
                    secondary: Some(tr("Cancel").to_owned()),
                    dismissable: true,
                    ..Default::default()
                }
            }
            InstallKind::SourceCheckout { root } => AlertView {
                icon: Some(Icon::Download),
                title: tr("Update from Git?").to_owned(),
                lines: vec![
                    tr_fmt!(
                        "In {path}, Axion runs “git pull --ff-only” and then “cargo build -p axion-app --release”. This can take several minutes.",
                        path = short_path(root)
                    ),
                    tr("Only a clean working tree whose remote is the configured repository is updated. Nothing is forced or reset.")
                        .to_owned(),
                ],
                primary: Some((tr("Update from Git").to_owned(), Tone::Accent)),
                secondary: Some(tr("Cancel").to_owned()),
                dismissable: true,
                ..Default::default()
            },
            InstallKind::Other => AlertView::default(),
        }
    }

    fn running_view(&self, running: &Running) -> AlertView {
        let source = matches!(self.kind, InstallKind::SourceCheckout { .. });
        let status = match running.step {
            Some(Step::Downloading) | None => match running.download {
                Some((done, Some(total))) => Some(tr_fmt!(
                    "Downloading… {done} of {total}",
                    done = crate::downloads::human_size(done as i64),
                    total = crate::downloads::human_size(total as i64)
                )),
                Some((done, None)) => {
                    Some(tr_fmt!("Downloading… {done}", done = crate::downloads::human_size(done as i64)))
                }
                None => Some(tr("Starting…").to_owned()),
            },
            Some(Step::Verifying) => Some(tr("Verifying the download…").to_owned()),
            Some(Step::Installing) => Some(tr("Installing…").to_owned()),
            Some(Step::CheckingTree) => Some(tr("Checking the working tree…").to_owned()),
            Some(Step::Pulling) => Some(tr("Pulling changes…").to_owned()),
            Some(Step::Building) => Some(tr("Building Axion… this takes a while.").to_owned()),
        };
        let progress = match (running.step, running.download) {
            (Some(Step::Downloading) | None, Some((done, Some(total)))) if total > 0 => {
                Some(done as f32 / total as f32)
            }
            _ => None,
        };
        let cancellable = !source && matches!(running.step, Some(Step::Downloading) | None);
        AlertView {
            icon: Some(Icon::Download),
            title: if source { tr("Updating from Git…") } else { tr("Updating Axion…") }.to_owned(),
            log: running.log.iter().cloned().collect(),
            show_log: source,
            progress,
            status,
            secondary: cancellable.then(|| tr("Cancel").to_owned()),
            ..Default::default()
        }
    }

    /// Everything Settings › About shows.
    pub fn about(&self, config: &Config, icon: Option<Image>) -> AboutInfo {
        let u = &config.updates;
        let state = match (&self.op, &self.status) {
            (Op::Done(_), Status::Available(r)) => AboutState::Installed { version: r.version.to_string() },
            (_, Status::Checking) => AboutState::Checking,
            (_, Status::Available(r)) => {
                AboutState::Available { version: r.version.to_string(), prerelease: r.prerelease }
            }
            (_, Status::Error(e)) => AboutState::Error(e.clone()),
            _ if !u.has_source() => AboutState::NoSource,
            (_, Status::UpToDate) => AboutState::UpToDate,
            (_, Status::Idle) => AboutState::NeverChecked,
        };
        let (notes, skipped) = match self.available() {
            Some(r) => (plain_notes(&r.notes, NOTES_MAX_CHARS), axion_update::is_skipped(u, r)),
            None => (String::new(), false),
        };
        AboutInfo {
            version: VERSION.to_owned(),
            build: BUILD.to_owned(),
            icon,
            mode: self.mode(),
            state,
            last_checked: if u.last_checked == 0 { String::new() } else { format_time(u.last_checked) },
            notes,
            skipped,
            busy: self.is_busy(),
        }
    }

    /// The release page of the available update (or of the repository's releases).
    pub fn release_page(&self) -> Option<String> {
        self.available().map(|r| r.page_url.clone()).filter(|u| !u.is_empty())
    }
}

/// A path short enough for a line of the alert: long ones keep their last two parts.
fn short_path(path: &std::path::Path) -> String {
    let full = path.display().to_string();
    if full.chars().count() <= SHORT_PATH_CHARS {
        return full;
    }
    let parts: Vec<String> =
        path.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    match parts.as_slice() {
        [name, parent] => format!("…/{parent}/{name}"),
        _ => full,
    }
}

/// Error text for the interface: the common cases translated, the rest as the library words them.
fn describe(error: &UpdateError) -> String {
    match error {
        UpdateError::NoSource => tr("No update source is set yet.").to_owned(),
        UpdateError::Network(_) => {
            tr("Couldn't reach the update server. Check your connection and try again.").to_owned()
        }
        UpdateError::Parse(_) => tr("The update server sent something Axion doesn't understand.").to_owned(),
        UpdateError::ChecksumMismatch { .. } => {
            tr("The download doesn't match its published checksum, so it was discarded.").to_owned()
        }
        UpdateError::DirtyTree => {
            tr("The checkout has uncommitted changes. Commit or stash them first.").to_owned()
        }
        UpdateError::NoUpstream => {
            tr("The checkout isn't on a branch that follows a remote branch.").to_owned()
        }
        UpdateError::RemoteMismatch { remote, repository } => tr_fmt!(
            "The checkout's remote ({remote}) isn't the configured repository ({repository}).",
            remote = remote.as_str(),
            repository = repository.as_str()
        ),
        UpdateError::NoAsset => tr("This release has no download for this installation.").to_owned(),
        UpdateError::TooLarge { .. } => tr("The download is larger than the allowed size.").to_owned(),
        other => other.to_string(),
    }
}

/// "2026-10-26 14:32" in the local time zone.
fn format_time(unix: u64) -> String {
    let Ok(seconds) = i64::try_from(unix) else { return String::new() };
    jiff::Timestamp::from_second(seconds)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// `axion --check-update`: asks the configured source once and prints the answer. Exit code 0
/// when the check worked (an update or not), 1 when it failed, 2 when no source is set.
pub fn check_cli(config: &Config) -> i32 {
    let mut out = std::io::stdout().lock();
    let build = if BUILD.is_empty() { String::new() } else { format!(" ({BUILD})") };
    let _ = writeln!(out, "Axion {VERSION}{build}");
    let kind = match axion_update::detect_current() {
        InstallKind::AppImage { path } => format!("AppImage ({})", path.display()),
        InstallKind::SourceCheckout { root } => format!("source checkout ({})", root.display()),
        InstallKind::Other => "package or other".to_owned(),
    };
    let _ = writeln!(out, "Installation: {kind}");
    match Source::from_config(&config.updates) {
        Err(UpdateError::NoSource) => {
            let _ = writeln!(out, "No update source is set (updates.repository / updates.manifest_url).");
            return 2;
        }
        Err(e) => {
            let _ = writeln!(out, "Invalid update source: {e}");
            return 1;
        }
        Ok(source) => {
            let _ = writeln!(out, "Source: {source:?}");
        }
    }
    let current = axion_update::running_version(VERSION);
    match axion_update::check(&config.updates, &current) {
        Ok(CheckReport { update: Some(release), .. }) => {
            let _ = writeln!(out, "Update available: {} ({})", release.version, release.tag);
            if !release.page_url.is_empty() {
                let _ = writeln!(out, "Release page: {}", release.page_url);
            }
            for asset in &release.assets {
                let _ = writeln!(out, "Asset: {} {}", asset.name, asset.url);
            }
            let notes = plain_notes(&release.notes, NOTES_MAX_CHARS);
            if !notes.is_empty() {
                let _ = writeln!(out, "\n{notes}");
            }
            0
        }
        Ok(CheckReport { latest, .. }) => {
            let latest = latest.map_or_else(|| "none published".to_owned(), |r| r.version.to_string());
            let _ = writeln!(out, "Up to date (latest release: {latest}).");
            0
        }
        Err(e) => {
            let _ = writeln!(out, "Check failed: {e}");
            1
        }
    }
}

/// The current time for scheduling (injected in tests of the library; the app uses the system's).
pub fn now() -> u64 {
    SystemClock.now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_checked_without_a_source() {
        let mut config = Config::default();
        config.updates.repository.clear();
        let mut updater = Updater::new();
        updater.check(&config, true);
        assert!(!updater.is_checking());
        assert!(updater.poll().is_empty());
        let about = updater.about(&config, None);
        assert_eq!(about.state, AboutState::NoSource);
        assert!(about.last_checked.is_empty());
    }

    #[test]
    fn long_paths_are_shortened_for_the_alert() {
        use std::path::Path;
        assert_eq!(short_path(Path::new("/opt/Axion.AppImage")), "/opt/Axion.AppImage");
        assert_eq!(
            short_path(Path::new("/home/someone/Downloads/applications/Axion-x86_64.AppImage")),
            "…/applications/Axion-x86_64.AppImage"
        );
    }

    #[test]
    fn times_are_formatted_or_empty() {
        assert_eq!(format_time(u64::MAX), "");
        assert_eq!(format_time(0).len(), "1970-01-01 00:00".len());
    }

    #[test]
    fn running_view_shows_progress() {
        let updater = Updater::with_kind(InstallKind::AppImage { path: "/opt/Axion.AppImage".into() });
        let running =
            Running { step: Some(Step::Downloading), download: Some((50, Some(200))), ..Default::default() };
        let view = updater.running_view(&running);
        assert_eq!(view.progress, Some(0.25));
        assert!(!view.show_log);
        assert!(view.secondary.is_some(), "a download can be cancelled");

        let source = Updater::with_kind(InstallKind::SourceCheckout { root: "/src/axion".into() });
        let building = Running { step: Some(Step::Building), ..Default::default() };
        let view = source.running_view(&building);
        assert!(
            view.show_log && view.secondary.is_none(),
            "a source update shows its log and cannot be cancelled"
        );
    }
}
