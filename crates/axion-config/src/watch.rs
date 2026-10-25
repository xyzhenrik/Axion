use std::sync::mpsc::{Receiver, channel};

use notify::{RecursiveMode, Watcher};

use crate::{ConfigError, ConfigPaths};

/// Which files changed since the last poll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub config: bool,
    pub keymap: bool,
}

/// Watches the configuration directory so edits apply without a restart.
pub struct ConfigWatcher {
    // Kept alive for as long as we want events.
    _watcher: notify::RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    paths: ConfigPaths,
}

impl ConfigWatcher {
    /// Starts watching. The directory is created if missing, since inotify needs it to exist.
    pub fn new(paths: &ConfigPaths) -> Result<Self, ConfigError> {
        std::fs::create_dir_all(&paths.dir)
            .map_err(|source| ConfigError::Write { path: paths.dir.clone(), source })?;
        let (tx, events) = channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            // The receiver only disappears on shutdown.
            let _ = tx.send(event);
        })?;
        // Watch the directory, not the files: editors save by replacing the file.
        watcher.watch(&paths.dir, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher, events, paths: paths.clone() })
    }

    /// Non-blocking: collects everything that happened since the last call.
    pub fn poll(&self) -> Changed {
        let mut changed = Changed::default();
        while let Ok(event) = self.events.try_recv() {
            let Ok(event) = event else { continue };
            if event.kind.is_access() {
                continue;
            }
            for path in &event.paths {
                changed.config |= path == &self.paths.config;
                changed.keymap |= path == &self.paths.keymap;
            }
        }
        changed
    }
}
