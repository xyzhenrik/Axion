//! Per-user desktop integration (Linux): a `~/.local/share/applications/axion.desktop` for
//! this executable plus the icon set in `~/.local/share/icons/hicolor`.
//!
//! On Wayland a window has no icon of its own (winit cannot set one, and `xdg-toplevel-icon`
//! needs the `xdg_toplevel` that winit keeps to itself): the compositor looks the window's
//! app id (`axion`, see `browser.rs`) up as `axion.desktop` and takes its `Icon=` from the icon
//! theme. Running `target/release/axion` straight from a build never installed those, so Axion
//! installs them itself at start (`general.desktop_integration`), the same thing
//! `tools/install-desktop.sh` does by hand.
//!
//! The entry is rewritten when it is missing or when it does not match this executable and
//! version (a stamp line in the file). The few small files are written before the window is
//! created (so a compositor that watches the directory sees them as early as possible); the
//! slow part, refreshing the desktop and icon caches, runs on a thread of its own and can only
//! fail silently.

use std::path::{Path, PathBuf};

/// The desktop entry template (`Exec=` is replaced by the running executable).
const TEMPLATE: &str = include_str!("../../../assets/axion.desktop");

/// The icon set: (size directory, PNG bytes), and the scalable SVG.
const PNG_ICONS: [(&str, &[u8]); 8] = [
    ("16x16", include_bytes!("../../../assets/icons/hicolor/16x16/apps/axion.png")),
    ("24x24", include_bytes!("../../../assets/icons/hicolor/24x24/apps/axion.png")),
    ("32x32", include_bytes!("../../../assets/icons/hicolor/32x32/apps/axion.png")),
    ("48x48", include_bytes!("../../../assets/icons/hicolor/48x48/apps/axion.png")),
    ("64x64", include_bytes!("../../../assets/icons/hicolor/64x64/apps/axion.png")),
    ("128x128", include_bytes!("../../../assets/icons/hicolor/128x128/apps/axion.png")),
    ("256x256", include_bytes!("../../../assets/icons/hicolor/256x256/apps/axion.png")),
    ("512x512", include_bytes!("../../../assets/icons/hicolor/512x512/apps/axion.png")),
];
const SVG_ICON: &[u8] = include_bytes!("../../../assets/icons/hicolor/scalable/apps/axion.svg");

/// Marks an entry as written by Axion (and for which executable and version).
const STAMP_KEY: &str = "# X-Axion-Integration:";

/// Where the entry and the icons go, and what the entry launches.
#[derive(Clone, Debug)]
pub struct Target {
    /// `$XDG_DATA_HOME` (default `~/.local/share`).
    pub data_home: PathBuf,
    /// The running executable.
    pub exe: PathBuf,
    /// Axion's version (part of the stamp: an update rewrites the entry).
    pub version: String,
}

impl Target {
    /// The real per-user target, or `None` without a data directory or executable path.
    pub fn current() -> Option<Self> {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".local/share")))?;
        let exe = std::env::current_exe().ok()?;
        Some(Self { data_home, exe, version: env!("CARGO_PKG_VERSION").to_owned() })
    }

    fn applications(&self) -> PathBuf {
        self.data_home.join("applications")
    }

    pub fn desktop_file(&self) -> PathBuf {
        self.applications().join("axion.desktop")
    }

    fn hicolor(&self) -> PathBuf {
        self.data_home.join("icons/hicolor")
    }

    fn icon_files(&self) -> Vec<(PathBuf, &'static [u8])> {
        let hicolor = self.hicolor();
        let mut files: Vec<_> = PNG_ICONS
            .iter()
            .map(|(size, bytes)| (hicolor.join(size).join("apps/axion.png"), *bytes))
            .collect();
        files.push((hicolor.join("scalable/apps/axion.svg"), SVG_ICON));
        files
    }
}

/// `path` as the `Exec=` command: quoted, with the characters the Desktop Entry Specification
/// reserves escaped (`"`, `` ` ``, `$`, `\` by a backslash, which the file's string syntax then
/// doubles; `%` is a field code and doubles too).
pub fn exec_command(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for c in path.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            // A newline cannot be in an entry's value; such a path cannot be launched anyway.
            '\n' | '\r' => quoted.push('?'),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted.push_str(" %U");
    quoted
}

/// The text of `axion.desktop` for `target`: the template with `Exec=` pointing at the
/// executable, and a stamp line (a comment, which the specification allows anywhere).
pub fn render_desktop_entry(target: &Target) -> String {
    let exec = format!("Exec={}", exec_command(&target.exe));
    let mut out = String::new();
    let mut stamped = false;
    for line in TEMPLATE.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&exec);
        } else {
            out.push_str(line);
        }
        out.push('\n');
        if line == "[Desktop Entry]" && !stamped {
            out.push_str(&format!("{STAMP_KEY} {} {}\n", target.version, target.exe.display()));
            stamped = true;
        }
    }
    out
}

/// The entry and every icon are there and the entry is the one this executable and version
/// would write.
pub fn is_up_to_date(target: &Target) -> bool {
    let Ok(current) = std::fs::read_to_string(target.desktop_file()) else { return false };
    current == render_desktop_entry(target)
        && target
            .icon_files()
            .iter()
            .all(|(path, bytes)| std::fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64))
}

/// Writes `bytes` to `path` (through a temporary file, so a reader never sees half of it)
/// unless it already holds exactly that. Returns whether it wrote.
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|have| have == bytes) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("axion-tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)?;
    Ok(true)
}

