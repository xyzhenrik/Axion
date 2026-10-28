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
//! The entry is rewritten when it is missing or when it does not match this launcher, version
//! and integration revision (a stamp line in the file). The few small files are written before
//! the window is created (icons first, so nothing ever sees an entry whose icon is missing);
//! the slow part, refreshing the desktop and icon caches, runs on a thread of its own and can
//! only fail silently.
//!
//! KDE needs one more step: every KDE process (KWin for the title bar and Alt+Tab, plasmashell
//! for the task manager) remembers an icon name it once failed to find, and it never notices
//! theme folders that appeared after it started. On a desktop that ran before the icons were
//! there, `Icon=axion` then resolves to an empty icon until the session restarts. After writing
//! the icons Axion therefore sends KDE's "icons changed" signal (`iconChanged` of
//! `org.kde.KIconLoader` on `/KIconLoader`, what the icon settings send after a theme change),
//! which makes those processes drop their caches and update the open windows (see
//! docs/troubleshooting.md, "The window has no icon on Wayland").

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The desktop entry template (`Exec=` is replaced by the launcher).
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

/// Marks an entry as written by Axion (and for which revision, version and launcher).
const STAMP_KEY: &str = "# X-Axion-Integration:";

/// Revision of what [`install`] writes and does. Raising it rewrites every existing entry once
/// (and so sends the icon change signal once): revision 2 added that signal, for entries an
/// older Axion wrote while KDE kept an empty icon for them; revision 3 is the new logo, so
/// every installed copy of the old icon is replaced and the caches refreshed once.
const REVISION: u32 = 3;

/// Where KDE's icon loaders listen for [`notify_icons_changed`].
const ICON_LOADER_PATH: &str = "/KIconLoader";
const ICON_LOADER_INTERFACE: &str = "org.kde.KIconLoader";

