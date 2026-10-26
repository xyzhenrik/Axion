//! Applying an update, by kind of installation:
//!
//! - **AppImage**: download the new image next to the old one, verify its checksum, swap it in
//!   atomically and keep the old one as `<name>.old`.
//! - **Source checkout** (`target/release/axion` inside a Git working tree): `git pull
//!   --ff-only` and `cargo build -p axion-app --release`, only on a clean tree whose remote is
//!   the configured repository. Never `--force`, never `reset`.
//! - **Anything else** (a distribution package): nothing to apply; the app shows the release
//!   notes and opens the release page.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use crate::UpdateError;
use crate::checksum::{expected_hash, verify_file};
use crate::http::Http;
use crate::release::Release;
use crate::source::Repository;

/// How this Axion was installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Running from an AppImage file at this path.
    AppImage { path: PathBuf },
    /// Built from a Git working tree at `root`.
    SourceCheckout { root: PathBuf },
    /// A package manager's or an unknown installation.
    Other,
}

/// Decides the kind from the `APPIMAGE` environment variable and the executable's path.
pub fn detect(appimage_env: Option<&OsStr>, exe: &Path) -> InstallKind {
    if let Some(path) = appimage_env.filter(|p| !p.is_empty()).map(PathBuf::from)
        && path.is_file()
    {
        return InstallKind::AppImage { path };
    }
    // `<root>/target/<profile>/axion`, with a `.git` in `<root>`.
    for root in exe.ancestors().skip(1) {
        let inside_target = exe
            .strip_prefix(root)
            .is_ok_and(|rel| rel.components().next().is_some_and(|c| c.as_os_str() == "target"));
        if inside_target && root.join(".git").exists() && root.join("Cargo.toml").is_file() {
            return InstallKind::SourceCheckout { root: root.to_path_buf() };
        }
    }
    InstallKind::Other
}

/// The path of the running executable, as it was when it started (Linux appends " (deleted)"
/// to `/proc/self/exe` once an update replaced the file).
pub fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let text = exe.to_string_lossy();
    Some(match text.strip_suffix(" (deleted)") {
        Some(stripped) => PathBuf::from(stripped),
        None => exe,
    })
}

/// The kind of the running installation.
pub fn detect_current() -> InstallKind {
    match current_exe() {
        Some(exe) => detect(std::env::var_os("APPIMAGE").as_deref(), &exe),
        None => InstallKind::Other,
    }
}

/// The phases the app reports ("Downloading…", "Verifying…").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Downloading,
    Verifying,
    Installing,
    CheckingTree,
    Pulling,
    Building,
}

/// Progress of an update, in the order it happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Step(Step),
    /// A line of a command's output.
    Line(String),
    Download {
        done: u64,
        total: Option<u64>,
    },
}

// ---- AppImage ----

/// What an AppImage update did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppImageInstalled {
    pub path: PathBuf,
    /// The previous image, kept.
    pub backup: PathBuf,
    /// Whether a published checksum was checked (otherwise the release had none).
    pub verified: bool,
}

/// The checksum a release publishes for `asset_name`: from its `SHA256SUMS` file, else from the
/// manifest. `Ok(None)` when it publishes none; a `SHA256SUMS` without an entry for the file
/// is an error (a wrong or tampered release).
pub fn published_checksum(
    release: &Release,
    asset_name: &str,
    http: &Http,
) -> Result<Option<String>, UpdateError> {
    if let Some(sums) = release.checksums_asset() {
        let text = http.get_text(&sums.url)?;
        return match expected_hash(&text, asset_name) {
            Some(hash) => Ok(Some(hash)),
            None => Err(UpdateError::Parse(format!("{} has no entry for {asset_name}", sums.name))),
        };
    }
    Ok(release.assets.iter().find(|a| a.name == asset_name).and_then(|a| a.sha256.clone()))
}

/// Downloads the release's AppImage for `arch`, verifies it and replaces `target` with it.
pub fn install_appimage(
    release: &Release,
    target: &Path,
    arch: &str,
    http: &Http,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<AppImageInstalled, UpdateError> {
    let asset = release.appimage_asset(arch).ok_or(UpdateError::NoAsset)?;
    let expected = published_checksum(release, &asset.name, http)?;

    // Next to the old image, so the final rename stays on one file system.
    let dir = target.parent().ok_or(UpdateError::NotSelfUpdating)?;
    let file_name = target.file_name().ok_or(UpdateError::NotSelfUpdating)?.to_string_lossy().into_owned();
    let partial = dir.join(format!(".{file_name}.axion-update"));

    progress(Progress::Step(Step::Downloading));
    http.download(&asset.url, &partial, cancel, &mut |done, total| {
        progress(Progress::Download { done, total });
    })?;

    let staged = (|| {
        progress(Progress::Step(Step::Verifying));
        if let Some(expected) = &expected {
            verify_file(&partial, expected)?;
        }
        ensure_elf(&partial)?;
        make_executable(&partial)?;
        progress(Progress::Step(Step::Installing));
        swap_in(&partial, target)
    })();
    if staged.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    let backup = staged?;
    Ok(AppImageInstalled { path: target.to_path_buf(), backup, verified: expected.is_some() })
}

/// An error page saved under the AppImage's name must never replace a working binary.
fn ensure_elf(path: &Path) -> Result<(), UpdateError> {
    let mut magic = [0u8; 4];
    let mut file = std::fs::File::open(path).map_err(|e| UpdateError::io_path("opening", path, e))?;
    file.read_exact(&mut magic).map_err(|e| UpdateError::io_path("reading", path, e))?;
    if magic == *b"\x7fELF" {
        Ok(())
    } else {
        Err(UpdateError::Parse("the download is not an executable AppImage".into()))
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| UpdateError::io_path("making executable", path, e))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), UpdateError> {
    Ok(())
}

/// Replaces `target` with `staged` in one rename; the old file stays as `<target>.old`.
/// Returns the backup's path.
pub fn swap_in(staged: &Path, target: &Path) -> Result<PathBuf, UpdateError> {
    let mut backup_name = target.as_os_str().to_owned();
    backup_name.push(".old");
    let backup = PathBuf::from(backup_name);
    if target.exists() {
        let _ = std::fs::remove_file(&backup);
        // A second name for the old file first, so `target` is never missing.
        if std::fs::hard_link(target, &backup).is_err() {
            std::fs::copy(target, &backup).map_err(|e| UpdateError::io_path("backing up", target, e))?;
        }
    }
    std::fs::rename(staged, target).map_err(|e| UpdateError::io_path("replacing", target, e))?;
    Ok(backup)
}

// ---- Source checkout ----

/// The programs the checkout update runs (replaceable for tests).
#[derive(Clone, Debug)]
pub struct Tools {
    pub git: PathBuf,
    pub cargo: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self { git: "git".into(), cargo: "cargo".into() }
    }
}

