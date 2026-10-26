//! Finding the other browsers' profiles on this machine (read-only).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::model::Limits;
use crate::{chromium, firefox};

/// The user's home and config directories. Injected, so tests and captures can point the
/// detection at a fake one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Home {
    home: PathBuf,
    config: PathBuf,
}

impl Home {
    /// A home directory whose config directory is `<home>/.config`.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let config = home.join(".config");
        Self { home, config }
    }

    pub fn with_config(home: impl Into<PathBuf>, config: impl Into<PathBuf>) -> Self {
        Self { home: home.into(), config: config.into() }
    }

    /// The real one: `$AXION_IMPORT_HOME` (a fake home for tests and captures) if set, else
    /// `$HOME` with `$XDG_CONFIG_HOME` (when absolute) or `~/.config`.
    pub fn from_env() -> Option<Self> {
        if let Some(fake) = std::env::var_os("AXION_IMPORT_HOME").filter(|v| !v.is_empty()) {
            return Some(Self::new(fake));
        }
        let home = PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Some(Self { home, config })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn config(&self) -> &Path {
        &self.config
    }
}

/// Which kind of data layout a browser has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

/// One profile of one installed browser that has something to import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Stable between runs (browser, install kind, profile directory).
    pub id: String,
    /// "Google Chrome", "Firefox", …
    pub browser: String,
    /// The profile's name ("Default", "Person 1", "default-release").
    pub profile: String,
    pub family: Family,
    /// The profile directory.
    pub dir: PathBuf,
    pub has_bookmarks: bool,
    pub has_history: bool,
    /// How many bookmarks and history entries it holds (filled by [`scan`]; `None` unknown).
    pub bookmark_count: Option<usize>,
    pub history_count: Option<usize>,
    /// When the profile's data was last written (unix seconds).
    pub modified: i64,
    /// The most recently used source: the one to preselect.
    pub primary: bool,
}

impl Source {
    /// "Google Chrome · Default".
    pub fn label(&self) -> String {
        if self.profile.is_empty() {
            self.browser.clone()
        } else {
            format!("{} · {}", self.browser, self.profile)
        }
    }
}

