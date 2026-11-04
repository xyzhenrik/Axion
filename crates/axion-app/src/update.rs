//! Updates in the browser: the glue between `axion-update` and the interface.
//!
//! Checks and installs run on background threads and report through a channel that
//! [`Updater::poll`] drains every frame, so nothing here blocks the UI. Settings › About
//! shows [`AboutInfo`]; the confirmation of an install is an [`AlertView`] in the glass alert
//! and its progress, result and failure are the page's [`RunInfo`] (the sheet closes as soon
//! as the update starts). The source is `updates.manifest_url`, else
//! `updates.repository`, else the default repository; everything stays inert while all are
//! empty (see `axion-update`).

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use axion_compositor::Icon;
use axion_config::Config;
use axion_i18n::{tr, tr_fmt};
use axion_update::{
    CheckReport, Clock, Http, InstallKind, Progress, Release, Source, Staged, Step, SystemClock, Tools,
    UpdateError, Version, plain_notes,
};

use crate::home::Image;
use crate::update_alert::{AlertView, Tone};

/// The running version (`Cargo.toml`) and, when built in a Git checkout, `git describe`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD: &str = env!("AXION_GIT_DESCRIBE");

/// Output lines kept of a running command.
const LOG_KEPT: usize = 400;
/// The failure shows this many of the newest output lines.
const LOG_TAIL: usize = 12;
/// A line of output shown under the progress bar is cut to this many characters.
const DETAIL_CHARS: usize = 110;
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
    AppImage {
        path: PathBuf,
        verified: bool,
    },
    /// The tarball is unpacked next to the installation; the swap happens at the restart.
    Tarball {
        staged: Staged,
    },
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
        /// More lines explaining it (the dirty files, what to do).
        details: Vec<String>,
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
    Tarball,
    Source,
    Other,
}

/// What the "Software update" section shows of an install that is running or has just ended.
#[derive(Clone, Debug, PartialEq)]
pub struct RunInfo {
    pub phase: Phase,
    pub title: String,
    /// The step ("Building Axion…") or the explanation of the result.
    pub status: String,
    /// The newest line of the command's output.
    pub detail: String,
    /// 0..1, or `None` for a step of unknown length (an indeterminate bar).
    pub progress: Option<f32>,
    /// A failure's reason and advice, one paragraph per entry.
    pub lines: Vec<String>,
    /// A failure's newest output lines.
    pub log: Vec<String>,
    pub cancellable: bool,
    /// The finished update was put away with "Later": only the one-line reminder shows.
    pub collapsed: bool,
    /// A failure offers the release page too.
    pub release_page: bool,
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
    /// The install that is running or has just ended.
    pub run: Option<RunInfo>,
    /// The version an update installed in this run: the running build is older ("restart to
    /// finish the update"), so version and build above are stale.
    pub pending_restart: Option<String>,
    /// A tarball installation Axion cannot write to (its folder): updates are done by hand.
    pub read_only: Option<String>,
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
            run: None,
            pending_restart: None,
            read_only: None,
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
    /// The repository used while none is typed (tests inject another one).
    default_repository: String,
    /// git and cargo (tests inject stand-ins).
    tools: Tools,
    /// A finished update was put away with "Later".
    later: bool,
    /// The version being installed (or installed in this run).
    run_version: String,
    /// Whether a tarball installation's folder can be replaced (decided when the updater
    /// starts; `None` for the other kinds).
    tarball_writable: Option<bool>,
}

impl Updater {
    pub fn new() -> Self {
        Self::with_kind(axion_update::detect_current())
    }

