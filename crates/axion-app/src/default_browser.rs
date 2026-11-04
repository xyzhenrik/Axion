//! Axion as the system's default browser (Linux): what the desktop says the default is, and
//! making Axion it.
//!
//! The default browser is the `.desktop` entry that `xdg-settings get default-web-browser`
//! names; it is stored as the handler of `x-scheme-handler/http` and `/https` (and of HTML
//! files) in `mimeapps.list`. Axion's own entry (`axion.desktop`, written by
//! [`crate::desktop_integration`]) lists those types, which is what makes it eligible.
//!
//! Every command goes through a [`Runner`], so tests run against a mock and never touch the
//! real desktop settings. The real calls take a moment (and `xdg-settings` may start helper
//! processes), so [`Service`] runs them on a thread of their own and hands the answer back.

use std::ffi::OsStr;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::desktop_integration::{self, Target};

/// Axion's desktop entry, as the default browser setting names it.
pub const DESKTOP_ID: &str = "axion.desktop";
/// What a default browser handles.
const MIME_TYPES: [&str; 4] =
    ["x-scheme-handler/http", "x-scheme-handler/https", "text/html", "application/xhtml+xml"];

/// What a command printed, and whether it succeeded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
}

/// Runs the desktop's settings tools.
pub trait Runner: Send + Sync {
    /// `None` when the program cannot be started (not installed).
    fn run(&self, program: &str, args: &[&str]) -> Option<Output>;
}

