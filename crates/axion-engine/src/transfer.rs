//! Data moving into pages: find-in-page queries, drops and pastes.

use std::path::PathBuf;

/// A find-in-page search (see [`crate::Engine::find`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindRequest {
    pub text: String,
    /// Search towards the end of the page (`false`: backwards).
    pub forward: bool,
    pub match_case: bool,
    /// Move to the next match of the running search instead of starting a new one.
    pub next: bool,
}

/// What is dragged onto a page from outside.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropData {
    /// Files, e.g. from the file manager.
    pub files: Vec<PathBuf>,
}

/// One file in a paste (an image copied from another app, or a copied file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastedFile {
    pub name: String,
    /// MIME type, e.g. `image/png`.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// What a paste delivers to the page (see [`crate::Engine::paste`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PasteData {
    pub text: Option<String>,
    pub html: Option<String>,
    pub files: Vec<PastedFile>,
}

impl PasteData {
    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty) && self.html.is_none() && self.files.is_empty()
    }
}