/// Where the entry and the icons go, and what the entry launches.
#[derive(Clone, Debug)]
pub struct Target {
    /// `$XDG_DATA_HOME` (default `~/.local/share`).
    pub data_home: PathBuf,
    /// What the entry launches (see [`launcher`]).
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
        let exe = launcher(
            std::env::var_os("APPIMAGE"),
            std::env::var_os("AXION_LAUNCHER"),
            axion_update::install::current_exe(),
            Path::is_file,
        )?;
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

/// What `Exec=` launches, in this order: the AppImage file (`$APPIMAGE`: inside an AppImage the
/// executable lives in a temporary mount that is gone after Axion quits), the packaged launcher
/// (`$AXION_LAUNCHER`, exported by `tools/package/AppRun`: the AppImage or the launcher script),
/// else the running executable (`current_exe`, already without the ` (deleted)` Linux appends
/// once an update replaced the file). A variable only counts when it names an existing file by
/// an absolute path.
pub fn launcher(
    appimage: Option<OsString>,
    axion_launcher: Option<OsString>,
    current_exe: Option<PathBuf>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    [appimage, axion_launcher]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.is_absolute() && is_file(p))
        .or(current_exe)
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
            out.push_str(&format!("{STAMP_KEY} r{REVISION} {} {}\n", target.version, target.exe.display()));
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
            .all(|(path, bytes)| std::fs::read(path).is_ok_and(|have| have == *bytes))
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

/// Best-effort cache refreshes, each skipped when its tool is not installed: the GTK icon
/// cache, KDE's icon caches (the "icons changed" signal, see the module docs), the desktop
/// database and KDE's service cache (where Plasma's task manager finds the entry). The signal
/// goes out right away (KWin's title bar and Alt+Tab icon) and once more after the service
/// cache is rebuilt (the task manager, which looks the entry up again then).
fn refresh_caches_in_background(target: Target) {
    let spawned = std::thread::Builder::new().name("desktop-integration".into()).spawn(move || {
        let apps = target.applications();
        let hicolor = target.hicolor();
        run("gtk-update-icon-cache", &["-f".as_ref(), "-t".as_ref(), "-q".as_ref(), hicolor.as_os_str()]);
        notify_icons_changed();
        run("update-desktop-database", &[apps.as_os_str()]);
        if !run("kbuildsycoca6", &["--noincremental".as_ref()]) {
            run("kbuildsycoca5", &["--noincremental".as_ref()]);
        }
        notify_icons_changed();
    });
    if let Err(e) = spawned {
        tracing::debug!("desktop integration: no cache refresh thread: {e}");
    }
}

/// KDE's "icons changed" broadcast: every KDE process drops its icon caches, rereads the theme
/// folders and updates the icons of its windows. Harmless where nobody listens.
fn notify_icons_changed() {
    let sent = zbus::blocking::Connection::session().and_then(|bus| {
        bus.emit_signal(None::<&str>, ICON_LOADER_PATH, ICON_LOADER_INTERFACE, "iconChanged", &(0i32,))
    });
    if let Err(e) = sent {
        tracing::debug!("desktop integration: no icon change signal: {e}");
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
        assert!(text.contains("# X-Axion-Integration: r3 1.2.3 /opt/axion browser/axion\n"));
        // The template's own `Exec=axion %U` is gone, and there is exactly one.
        assert_eq!(text.matches("\nExec=").count(), 1);
        // The group header stays the first line, as the specification wants.
        assert!(text.starts_with("[Desktop Entry]\n"));
    }

    #[test]
    fn an_entry_of_an_older_revision_is_stale() {
        let dir = scratch("revision");
        let t = target(dir.clone(), "/usr/bin/axion");
        assert!(install(&t).expect("install"));
        // What revision 1 wrote: the same entry with the old stamp.
        let entry = dir.join("applications/axion.desktop");
        let old = std::fs::read_to_string(&entry)
            .expect("entry")
            .replace("# X-Axion-Integration: r3 1.2.3 ", "# X-Axion-Integration: 1.2.3 ");
        std::fs::write(&entry, old).expect("write");
        assert!(!is_up_to_date(&t));
        assert!(install(&t).expect("rewrite"));
        assert!(is_up_to_date(&t));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn launcher_prefers_the_appimage_then_the_packaged_launcher() {
        let exe = Some(PathBuf::from("/tmp/.mount_AxionX/usr/bin/axion"));
        let files = ["/home/u/Axion.AppImage", "/home/u/.local/opt/axion/AppRun"];
        let is_file = |p: &Path| files.iter().any(|f| Path::new(f) == p);
        let os = |s: &str| Some(OsString::from(s));
        let path = |s: &str| Some(PathBuf::from(s));
        // Inside an AppImage: the image file, never the temporary mount.
        assert_eq!(
            launcher(os("/home/u/Axion.AppImage"), os("/home/u/Axion.AppImage"), exe.clone(), is_file),
            path("/home/u/Axion.AppImage")
        );
        // The tarball or an installed copy: the launcher script AppRun exports.
        assert_eq!(
            launcher(None, os("/home/u/.local/opt/axion/AppRun"), exe.clone(), is_file),
            path("/home/u/.local/opt/axion/AppRun")
        );
        assert_eq!(
            launcher(os("/gone/Axion.AppImage"), os("/home/u/.local/opt/axion/AppRun"), exe.clone(), is_file),
            path("/home/u/.local/opt/axion/AppRun")
        );
        // Relative, missing or empty values do not count.
        assert_eq!(launcher(os("Axion.AppImage"), os(""), exe.clone(), is_file), exe);
        assert_eq!(launcher(os("/gone/Axion.AppImage"), None, exe.clone(), is_file), exe);
        // A plain build: the executable itself.
        assert_eq!(launcher(None, None, exe.clone(), is_file), exe);
        assert_eq!(launcher(None, None, None, is_file), None);
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
    fn an_old_icon_is_replaced() {
        let dir = scratch("old-icon");
        let t = target(dir.clone(), "/usr/bin/axion");
        assert!(install(&t).expect("install"));
        // An older Axion's icon (any other bytes, even of the same length) is stale and replaced.
        let path = dir.join("icons/hicolor/64x64/apps/axion.png");
        let mut old = std::fs::read(&path).expect("read");
        old[100] ^= 0xff;
        std::fs::write(&path, &old).expect("write");
        assert!(!is_up_to_date(&t));
        assert!(install(&t).expect("replace"));
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