/// The real thing.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Option<Output> {
        let out = Command::new(program)
            .args(args.iter().map(OsStr::new))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        Some(Output {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

/// Who is the default browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Axion.
    Axion,
    /// Another browser (its desktop entry, `firefox.desktop`), or none set (empty).
    Other(String),
    /// This system has no way to ask (no `xdg-settings`, or not Linux).
    Unavailable,
}

impl State {
    /// Whether a button that makes Axion the default makes sense.
    pub fn can_change(&self) -> bool {
        !matches!(self, State::Axion | State::Unavailable)
    }
}

/// A desktop entry id as a name to show: `firefox.desktop` and `org.mozilla.firefox.desktop`
/// both read "Firefox".
pub fn display_name(id: &str) -> String {
    let stem = id.strip_suffix(".desktop").unwrap_or(id);
    let name = stem.rsplit('.').next().unwrap_or(stem);
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Asks the desktop who the default browser is.
pub fn current(runner: &dyn Runner) -> State {
    if !cfg!(target_os = "linux") {
        return State::Unavailable;
    }
    let ask = |program: &str, args: &[&str]| {
        runner.run(program, args).filter(|o| o.success).map(|o| o.stdout.trim().to_owned())
    };
    let answer = ask("xdg-settings", &["get", "default-web-browser"])
        .or_else(|| ask("xdg-mime", &["query", "default", "x-scheme-handler/https"]));
    match answer {
        None => State::Unavailable,
        Some(id) if id == DESKTOP_ID => State::Axion,
        Some(id) => State::Other(id),
    }
}

/// Makes Axion the default browser: its desktop entry is written first if `target` says where
/// (so the setting names an entry that exists), then `xdg-settings`, else `xdg-mime`, and the
/// result is read back. The error is a short reason for the user.
pub fn make_default(runner: &dyn Runner, target: Option<&Target>) -> Result<(), String> {
    if let Some(target) = target
        && let Err(e) = desktop_integration::install(target)
    {
        return Err(format!("the desktop entry could not be written: {e}"));
    }
    let via_settings =
        runner.run("xdg-settings", &["set", "default-web-browser", DESKTOP_ID]).is_some_and(|o| o.success);
    if !via_settings || current(runner) != State::Axion {
        for mime in MIME_TYPES {
            // A failure of one type shows in the read-back below.
            runner.run("xdg-mime", &["default", DESKTOP_ID, mime]);
        }
    }
    match current(runner) {
        State::Axion => Ok(()),
        State::Unavailable => Err("this desktop has no tool to set the default browser".to_owned()),
        State::Other(other) if other.is_empty() => Err("the desktop did not take the setting".to_owned()),
        State::Other(other) => Err(format!("the desktop still names {other}")),
    }
}

/// A finished background job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// The state was asked for.
    State(State),
    /// Making Axion the default ended with this (the new state, or why not).
    Made(Result<State, String>),
}

/// Runs the commands off the main thread. The browser polls for answers once per pump.
pub struct Service {
    runner: Arc<dyn Runner>,
    /// Where the desktop entry goes (`None`: it is not written).
    target: Option<Target>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// A job is running: no second one starts.
    busy: bool,
}

impl Service {
    pub fn new(runner: Arc<dyn Runner>, target: Option<Target>) -> Self {
        let (tx, rx) = channel();
        Self { runner, target, tx, rx, busy: false }
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Asks who the default is.
    pub fn refresh(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        let (runner, tx) = (self.runner.clone(), self.tx.clone());
        self.spawn(move || {
            let _ = tx.send(Done::State(current(&*runner)));
        });
    }

    /// Makes Axion the default.
    pub fn make_default(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        let (runner, tx, target) = (self.runner.clone(), self.tx.clone(), self.target.clone());
        self.spawn(move || {
            let result = make_default(&*runner, target.as_ref()).map(|()| State::Axion);
            let _ = tx.send(Done::Made(result));
        });
    }

    fn spawn(&mut self, job: impl FnOnce() + Send + 'static) {
        if let Err(e) = std::thread::Builder::new().name("default-browser".into()).spawn(job) {
            tracing::warn!("default browser: no thread: {e}");
            self.busy = false;
        }
    }

    /// The next finished job, if any.
    pub fn poll(&mut self) -> Option<Done> {
        let done = self.rx.try_recv().ok()?;
        self.busy = false;
        Some(done)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;

    /// Answers like a desktop: `default` is what `xdg-settings get` returns; `set` changes it
    /// (unless `refuse`); `has_settings` false makes `xdg-settings` missing.
    struct Mock {
        default: Mutex<String>,
        has_settings: bool,
        has_mime: bool,
        refuse: bool,
        calls: Mutex<Vec<String>>,
    }

    impl Mock {
        fn new(default: &str) -> Self {
            Self {
                default: Mutex::new(default.to_owned()),
                has_settings: true,
                has_mime: true,
                refuse: false,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Runner for Mock {
        fn run(&self, program: &str, args: &[&str]) -> Option<Output> {
            self.calls.lock().expect("calls").push(format!("{program} {}", args.join(" ")));
            let ok = |stdout: String| Some(Output { success: true, stdout });
            match (program, args) {
                ("xdg-settings", ["get", ..]) if self.has_settings => {
                    ok(format!("{}\n", self.default.lock().expect("default")))
                }
                ("xdg-settings", ["set", _, id]) if self.has_settings => {
                    if self.refuse {
                        return Some(Output { success: false, stdout: String::new() });
                    }
                    *self.default.lock().expect("default") = (*id).to_owned();
                    ok(String::new())
                }
                ("xdg-mime", ["query", ..]) if self.has_mime => {
                    ok(format!("{}\n", self.default.lock().expect("default")))
                }
                ("xdg-mime", ["default", id, _]) if self.has_mime => {
                    if !self.refuse {
                        *self.default.lock().expect("default") = (*id).to_owned();
                    }
                    ok(String::new())
                }
                _ => None,
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axion-default-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn target(dir: &std::path::Path) -> Target {
        Target { data_home: dir.to_path_buf(), exe: PathBuf::from("/usr/bin/axion"), version: "1.0.0".into() }
    }

    #[test]
    fn ids_read_as_names() {
        assert_eq!(display_name("firefox.desktop"), "Firefox");
        assert_eq!(display_name("org.mozilla.firefox.desktop"), "Firefox");
        assert_eq!(display_name("brave-browser.desktop"), "Brave-browser");
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn the_state_names_who_is_default() {
        assert_eq!(current(&Mock::new("axion.desktop")), State::Axion);
        assert_eq!(current(&Mock::new("firefox.desktop")), State::Other("firefox.desktop".into()));
        assert_eq!(current(&Mock::new("")), State::Other(String::new()));
        let none = Mock { has_settings: false, has_mime: false, ..Mock::new("") };
        assert_eq!(current(&none), State::Unavailable);
        // Without xdg-settings, xdg-mime answers.
        let mime_only = Mock { has_settings: false, ..Mock::new("brave.desktop") };
        assert_eq!(current(&mime_only), State::Other("brave.desktop".into()));
        assert!(State::Other("x".into()).can_change());
        assert!(!State::Axion.can_change() && !State::Unavailable.can_change());
    }

    #[test]
    fn making_default_sets_it_through_xdg_settings_and_writes_the_entry_first() {
        let dir = scratch("set");
        let mock = Mock::new("firefox.desktop");
        make_default(&mock, Some(&target(&dir))).expect("made default");
        assert_eq!(*mock.default.lock().expect("default"), "axion.desktop");
        let calls = mock.calls.lock().expect("calls");
        assert_eq!(calls[0], "xdg-settings set default-web-browser axion.desktop");
        assert!(!calls.iter().any(|c| c.starts_with("xdg-mime default")), "settings was enough: {calls:?}");
        // The entry that the setting names exists, in the throwaway data directory only.
        let entry = std::fs::read_to_string(dir.join("applications/axion.desktop")).expect("entry");
        assert!(entry.contains("x-scheme-handler/https"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn xdg_mime_is_the_fallback_for_every_type() {
        let mock = Mock { has_settings: false, ..Mock::new("firefox.desktop") };
        make_default(&mock, None).expect("made default");
        let calls = mock.calls.lock().expect("calls");
        for mime in MIME_TYPES {
            assert!(calls.contains(&format!("xdg-mime default axion.desktop {mime}")), "{mime}: {calls:?}");
        }
    }

    #[test]
    fn a_refusal_is_reported_not_hidden() {
        let mock = Mock { refuse: true, ..Mock::new("firefox.desktop") };
        let err = make_default(&mock, None).expect_err("refused");
        assert!(err.contains("firefox.desktop"), "{err}");
        let none = Mock { has_settings: false, has_mime: false, ..Mock::new("") };
        assert!(make_default(&none, None).is_err());
    }

    #[test]
    fn the_service_answers_off_thread_and_runs_one_job_at_a_time() {
        let runner = Arc::new(Mock::new("firefox.desktop"));
        let mut service = Service::new(runner.clone(), None);
        service.refresh();
        assert!(service.is_busy());
        // No second job while one runs.
        service.make_default();
        let wait = |service: &mut Service| {
            for _ in 0..200 {
                if let Some(done) = service.poll() {
                    return done;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("no answer");
        };
        assert_eq!(wait(&mut service), Done::State(State::Other("firefox.desktop".into())));
        assert!(!service.is_busy());
        assert_eq!(
            *runner.default.lock().expect("default"),
            "firefox.desktop",
            "the second call was ignored"
        );
        service.make_default();
        assert_eq!(wait(&mut service), Done::Made(Ok(State::Axion)));
    }

    #[test]
    fn the_desktop_entry_declares_the_types_the_default_needs() {
        let entry = include_str!("../../../assets/axion.desktop");
        for mime in MIME_TYPES {
            assert!(entry.contains(mime), "axion.desktop lacks {mime}");
        }
        assert!(entry.contains("Exec=axion %U"), "links arrive as URLs");
    }
}