/// Where a browser keeps its data, relative to the home or the config directory.
#[derive(Clone, Copy)]
enum Loc {
    Config(&'static str),
    Home(&'static str),
}

struct Def {
    name: &'static str,
    family: Family,
    /// Native, Flatpak and Snap locations.
    locations: &'static [Loc],
}

const BROWSERS: &[Def] = &[
    Def {
        name: "Google Chrome",
        family: Family::Chromium,
        locations: &[
            Loc::Config("google-chrome"),
            Loc::Home(".var/app/com.google.Chrome/config/google-chrome"),
        ],
    },
    Def {
        name: "Google Chrome Beta",
        family: Family::Chromium,
        locations: &[Loc::Config("google-chrome-beta")],
    },
    Def {
        name: "Chromium",
        family: Family::Chromium,
        locations: &[
            Loc::Config("chromium"),
            Loc::Home(".var/app/org.chromium.Chromium/config/chromium"),
            Loc::Home("snap/chromium/common/chromium"),
        ],
    },
    Def {
        name: "Brave",
        family: Family::Chromium,
        locations: &[
            Loc::Config("BraveSoftware/Brave-Browser"),
            Loc::Home(".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser"),
            Loc::Home("snap/brave/current/.config/BraveSoftware/Brave-Browser"),
        ],
    },
    Def {
        name: "Microsoft Edge",
        family: Family::Chromium,
        locations: &[
            Loc::Config("microsoft-edge"),
            Loc::Home(".var/app/com.microsoft.Edge/config/microsoft-edge"),
        ],
    },
    Def {
        name: "Vivaldi",
        family: Family::Chromium,
        locations: &[Loc::Config("vivaldi"), Loc::Home(".var/app/com.vivaldi.Vivaldi/config/vivaldi")],
    },
    Def {
        name: "Opera",
        family: Family::Chromium,
        locations: &[
            Loc::Config("opera"),
            Loc::Home(".var/app/com.opera.Opera/config/opera"),
            Loc::Home("snap/opera/current/.config/opera"),
        ],
    },
    Def {
        name: "Firefox",
        family: Family::Firefox,
        locations: &[
            Loc::Home(".mozilla/firefox"),
            Loc::Config("mozilla/firefox"),
            Loc::Home(".var/app/org.mozilla.firefox/.mozilla/firefox"),
            Loc::Home("snap/firefox/common/.mozilla/firefox"),
        ],
    },
    Def {
        name: "LibreWolf",
        family: Family::Firefox,
        locations: &[
            Loc::Home(".librewolf"),
            Loc::Config("librewolf"),
            Loc::Home(".var/app/io.gitlab.librewolf-community/.librewolf"),
        ],
    },
    Def {
        name: "Zen",
        family: Family::Firefox,
        locations: &[Loc::Home(".zen"), Loc::Config("zen"), Loc::Home(".var/app/app.zen_browser.zen/.zen")],
    },
    Def {
        name: "Waterfox",
        family: Family::Firefox,
        locations: &[Loc::Home(".waterfox"), Loc::Home(".var/app/net.waterfox.waterfox/.waterfox")],
    },
    Def { name: "Floorp", family: Family::Firefox, locations: &[Loc::Home(".floorp")] },
];

/// The profiles of every installed browser that has bookmarks or history, without counts.
/// Cheap (only looks at the file system); most recently used first, which is marked primary.
pub fn detect(home: &Home) -> Vec<Source> {
    let mut found: Vec<Source> = Vec::new();
    for def in BROWSERS {
        for (n, loc) in def.locations.iter().enumerate() {
            let base = match loc {
                Loc::Config(rel) => home.config.join(rel),
                Loc::Home(rel) => home.home.join(rel),
            };
            if !base.is_dir() {
                continue;
            }
            let kind = if base.starts_with(home.home.join(".var")) {
                "flatpak"
            } else if base.starts_with(home.home.join("snap")) {
                "snap"
            } else {
                "native"
            };
            let profiles: Vec<(String, String, PathBuf)> = match def.family {
                Family::Chromium => chromium::profiles(&base)
                    .into_iter()
                    .map(|(dir, name)| {
                        let path = base.join(&dir);
                        (if dir.is_empty() { "Default".to_owned() } else { dir }, name, path)
                    })
                    .collect(),
                Family::Firefox => firefox::profiles(&base)
                    .into_iter()
                    .map(|p| {
                        let dir_name =
                            p.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        (dir_name, p.name, p.dir)
                    })
                    .collect(),
            };
            for (dir_name, profile, dir) in profiles {
                let (has_bookmarks, has_history) = match def.family {
                    Family::Chromium => (dir.join("Bookmarks").is_file(), dir.join("History").is_file()),
                    Family::Firefox => {
                        let places = dir.join("places.sqlite").is_file();
                        (places, places)
                    }
                };
                if !has_bookmarks && !has_history {
                    continue;
                }
                let id = format!("{}:{kind}{n}:{dir_name}", def.name.to_ascii_lowercase().replace(' ', "-"));
                if found.iter().any(|s| s.id == id) {
                    continue;
                }
                found.push(Source {
                    id,
                    browser: def.name.to_owned(),
                    profile,
                    family: def.family,
                    modified: modified(&dir, def.family),
                    dir,
                    has_bookmarks,
                    has_history,
                    bookmark_count: None,
                    history_count: None,
                    primary: false,
                });
            }
        }
    }
    found.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
    if let Some(first) = found.first_mut() {
        first.primary = true;
    }
    found
}

/// [`detect`], then counts what each source holds (this reads the databases: run it off the
/// interface thread). A source whose data cannot be read keeps `None` counts.
pub fn scan(home: &Home, limits: &Limits) -> Vec<Source> {
    let mut sources = detect(home);
    for source in &mut sources {
        match source.family {
            Family::Chromium => {
                if source.has_bookmarks {
                    source.bookmark_count =
                        chromium::read_bookmarks(&source.dir, limits).ok().map(|b| b.len());
                }
                if source.has_history {
                    source.history_count = chromium::count_history(&source.dir, limits).ok();
                }
            }
            Family::Firefox => {
                if let Ok((bookmarks, history)) = firefox::count(&source.dir, limits) {
                    source.bookmark_count = Some(bookmarks);
                    source.history_count = Some(history);
                }
            }
        }
    }
    // A profile with nothing to import is of no use.
    sources.retain(|s| s.bookmark_count.unwrap_or(1) > 0 || s.history_count.unwrap_or(1) > 0);
    if !sources.iter().any(|s| s.primary)
        && let Some(first) = sources.first_mut()
    {
        first.primary = true;
    }
    sources
}

/// The newest write time of the profile's data files (unix seconds, 0 if unknown).
fn modified(dir: &Path, family: Family) -> i64 {
    let files: &[&str] = match family {
        Family::Chromium => &["History", "Bookmarks"],
        Family::Firefox => &["places.sqlite", "places.sqlite-wal"],
    };
    files
        .iter()
        .filter_map(|f| std::fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok())
        .filter_map(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .max()
        .unwrap_or(0)
}