    /// An updater for an installation of this kind (tests pick one; `new` detects it).
    pub fn with_kind(kind: InstallKind) -> Self {
        let kind_for_probe = kind.clone();
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
            default_repository: axion_config::DEFAULT_REPOSITORY.to_owned(),
            tools: Tools::default(),
            later: false,
            run_version: String::new(),
            tarball_writable: match &kind_for_probe {
                InstallKind::Tarball { dir } => Some(axion_update::tarball::check_writable(dir).is_ok()),
                _ => None,
            },
        }
    }

    /// `updates` with the repository in effect written into it, so everything downstream
    /// (`axion_update`) sees one explicit value.
    fn resolved(&self, updates: &axion_config::Updates) -> axion_config::Updates {
        let mut resolved = updates.clone();
        resolved.repository = updates.effective_repository_or(&self.default_repository).to_owned();
        resolved
    }

    pub fn mode(&self) -> InstallMode {
        match self.kind {
            InstallKind::AppImage { .. } => InstallMode::AppImage,
            InstallKind::Tarball { .. } if self.tarball_writable == Some(false) => InstallMode::Other,
            InstallKind::Tarball { .. } => InstallMode::Tarball,
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
        if self.is_checking() || !config.updates.has_source_or(&self.default_repository) {
            return;
        }
        self.status = Status::Checking;
        let updates = self.resolved(&config.updates);
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
                    self.later = false;
                    self.op = match result {
                        Ok(installed) => Op::Done(installed),
                        Err(UpdateError::Cancelled) => Op::Idle,
                        Err(e) => Op::Failed { message: describe(&e), details: advice(&e), step, log },
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
        self.later = false;
        self.run_version = release.version.to_string();
        self.cancel.store(false, Ordering::Relaxed);
        let (kind, updates, sender, cancel, tools) = (
            self.kind.clone(),
            self.resolved(&config.updates),
            self.sender.clone(),
            Arc::clone(&self.cancel),
            self.tools.clone(),
        );
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
                InstallKind::Tarball { dir } => {
                    let http = Http::new(&updates, &axion_update::user_agent(VERSION));
                    let max_extracted = u64::from(updates.max_extracted_mb) * 1024 * 1024;
                    axion_update::tarball::stage_tarball(
                        &release,
                        &dir,
                        std::env::consts::ARCH,
                        &http,
                        max_extracted,
                        &cancel,
                        &mut progress,
                    )
                    .map(|staged| Installed::Tarball { staged })
                }
                InstallKind::SourceCheckout { root } => {
                    // The checkout's remote must be the configured repository: `repository`,
                    // even when a manifest decides which version is current.
                    axion_update::parse_repository(&updates.repository)
                        .map_err(|_| UpdateError::NoSource)
                        .and_then(|repository| {
                            axion_update::update_checkout(&root, &repository, &tools, &cancel, &mut progress)
                        })
                        .map(|()| Installed::Source)
                }
                InstallKind::Other => Err(UpdateError::NotSelfUpdating),
            };
            let _ = sender.send(Event::Installed(result));
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the update: {e}");
            self.op = Op::Failed { message: e.to_string(), details: Vec::new(), step: None, log: Vec::new() };
        }
    }

    /// Stops the running update: a download or unpacking is dropped, and git / cargo get a
    /// SIGTERM (cargo keeps the previous binary until a build links, so it stays usable).
    pub fn cancel(&mut self) {
        if matches!(self.op, Op::Running(_)) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// "Later" on a finished update: only the reminder to restart stays.
    pub fn defer(&mut self) {
        if matches!(self.op, Op::Done(_)) {
            self.later = true;
        }
    }

    /// "Try again" after a failure: runs the confirmed update once more.
    pub fn retry(&mut self, config: &Config) {
        if matches!(self.op, Op::Failed { .. }) {
            self.op = Op::Confirm;
            self.start_install(config);
        }
    }

    /// The staged tree of a tarball update that has not been swapped in yet.
    pub fn pending_swap(&self) -> Option<Staged> {
        match &self.op {
            Op::Done(Installed::Tarball { staged }) => Some(staged.clone()),
            _ => None,
        }
    }

    /// The alert is gone (declined, or the result was read).
    pub fn dismiss(&mut self) {
        if matches!(self.op, Op::Confirm | Op::Failed { .. }) {
            self.op = Op::Idle;
        }
    }

    /// The program to start after the update: the new AppImage, or the rebuilt binary.
    pub fn relaunch_exe(&self) -> Option<PathBuf> {
        match &self.op {
            Op::Done(Installed::AppImage { path, .. }) => Some(path.clone()),
            Op::Done(Installed::Source | Installed::Tarball { .. }) => self.exe.clone(),
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

    /// Only the confirmation is a sheet; progress and results are on the page ([`RunInfo`]).
    fn alert_view(&self, release: &Release) -> Option<AlertView> {
        let version = release.version.to_string();
        match &self.op {
            Op::Confirm => Some(self.confirm_view(release, &version)),
            _ => None,
        }
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
            InstallKind::Tarball { dir } => {
                let asset = release.tarball_asset(std::env::consts::ARCH);
                let file = asset.map_or_else(|| tr("the tarball").to_owned(), |a| a.name.clone());
                let size = asset
                    .and_then(|a| a.size)
                    .map_or_else(|| tr("size unknown").to_owned(), |s| crate::downloads::human_size(s as i64));
                let checksum = release.checksums_asset().is_some() || asset.is_some_and(|a| a.sha256.is_some());
                AlertView {
                    icon: Some(Icon::Download),
                    title: tr_fmt!("Update Axion to {version}?", version = version),
                    lines: vec![
                        tr_fmt!(
                            "Axion downloads {file} ({size}) and unpacks it next to {path}. {path} itself is replaced only when you restart Axion; the current version is kept as a backup.",
                            file = file,
                            size = size,
                            path = short_path(dir)
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
                        "In {path}, Axion runs “git fetch”, then “git pull --ff-only” if there is something new, and “cargo build -p axion-app --release”. This can take several minutes.",
                        path = short_path(root)
                    ),
                    tr("Changes to tracked files stop the update; untracked files are fine. Local commits are never merged, rebased or reset, and nothing is forced.")
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

    /// What the page shows of the install: progress while it runs, the result after it.
    pub fn run_info(&self) -> Option<RunInfo> {
        let version = self.run_version.clone();
        match &self.op {
            Op::Idle | Op::Confirm => None,
            Op::Running(running) => Some(self.running_info(running)),
            Op::Done(installed) => {
                let status = match installed {
                    Installed::AppImage { verified, .. } => {
                        let check = if *verified {
                            tr("The download matched its published checksum.")
                        } else {
                            tr("This release publishes no checksum, so the download could not be verified.")
                        };
                        format!("{check} {}", tr("Restart Axion to start using it."))
                    }
                    Installed::Tarball { .. } => tr(
                        "The new version is unpacked and checked. It replaces the current one when Axion restarts; the current one is kept as a backup.",
                    )
                    .to_owned(),
                    Installed::Source => tr(
                        "The source was updated and built. Restart Axion to finish the update: until then this window still runs the old build.",
                    )
                    .to_owned(),
                };
                Some(RunInfo {
                    phase: Phase::Done,
                    title: tr_fmt!("Updated to {version}", version = version.as_str()),
                    status,
                    detail: String::new(),
                    progress: Some(1.0),
                    lines: Vec::new(),
                    log: Vec::new(),
                    cancellable: false,
                    collapsed: self.later,
                    release_page: false,
                })
            }
            Op::Failed { message, details, step, log } => {
                let mut lines = vec![message.clone()];
                match step {
                    Some(Step::Building) => lines.push(
                        tr("The source was updated, but building it failed. The previous build is still in place.")
                            .to_owned(),
                    ),
                    Some(Step::CheckingTree | Step::Fetching | Step::Pulling) => {
                        lines.push(tr("Nothing was changed.").to_owned());
                    }
                    _ => lines.push(tr("The current version was left as it was.").to_owned()),
                }
                lines.extend(details.iter().cloned());
                let start = log.len().saturating_sub(LOG_TAIL);
                Some(RunInfo {
                    phase: Phase::Failed,
                    title: tr("Update failed").to_owned(),
                    status: String::new(),
                    detail: String::new(),
                    progress: None,
                    lines,
                    log: log[start..].to_vec(),
                    cancellable: false,
                    collapsed: false,
                    release_page: !matches!(self.kind, InstallKind::SourceCheckout { .. }),
                })
            }
        }
    }

    fn running_info(&self, running: &Running) -> RunInfo {
        let source = matches!(self.kind, InstallKind::SourceCheckout { .. });
        let status = match running.step {
            Some(Step::Downloading) | None => match running.download {
                Some((done, Some(total))) => tr_fmt!(
                    "Downloading… {done} of {total}",
                    done = crate::downloads::human_size(done as i64),
                    total = crate::downloads::human_size(total as i64)
                ),
                Some((done, None)) => {
                    tr_fmt!("Downloading… {done}", done = crate::downloads::human_size(done as i64))
                }
                None => tr("Starting…").to_owned(),
            },
            Some(Step::Verifying) => tr("Verifying the download…").to_owned(),
            Some(Step::Extracting) => tr("Unpacking the new version…").to_owned(),
            Some(Step::Installing) => tr("Installing…").to_owned(),
            Some(Step::CheckingTree) => tr("Checking the working tree…").to_owned(),
            Some(Step::Fetching) => tr("Fetching changes…").to_owned(),
            Some(Step::Pulling) => tr("Pulling changes…").to_owned(),
            Some(Step::Building) => tr("Building Axion… this takes a while.").to_owned(),
        };
        // Downloads and unpacking know their size; git and cargo do not (a moving bar).
        let progress = match (running.step, running.download) {
            (Some(Step::Downloading | Step::Extracting) | None, Some((done, Some(total)))) if total > 0 => {
                Some((done as f32 / total as f32).clamp(0.0, 1.0))
            }
            _ => None,
        };
        let detail = running
            .log
            .back()
            .map(|line| line.chars().take(DETAIL_CHARS).collect::<String>())
            .unwrap_or_default();
        RunInfo {
            phase: Phase::Running,
            title: if source { tr("Updating from Git…") } else { tr("Updating Axion…") }.to_owned(),
            status,
            detail,
            progress,
            lines: Vec::new(),
            log: Vec::new(),
            cancellable: true,
            collapsed: false,
            release_page: false,
        }
    }

    /// Everything Settings › About shows.
    pub fn about(&self, config: &Config, icon: Option<Image>) -> AboutInfo {
        let u = &config.updates;
        let state = match (&self.op, &self.status) {
            (Op::Done(_), _) => AboutState::Installed { version: self.run_version.clone() },
            (_, Status::Checking) => AboutState::Checking,
            (_, Status::Available(r)) => {
                AboutState::Available { version: r.version.to_string(), prerelease: r.prerelease }
            }
            (_, Status::Error(e)) => AboutState::Error(e.clone()),
            _ if !u.has_source_or(&self.default_repository) => AboutState::NoSource,
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
            run: self.run_info(),
            pending_restart: matches!(self.op, Op::Done(_)).then(|| self.run_version.clone()),
            read_only: match (&self.kind, self.tarball_writable) {
                (InstallKind::Tarball { dir }, Some(false)) => Some(dir.display().to_string()),
                _ => None,
            },
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
        UpdateError::DirtyTree { .. } => {
            tr("The checkout has uncommitted changes to tracked files, so Axion left it alone.").to_owned()
        }
        UpdateError::LocalAhead { ahead, upstream } => tr_fmt!(
            "The checkout is {ahead} commit(s) ahead of {upstream} and has nothing new to take from it.",
            ahead = ahead.to_string(),
            upstream = upstream.as_str()
        ),
        UpdateError::Diverged { ahead, behind, upstream } => tr_fmt!(
            "The checkout has diverged from {upstream}: {ahead} local and {behind} new commit(s).",
            ahead = ahead.to_string(),
            behind = behind.to_string(),
            upstream = upstream.as_str()
        ),
        UpdateError::NotWritable(path) => tr_fmt!(
            "Axion can't write to {path}, so it can't replace this installation.",
            path = path.display().to_string()
        ),
        UpdateError::BadArchive(why) => {
            tr_fmt!("The downloaded archive was rejected: {why}", why = why.as_str())
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

/// What to do about a failure, one line each (the dirty files first).
fn advice(error: &UpdateError) -> Vec<String> {
    match error {
        UpdateError::DirtyTree { files } => {
            let mut lines: Vec<String> = files.iter().take(10).cloned().collect();
            if files.len() > 10 {
                lines.push(tr_fmt!("…and {count} more", count = (files.len() - 10).to_string()));
            }
            lines.push(
                tr("Commit, stash or revert these changes (see “git status”), then try again. Untracked files do not matter.")
                    .to_owned(),
            );
            lines
        }
        UpdateError::LocalAhead { .. } => vec![
            tr("Push your commits or reset the branch yourself; Axion never rewrites your history.")
                .to_owned(),
        ],
        UpdateError::Diverged { .. } => vec![
            tr("Rebase or merge the branch yourself (git pull --rebase), then try again. Axion never does it for you.")
                .to_owned(),
        ],
        UpdateError::NotWritable(_) => vec![
            tr("This installation is managed elsewhere or needs permissions. Download the new version from the release page.")
                .to_owned(),
        ],
        _ => Vec::new(),
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
        InstallKind::Tarball { dir } => format!("tarball / portable folder ({})", dir.display()),
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
        let config = Config::default();
        let mut updater = Updater::new();
        updater.default_repository.clear();
        updater.check(&config, true);
        assert!(!updater.is_checking());
        assert!(updater.poll().is_empty());
        let about = updater.about(&config, None);
        assert_eq!(about.state, AboutState::NoSource);
        assert!(about.last_checked.is_empty());
    }

    #[test]
    fn the_about_state_follows_the_effective_repository() {
        let mut updater = Updater::new();
        let mut config = Config::default();
        // Default applies while nothing is typed.
        updater.default_repository = "def/ault".into();
        assert_eq!(updater.about(&config, None).state, AboutState::NeverChecked);
        assert_eq!(updater.resolved(&config.updates).repository, "def/ault");
        // A typed repository overrides it.
        config.updates.set_repository("me/fork");
        assert_eq!(updater.resolved(&config.updates).repository, "me/fork");
        assert_eq!(updater.about(&config, None).state, AboutState::NeverChecked);
        // Cleared again: the default; with no default either, no source.
        config.updates.set_repository("");
        assert_eq!(updater.resolved(&config.updates).repository, "def/ault");
        updater.default_repository.clear();
        assert_eq!(updater.about(&config, None).state, AboutState::NoSource);
        // A manifest alone is a source.
        config.updates.manifest_url = "https://example.org/latest.json".into();
        assert_eq!(updater.about(&config, None).state, AboutState::NeverChecked);
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

    // ---- the install state machine: confirm -> running -> done / failed ----

    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.org")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.org")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    struct Checkout {
        dir: tempfile::TempDir,
        root: PathBuf,
    }

    /// A clone of `<tmp>/henrik/axion.git` that is one commit behind it.
    fn checkout(cargo_script: &str) -> Checkout {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path();
        let bare = base.join("henrik/axion.git");
        std::fs::create_dir_all(&bare).expect("mkdir");
        git(&bare, &["init", "--bare", "-b", "main"]);
        let root = base.join("checkout");
        git(base, &["clone", bare.to_str().expect("utf8"), "checkout"]);
        std::fs::write(root.join("Cargo.toml"), b"[workspace]\n").expect("write");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "first"]);
        git(&root, &["push", "-u", "origin", "main"]);
        git(base, &["clone", bare.to_str().expect("utf8"), "developer"]);
        let developer = base.join("developer");
        std::fs::write(developer.join("feature.txt"), b"new\n").expect("write");
        git(&developer, &["add", "."]);
        git(&developer, &["commit", "-m", "second"]);
        git(&developer, &["push"]);
        let cargo = base.join("cargo");
        std::fs::write(&cargo, cargo_script).expect("write");
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        Checkout { dir, root }
    }

    const QUICK_CARGO: &str = "#!/bin/sh\necho '   Compiling axion-app v0.0.1'\nexit 0\n";

    fn release(version: &str) -> Release {
        Release {
            version: Version::parse(version).expect("version"),
            tag: format!("v{version}"),
            name: String::new(),
            notes: String::new(),
            page_url: "https://example.org/release".into(),
            prerelease: false,
            assets: Vec::new(),
        }
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.updates.set_repository("henrik/axion");
        config
    }

    fn updater_for(c: &Checkout) -> Updater {
        let mut updater = Updater::with_kind(InstallKind::SourceCheckout { root: c.root.clone() });
        updater.tools = Tools { git: "git".into(), cargo: c.dir.path().join("cargo") };
        updater.status = Status::Available(release("9.0.0"));
        updater
    }

    /// Polls until the install is no longer running (like the app's frame loop).
    fn finish(updater: &mut Updater) -> Vec<Notice> {
        let mut all = Vec::new();
        let started = Instant::now();
        while updater.phase() == Phase::Running {
            assert!(started.elapsed() < Duration::from_secs(30), "the update never ended");
            all.extend(updater.poll());
            std::thread::sleep(Duration::from_millis(10));
        }
        all.extend(updater.poll());
        all
    }

    #[test]
    fn confirm_then_running_then_done() {
        let c = checkout(QUICK_CARGO);
        let config = config();
        let mut updater = updater_for(&c);
        assert_eq!(updater.phase(), Phase::Idle);
        assert!(updater.run_info().is_none());

        let sheet = updater.prepare_install().expect("asks first");
        assert_eq!(updater.phase(), Phase::Confirm);
        assert!(sheet.primary.is_some() && sheet.secondary.is_some());
        assert!(updater.current_alert().is_some(), "the sheet shows while confirming");

        updater.start_install(&config);
        assert_eq!(updater.phase(), Phase::Running);
        assert!(updater.current_alert().is_none(), "the sheet is gone as soon as the update runs");
        let running = updater.run_info().expect("the page shows progress");
        assert_eq!(running.phase, Phase::Running);
        assert!(running.cancellable);
        assert!(updater.about(&config, None).busy);

        let notices = finish(&mut updater);
        assert!(notices.contains(&Notice::Changed));
        assert_eq!(updater.phase(), Phase::Done);
        assert!(c.root.join("feature.txt").exists(), "pulled");
        let done = updater.run_info().expect("result");
        assert_eq!(done.phase, Phase::Done);
        assert!(done.title.contains("9.0.0"), "{}", done.title);
        assert!(!done.collapsed);
        let about = updater.about(&config, None);
        assert_eq!(about.state, AboutState::Installed { version: "9.0.0".into() });
        assert_eq!(about.pending_restart.as_deref(), Some("9.0.0"), "the stale version line is flagged");
        assert!(updater.relaunch_exe().is_some());

        // "Later" keeps only the reminder; a new check does not undo the installed state.
        updater.defer();
        assert!(updater.run_info().expect("still there").collapsed);
        updater.status = Status::UpToDate;
        assert_eq!(updater.about(&config, None).state, AboutState::Installed { version: "9.0.0".into() });
    }

    #[test]
    fn a_dirty_tree_fails_with_the_files_and_can_be_retried() {
        let c = checkout(QUICK_CARGO);
        let config = config();
        let mut updater = updater_for(&c);
        std::fs::write(c.root.join("Cargo.toml"), b"[workspace]\n# edit\n").expect("write");
        updater.prepare_install().expect("asks first");
        updater.start_install(&config);
        finish(&mut updater);
        assert_eq!(updater.phase(), Phase::Failed);
        let failed = updater.run_info().expect("result");
        assert_eq!(failed.phase, Phase::Failed);
        let text = failed.lines.join("\n");
        assert!(text.contains(" M Cargo.toml"), "the dirty file is listed: {text}");
        assert!(text.contains("Nothing was changed"), "{text}");
        assert!(!failed.release_page, "a source checkout has no tarball to download");
        assert!(!c.root.join("feature.txt").exists());

        // Fixed by the user: "Try again" runs the confirmed update.
        git(&c.root, &["checkout", "--", "Cargo.toml"]);
        updater.retry(&config);
        assert_eq!(updater.phase(), Phase::Running);
        finish(&mut updater);
        assert_eq!(updater.phase(), Phase::Done);

        // Closing a failure goes back to idle.
        let c = checkout(QUICK_CARGO);
        let mut updater = updater_for(&c);
        std::fs::write(c.root.join("Cargo.toml"), b"x").expect("write");
        updater.prepare_install().expect("asks first");
        updater.start_install(&config);
        finish(&mut updater);
        updater.dismiss();
        assert_eq!(updater.phase(), Phase::Idle);
    }

    #[test]
    fn local_commits_are_explained() {
        let c = checkout(QUICK_CARGO);
        let config = config();
        std::fs::write(c.root.join("mine.txt"), b"x").expect("write");
        git(&c.root, &["add", "."]);
        git(&c.root, &["commit", "-m", "local"]);
        let mut updater = updater_for(&c);
        updater.prepare_install().expect("asks first");
        updater.start_install(&config);
        finish(&mut updater);
        let failed = updater.run_info().expect("result");
        let text = failed.lines.join("\n");
        assert!(text.contains("diverged") && text.contains("origin/main"), "{text}");
        assert!(text.contains("never"), "advice says Axion does not rewrite history: {text}");
    }

    #[test]
    fn cancelling_a_running_build_returns_to_idle() {
        let c = checkout("#!/bin/sh\necho building\nsleep 30\n");
        let config = config();
        let mut updater = updater_for(&c);
        updater.prepare_install().expect("asks first");
        updater.start_install(&config);
        let started = Instant::now();
        // Wait for the build to be running, then cancel it.
        loop {
            updater.poll();
            if updater.run_info().is_some_and(|r| r.detail == "building") {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(20), "the build never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        updater.cancel();
        finish(&mut updater);
        assert_eq!(updater.phase(), Phase::Idle);
        assert!(started.elapsed() < Duration::from_secs(25), "stopped instead of waiting for the build");
        assert!(updater.run_info().is_none());
    }

    #[test]
    fn a_read_only_tarball_installation_is_managed_elsewhere() {
        let dir = tempfile::tempdir().expect("tempdir");
        let install = dir.path().join("opt/axion");
        std::fs::create_dir_all(install.join("usr/bin")).expect("mkdir");
        std::fs::write(install.join("usr/bin/axion"), b"x").expect("write");
        std::fs::write(install.join("AppRun"), b"x").expect("write");
        let parent = dir.path().join("opt");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        let updater = Updater::with_kind(InstallKind::Tarball { dir: install.clone() });
        let about = updater.about(&Config::default(), None);
        let mode = updater.mode();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        if axion_update::tarball::check_writable(&install).is_ok() {
            return; // root can write anywhere
        }
        assert_eq!(mode, InstallMode::Other);
        assert_eq!(about.read_only.as_deref(), Some(install.to_str().expect("utf8")));

        let writable = Updater::with_kind(InstallKind::Tarball { dir: install });
        assert_eq!(writable.mode(), InstallMode::Tarball);
        assert!(writable.about(&Config::default(), None).read_only.is_none());
    }

    #[test]
    fn a_phase_change_is_reported_once() {
        let c = checkout(QUICK_CARGO);
        let config = config();
        let mut updater = updater_for(&c);
        updater.prepare_install().expect("asks first");
        updater.start_install(&config);
        // A second start while running changes nothing.
        updater.start_install(&config);
        assert_eq!(updater.phase(), Phase::Running);
        finish(&mut updater);
        assert!(updater.poll().is_empty(), "nothing more to report");
    }
}