/// A checkout that may be updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub branch: String,
    pub remote: String,
    pub remote_url: String,
}

fn git_command(tools: &Tools, root: &Path) -> Command {
    let mut command = Command::new(&tools.git);
    command.arg("-C").arg(root).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
    command
}

/// Runs a command to completion and returns its trimmed stdout.
fn output(mut command: Command, label: &str) -> Result<String, UpdateError> {
    let out = command.output().map_err(|e| UpdateError::io(format!("running {label}"), e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let first = stderr.lines().next().unwrap_or("").trim();
        return Err(UpdateError::CommandFailed {
            command: label.to_owned(),
            status: if first.is_empty() { out.status.to_string() } else { first.to_owned() },
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Whether the working tree has no changes (tracked or untracked; ignored files do not count).
pub fn is_clean(root: &Path, tools: &Tools) -> Result<bool, UpdateError> {
    let mut command = git_command(tools, root);
    command.args(["status", "--porcelain"]);
    Ok(output(command, "git status --porcelain")?.is_empty())
}

/// Checks that `root` may be updated: a clean tree on a branch with an upstream whose remote is
/// the configured `repository`.
pub fn inspect_checkout(
    root: &Path,
    repository: &Repository,
    tools: &Tools,
) -> Result<Checkout, UpdateError> {
    if !is_clean(root, tools)? {
        return Err(UpdateError::DirtyTree);
    }
    let mut command = git_command(tools, root);
    command.args(["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = output(command, "git rev-parse")?;
    if branch.is_empty() || branch == "HEAD" {
        return Err(UpdateError::NoUpstream);
    }
    let mut command = git_command(tools, root);
    command.args(["config", "--get", &format!("branch.{branch}.remote")]);
    let remote = output(command, "git config").map_err(|_| UpdateError::NoUpstream)?;
    let mut command = git_command(tools, root);
    command.args(["remote", "get-url", &remote]);
    let remote_url = output(command, "git remote get-url")?;
    if !repository.matches_remote(&remote_url) {
        return Err(UpdateError::RemoteMismatch { remote: remote_url, repository: repository.to_string() });
    }
    Ok(Checkout { branch, remote, remote_url })
}

/// Runs `command`, passing each line of its output on as it appears. Fails on a non-zero exit.
fn run_streaming(
    mut command: Command,
    label: &str,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), UpdateError> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| UpdateError::io(format!("starting {label}"), e))?;
    let (sender, receiver) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, sender.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, sender.clone()));
    }
    drop(sender);
    for line in receiver {
        progress(Progress::Line(line));
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().map_err(|e| UpdateError::io(format!("waiting for {label}"), e))?;
    if status.success() {
        Ok(())
    } else {
        Err(UpdateError::CommandFailed { command: label.to_owned(), status: status.to_string() })
    }
}

fn spawn_reader(
    stream: impl Read + Send + 'static,
    sender: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            // Progress bars rewrite their line with carriage returns: keep the last state.
            let line = line.rsplit('\r').next().unwrap_or("").trim_end().to_owned();
            if !line.is_empty() && sender.send(line).is_err() {
                break;
            }
        }
    })
}

/// `git pull --ff-only` and `cargo build -p axion-app --release` in `root`, after
/// [`inspect_checkout`] passed. Stops at the first failure and leaves things as they are.
pub fn update_checkout(
    root: &Path,
    repository: &Repository,
    tools: &Tools,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), UpdateError> {
    progress(Progress::Step(Step::CheckingTree));
    inspect_checkout(root, repository, tools)?;

    progress(Progress::Step(Step::Pulling));
    let mut pull = git_command(tools, root);
    pull.args(["pull", "--ff-only"]);
    run_streaming(pull, "git pull --ff-only", progress)?;

    progress(Progress::Step(Step::Building));
    let mut build = Command::new(&tools.cargo);
    build
        .args(["build", "-p", "axion-app", "--release"])
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    run_streaming(build, "cargo build -p axion-app --release", progress)
}

#[cfg(test)]
mod tests;
