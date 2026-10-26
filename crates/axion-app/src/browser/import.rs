//! The browser's side of importing from other browsers: Settings › Data & Downloads and the
//! first-run introduction ask, `crate::import` works on threads, and this applies what
//! comes back to the interface (the sidebar lists, the bookmarks bar, the start page).

use super::*;
use crate::import::{self, Event, Finished};
use crate::onboarding::ImportChoice;
use crate::settings::{ImportAction, ImportRequest, ImportSource, ImportStatus};
use axion_import::Wanted;

impl Browser {
    /// The Data page opened: look for browsers again (the list is kept until the answer comes).
    pub(super) fn import_page_opened(&mut self) {
        self.settings.set_import_available(self.importer.available());
        self.importer.scan(&self.config);
    }

    /// A click in the import group.
    pub(super) fn import_request(&mut self, request: ImportRequest) {
        match request {
            ImportRequest::ChooseFile => self.importer.choose_file(),
            ImportRequest::Run { sources, bookmarks, history } => {
                let picked: Vec<_> =
                    sources.iter().filter_map(|&i| self.importer.sources().get(i).cloned()).collect();
                if picked.is_empty() || !(bookmarks || history) {
                    return;
                }
                self.settings.set_import_busy(true);
                self.importer.run(picked, Wanted { bookmarks, history }, &self.config);
            }
        }
    }

    /// The browsers the introduction offers, found without reading their databases.
    pub(super) fn onboarding_import_choices(&mut self) -> Vec<ImportChoice> {
        self.importer
            .offer()
            .iter()
            .map(|s| ImportChoice {
                label: s.label(),
                detail: match (s.has_bookmarks, s.has_history) {
                    (true, true) => tr("Bookmarks and history").to_owned(),
                    (true, false) => tr("Bookmarks").to_owned(),
                    _ => tr("History").to_owned(),
                },
                selected: s.primary,
            })
            .collect()
    }

    /// Capture aids (`--open=import:…`): clicks of the import group without a pointer.
    pub(super) fn import_capture(&mut self, what: &str) {
        let action = match what.split_once(':') {
            Some(("source", n)) => n.parse().ok().map(ImportAction::Source),
            Some(("file", path)) => {
                self.settings.set_import_status(None);
                self.settings.set_import_busy(true);
                self.importer.run_file(path.into(), &self.config);
                self.refresh_settings();
                return;
            }
            _ => match what {
                "bookmarks" => Some(ImportAction::Bookmarks),
                "history" => Some(ImportAction::History),
                "run" => Some(ImportAction::Run),
                _ => None,
            },
        };
        let Some(action) = action else {
            tracing::warn!("--open=import:{what} is not a known step");
            return;
        };
        let outcome = self.settings.action(
            &mut self.ui,
            SettingsAction::Import(action),
            &mut self.config,
            &self.keymap,
        );
        self.settings_outcome(outcome);
    }

    /// The introduction's import step was left with these browsers checked.
    pub(super) fn onboarding_import(&mut self, picks: Vec<usize>) {
        let sources: Vec<_> = picks.iter().filter_map(|&i| self.importer.offered().get(i).cloned()).collect();
        if sources.is_empty() || !self.importer.available() {
            return;
        }
        self.importer.run(sources, Wanted::ALL, &self.config);
    }

    /// Results of background work: the scan, a chosen file, a finished import. Call every frame.
    pub(super) fn import_pump(&mut self) {
        for event in self.importer.poll() {
            match event {
                Event::Scanned(sources) => {
                    let rows = sources
                        .iter()
                        .map(|s| ImportSource {
                            label: s.label(),
                            bookmarks: s.bookmark_count,
                            history: s.history_count,
                        })
                        .collect();
                    self.settings.set_import_sources(Some(rows));
                }
                Event::FileChosen(path) => {
                    self.settings.set_import_status(None);
                    self.settings.set_import_busy(true);
                    self.importer.run_file(path, &self.config);
                }
                Event::Finished(finished) => self.import_finished(finished),
            }
            self.refresh_settings();
        }
    }

    fn import_finished(&mut self, finished: Finished) {
        self.settings.set_import_busy(false);
        let status = match finished.result {
            Ok(report) => {
                let text = import::summary(&report, finished.wanted);
                tracing::info!(
                    bookmarks = report.bookmarks_added,
                    history = report.history_added,
                    existing = report.existing(),
                    "import finished"
                );
                if report.bookmarks_added + report.history_added > 0 {
                    self.import_refresh_views();
                    self.show_toast(Icon::Check, &import::toast_text(&report, finished.wanted));
                }
                ImportStatus { text, error: false }
            }
            Err(error) => {
                tracing::warn!("import failed: {error}");
                ImportStatus { text: tr_fmt!("Import failed: {error}", error = error), error: true }
            }
        };
        self.settings.set_import_status(Some(status));
    }

    /// New bookmarks and history are in the store: the lists, the bookmarks bar and the start
    /// page's top sites show them.
    fn import_refresh_views(&mut self) {
        self.bookmarks_changed = true;
        if let Some((panel, _)) = self.panel {
            self.show_panel_with(panel, true);
        }
        self.refresh_home_pages();
    }
}
