//! The "Import from another browser" group of Data & Downloads: the browsers found on this
//! machine (with their profiles and what they hold), what to import from them, the Import
//! button with its result, importing from a file, and the pointer to the Passwords page.

use axion_compositor::IconWeight;

use super::*;

/// Ids of the group's clicks: between the lock screen's ids (9000..9002) and the accounts sheet's (9100).
const IMPORT_BASE: u32 = 1020;
/// Browser profiles listed (more are not offered; the file import covers the rest).
pub const MAX_SOURCES: usize = 12;

/// A click in the group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportAction {
    /// Selects or deselects the browser profile at this index.
    Source(usize),
    Bookmarks,
    History,
    Run,
    File,
    /// "Import site icons".
    Icons,
}

impl ImportAction {
    pub fn id(self) -> u32 {
        IMPORT_BASE
            + match self {
                ImportAction::Bookmarks => 0,
                ImportAction::History => 1,
                ImportAction::Run => 2,
                ImportAction::File => 3,
                ImportAction::Icons => 4,
                ImportAction::Source(i) => 8 + i.min(MAX_SOURCES - 1) as u32,
            }
    }

    pub fn from_id(n: u32) -> Option<Self> {
        Some(match n.checked_sub(IMPORT_BASE)? {
            0 => ImportAction::Bookmarks,
            1 => ImportAction::History,
            2 => ImportAction::Run,
            3 => ImportAction::File,
            4 => ImportAction::Icons,
            i @ 8.. if ((i - 8) as usize) < MAX_SOURCES => ImportAction::Source((i - 8) as usize),
            _ => return None,
        })
    }
}

/// What the browser asks of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportRequest {
    /// Import these browser profiles (indexes into the listed ones).
    Run { sources: Vec<usize>, bookmarks: bool, history: bool, icons: bool },
    /// Pick a bookmarks file to import.
    ChooseFile,
}

/// A detected browser profile as the group lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSource {
    /// "Google Chrome · Person 1".
    pub label: String,
    pub bookmarks: Option<usize>,
    pub history: Option<usize>,
}

/// The result line under the group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportStatus {
    pub text: String,
    pub error: bool,
}

/// What the group shows and the choices made in it.
#[derive(Clone, Debug)]
pub struct ImportView {
    /// `None` while the browsers are being looked for.
    pub sources: Option<Vec<ImportSource>>,
    pub selected: Vec<bool>,
    pub bookmarks: bool,
    pub history: bool,
    /// Site icons come along (starts as `import.favicons`).
    pub icons: bool,
    /// An import is running.
    pub busy: bool,
    /// Without a history database nothing can be imported.
    pub available: bool,
    pub status: Option<ImportStatus>,
}

impl Default for ImportView {
    fn default() -> Self {
        Self {
            sources: None,
            selected: Vec::new(),
            bookmarks: true,
            history: true,
            icons: true,
            busy: false,
            available: true,
            status: None,
        }
    }
}

impl ImportView {
    /// Whether the Import button can be pressed.
    pub fn can_run(&self) -> bool {
        self.available && !self.busy && (self.bookmarks || self.history) && self.selected.iter().any(|&s| s)
    }
}

/// "3 bookmarks · 4 history entries" for what a profile holds.
pub fn describe(source: &ImportSource) -> String {
    let mut parts = Vec::new();
    if let Some(n) = source.bookmarks {
        parts.push(crate::import::bookmarks_text(n));
    }
    if let Some(n) = source.history {
        parts.push(crate::import::history_text(n));
    }
    parts.join(" · ")
}

impl SettingsSheet {
    /// The browsers found (or `None` while looking). The most recently used one is selected;
    /// a list that did not change keeps the choices made in it.
    pub fn set_import_sources(&mut self, sources: Option<Vec<ImportSource>>) {
        let same = match (&self.import.sources, &sources) {
            (Some(old), Some(new)) => old.iter().map(|s| &s.label).eq(new.iter().map(|s| &s.label)),
            _ => false,
        };
        if !same {
            self.import.selected =
                sources.as_ref().map(|s| (0..s.len()).map(|i| i == 0).collect()).unwrap_or_default();
        }
        self.import.sources = sources;
    }