/// Writes the entry and the icons. Returns whether anything changed (the caches need a
/// refresh then).
pub fn install(target: &Target) -> std::io::Result<bool> {
    let mut changed = false;
    for (path, bytes) in target.icon_files() {
        changed |= write_if_changed(&path, bytes)?;
    }
    // The entry last: a compositor that sees it finds the icons already there.
    changed |= write_if_changed(&target.desktop_file(), render_desktop_entry(target).as_bytes())?;
    Ok(changed)
}

/// What `config.general.desktop_integration` asks for at start: installs the entry and icons
/// if they are missing or stale, then refreshes the desktop caches on a thread of its own.
/// Never fails and never waits for the caches.
pub fn ensure(enabled: bool) {
    if !enabled || !cfg!(target_os = "linux") {
        return;
    }
    let Some(target) = Target::current() else { return };
    if is_up_to_date(&target) {
        return;
    }
    match install(&target) {
        Ok(true) => {
            tracing::info!(entry = %target.desktop_file().display(), "installed the desktop entry and icons");
            refresh_caches_in_background(target);
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("desktop integration: {e}"),
    }
}

/// Best-effort cache refreshes, each skipped when its tool is not installed: the desktop
/// database, the GTK icon cache and KDE's service cache (the one Plasma's task manager and
/// KWin match app ids against).
fn refresh_caches_in_background(target: Target) {
    let spawned = std::thread::Builder::new().name("desktop-integration".into()).spawn(move || {
        let apps = target.applications();
        let hicolor = target.hicolor();
        run("update-desktop-database", &[apps.as_os_str()]);
        run("gtk-update-icon-cache", &["-f".as_ref(), "-t".as_ref(), "-q".as_ref(), hicolor.as_os_str()]);
        if !run("kbuildsycoca6", &["--noincremental".as_ref()]) {
            run("kbuildsycoca5", &["--noincremental".as_ref()]);
        }
    });
    if let Err(e) = spawned {
        tracing::debug!("desktop integration: no cache refresh thread: {e}");
    }
}

/// Runs a tool quietly; `false` when it is missing or fails.
fn run(program: &str, args: &[&std::ffi::OsStr]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh throwaway data directory (never the user's real one).
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axion-desktop-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn target(data_home: PathBuf, exe: &str) -> Target {
        Target { data_home, exe: PathBuf::from(exe), version: "1.2.3".into() }
    }

    #[test]
    fn entry_launches_the_running_executable() {
        let text = render_desktop_entry(&target(PathBuf::from("/x"), "/opt/axion browser/axion"));
        assert!(text.contains("\nExec=\"/opt/axion browser/axion\" %U\n"), "{text}");
        assert!(text.contains("\nIcon=axion\n") && text.contains("\nStartupWMClass=axion\n"));
        assert!(text.contains("\nCategories=Network;WebBrowser;\n"));
        assert!(text.contains("x-scheme-handler/https"));
        assert!(text.contains("# X-Axion-Integration: 1.2.3 /opt/axion browser/axion\n"));
        // The template's own `Exec=axion %U` is gone, and there is exactly one.
        assert_eq!(text.matches("\nExec=").count(), 1);
        // The group header stays the first line, as the specification wants.
        assert!(text.starts_with("[Desktop Entry]\n"));
    }

    #[test]
    fn exec_escapes_what_the_specification_reserves() {
        assert_eq!(exec_command(Path::new("/a/b")), "\"/a/b\" %U");
        assert_eq!(exec_command(Path::new("/a\"b")), "\"/a\\\\\"b\" %U");
        assert_eq!(exec_command(Path::new("/a$b")), "\"/a\\\\$b\" %U");
        assert_eq!(exec_command(Path::new("/a\\b")), "\"/a\\\\\\\\b\" %U");
        assert_eq!(exec_command(Path::new("/100%/axion")), "\"/100%%/axion\" %U");
    }

    #[test]
    fn missing_or_stale_entries_are_not_up_to_date() {
        let dir = scratch("stale");
        let t = target(dir.clone(), "/home/u/axion/target/release/axion");
        assert!(!is_up_to_date(&t), "nothing installed yet");
        assert!(install(&t).expect("install"));
        assert!(is_up_to_date(&t));
        // Another executable path, or another version, rewrites the entry.
        let moved = target(dir.clone(), "/elsewhere/axion");
        assert!(!is_up_to_date(&moved));
        let newer = Target { version: "1.2.4".into(), ..t.clone() };
        assert!(!is_up_to_date(&newer));
        // A missing icon counts as stale.
        std::fs::remove_file(dir.join("icons/hicolor/48x48/apps/axion.png")).expect("remove");
        assert!(!is_up_to_date(&t));
        assert!(install(&t).expect("repair"));
        assert!(is_up_to_date(&t));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_writes_entry_and_every_icon_and_is_idempotent() {
        let dir = scratch("install");
        let t = target(dir.clone(), "/usr/bin/axion");
        assert!(install(&t).expect("install"));
        assert!(dir.join("applications/axion.desktop").is_file());
        for size in ["16x16", "24x24", "32x32", "48x48", "64x64", "128x128", "256x256", "512x512"] {
            assert!(dir.join(format!("icons/hicolor/{size}/apps/axion.png")).is_file(), "{size}");
        }
        assert!(dir.join("icons/hicolor/scalable/apps/axion.svg").is_file());
        assert!(!install(&t).expect("again"), "a second run changes nothing");
        // No temporary files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.join("applications"))
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "axion-tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn embedded_icons_are_pngs() {
        for (size, bytes) in PNG_ICONS {
            assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "{size}");
        }
    }
}