    /// The Data page opened: the icons choice starts as the configuration says.
    pub fn set_import_icons_default(&mut self, icons: bool) {
        if !self.import.busy {
            self.import.icons = icons;
        }
    }

    pub fn set_import_busy(&mut self, busy: bool) {
        self.import.busy = busy;
    }

    pub fn set_import_available(&mut self, available: bool) {
        self.import.available = available;
    }

    pub fn set_import_status(&mut self, status: Option<ImportStatus>) {
        self.import.status = status;
    }

    pub(super) fn import_action(&mut self, action: ImportAction) -> Outcome {
        match action {
            ImportAction::Source(i) => {
                if let Some(s) = self.import.selected.get_mut(i) {
                    *s = !*s;
                }
            }
            ImportAction::Bookmarks => self.import.bookmarks = !self.import.bookmarks,
            ImportAction::History => self.import.history = !self.import.history,
            ImportAction::Icons => self.import.icons = !self.import.icons,
            ImportAction::File => return Outcome::Import(ImportRequest::ChooseFile),
            ImportAction::Run => {
                if self.import.can_run() {
                    let sources = self
                        .import
                        .selected
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| **s)
                        .map(|(i, _)| i)
                        .collect();
                    self.import.status = None;
                    return Outcome::Import(ImportRequest::Run {
                        sources,
                        bookmarks: self.import.bookmarks,
                        history: self.import.history,
                        icons: self.import.icons,
                    });
                }
            }
        }
        Outcome::Nothing
    }

    /// The group, in Data & Downloads.
    pub(super) fn import_group(&mut self, ui: &mut Ui, config: &Config) {
        let _ = config;
        self.section(ui, tr("Import from another browser"));
        match self.import.sources.clone() {
            None => {
                self.row(ui, tr("Looking for other browsers…"));
            }
            Some(sources) if sources.is_empty() => {
                self.row(ui, tr("No other browsers found"));
            }
            Some(sources) => {
                for (i, source) in sources.iter().take(MAX_SOURCES).enumerate() {
                    let selected = self.import.selected.get(i).copied().unwrap_or(false);
                    self.check_row(
                        ui,
                        ImportAction::Source(i),
                        &source.label,
                        Some(&describe(source)),
                        selected,
                    );
                }
                if sources.len() > MAX_SOURCES {
                    self.row(ui, &tr_fmt!("And {count} more.", count = sources.len() - MAX_SOURCES));
                }
                self.check_row(ui, ImportAction::Bookmarks, tr("Bookmarks"), None, self.import.bookmarks);
                self.check_row(ui, ImportAction::History, tr("History"), None, self.import.history);
                self.check_row(
                    ui,
                    ImportAction::Icons,
                    tr("Import site icons"),
                    Some(tr("So imported bookmarks and history show their icons at once")),
                    self.import.icons,
                );
                let row_node = self.row(ui, "");
                if self.import.busy {
                    idle_pill(ui, row_node, tr("Importing…"));
                } else if self.import.can_run() {
                    primary_button(ui, row_node, tr("Import"), SettingsAction::Import(ImportAction::Run));
                } else {
                    idle_pill(ui, row_node, tr("Import"));
                }
            }
        }
        if let Some(status) = self.import.status.clone() {
            // The result sits under the card, like a note, in the accent colour (red: failed).
            self.note(ui, &status.text);
            if let Some(&label) = ui.children(self.content).last()
                && let Some(w) = ui.widget_mut(label)
            {
                w.set_paint(if status.error { Paint::Fixed(DESTRUCTIVE) } else { Paint::Accent });
            }
        }
        let row_node = self.row(ui, tr("Bookmarks file (HTML or JSON)"));
        glass_pill(ui, row_node, tr("Import from File…"), SettingsAction::Import(ImportAction::File));
        self.note(
            ui,
            tr("Reads bookmarks and history without changing the other browser. Entries you already have are kept as they are."),
        );
        let row_node = self.row(ui, tr("Passwords"));
        glass_pill(ui, row_node, tr("Open Passwords"), SettingsAction::OpenPasswords);
        self.note(
            ui,
            tr("Passwords are encrypted by the other browser and can't be read from here. Export them there as a CSV file and import it in Passwords."),
        );
    }

    /// A row that selects or deselects something: a title (and detail) with a check circle.
    fn check_row(
        &mut self,
        ui: &mut Ui,
        action: ImportAction,
        title: &str,
        detail: Option<&str>,
        checked: bool,
    ) {
        let parent = self.group_for_row(ui);
        let node = ui.add(
            parent,
            Widget::container().with_action(SettingsAction::Import(action).id()).with_radius(10.0),
            Style {
                min_size: Size { width: auto(), height: length(ROW_HEIGHT) },
                padding: padding(0.0, 6.0),
                flex_shrink: 0.0,
                ..row(10.0)
            },
        );
        let text = ui.add(
            node,
            Widget::container(),
            Style {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_size: Size { width: length(0.0), height: auto() },
                ..Default::default()
            },
        );
        let label = ui.add_label(text, title, TextStyle::Body, grow_label());
        ui.set_label_lines(label, LABEL_LINES);
        if let Some(detail) = detail.filter(|d| !d.is_empty()) {
            let note = ui.add_label(text, detail, TextStyle::Footnote, grow_label());
            ui.set_label_lines(note, LABEL_LINES);
            if let Some(w) = ui.widget_mut(note) {
                w.set_paint(Paint::Secondary);
            }
        }
        let circle = ui.add(
            node,
            Widget::container().with_radius(CAPSULE).with_background(if checked {
                Background::Fill(Paint::Accent, 1.0)
            } else {
                Background::Fill(Paint::Label, 0.14)
            }),
            Style {
                size: Size { width: length(22.0), height: length(22.0) },
                flex_shrink: 0.0,
                justify_content: JustifyContent::CENTER,
                ..row(0.0)
            },
        );
        if checked {
            ui.add(
                circle,
                Widget::icon(Icon::Check, 14.0)
                    .with_paint(Paint::OnAccent)
                    .with_icon_weight(IconWeight::Bold),
                Style::default(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        let mut all = vec![
            ImportAction::Bookmarks,
            ImportAction::History,
            ImportAction::Run,
            ImportAction::File,
            ImportAction::Icons,
        ];
        all.extend((0..MAX_SOURCES).map(ImportAction::Source));
        for action in all {
            assert_eq!(ImportAction::from_id(action.id()), Some(action));
        }
        assert_eq!(ImportAction::from_id(IMPORT_BASE - 1), None);
        assert_eq!(ImportAction::from_id(IMPORT_BASE + 5), None);
        assert_eq!(ImportAction::from_id(IMPORT_BASE + 8 + MAX_SOURCES as u32), None);
    }

    #[test]
    fn running_needs_a_source_and_a_kind() {
        let mut view = ImportView { selected: vec![true, false], ..ImportView::default() };
        assert!(view.can_run());
        view.bookmarks = false;
        assert!(view.can_run());
        view.history = false;
        assert!(!view.can_run());
        view.history = true;
        view.selected = vec![false, false];
        assert!(!view.can_run());
        view.selected = vec![true];
        view.busy = true;
        assert!(!view.can_run());
        view.busy = false;
        view.available = false;
        assert!(!view.can_run());
    }

    #[test]
    fn profiles_are_described_by_their_counts() {
        let source = |bookmarks, history| ImportSource { label: String::new(), bookmarks, history };
        assert_eq!(describe(&source(Some(3), Some(1))), "3 bookmarks · 1 history entry");
        assert_eq!(describe(&source(Some(1), None)), "1 bookmark");
        assert_eq!(describe(&source(None, None)), "");
    }
}
