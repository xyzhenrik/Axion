/// A bookmark read from another browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportBookmark {
    pub url: String,
    pub title: String,
    /// The folders it was in, outermost first. Empty: the bookmarks bar (top level). The
    /// other roots of a browser appear as their name ("Other Bookmarks", "Bookmarks Menu").
    pub folders: Vec<String>,
    /// Unix seconds; 0 when unknown.
    pub added: i64,
}

impl ImportBookmark {
    /// "Dev / Rust" (empty for the top level).
    pub fn folder_path(&self) -> String {
        self.folders.join(" / ")
    }
}

/// A history entry read from another browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportHistory {
    pub url: String,
    pub title: String,
    pub visit_count: u32,
    /// Unix seconds.
    pub last_visit: i64,
}

/// What was read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub bookmarks: Vec<ImportBookmark>,
    /// Most recent first.
    pub history: Vec<ImportHistory>,
    /// Site icons for the hosts of the bookmarks and history above (see [`crate::write_icons`]).
    pub icons: Vec<crate::ImportIcon>,
    /// Parts that could not be read while others could.
    pub warnings: Vec<String>,
}

/// Which kinds of data to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub bookmarks: bool,
    pub history: bool,
}

impl Wanted {
    pub const ALL: Wanted = Wanted { bookmarks: true, history: true };
}

/// Caps that keep an import from eating memory or time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// At most this many history entries, the most recently visited first.
    pub max_history_items: usize,
    pub max_bookmarks: usize,
    /// Databases larger than this are not copied (bytes).
    pub max_db_bytes: u64,
    /// Bookmark files larger than this are refused (bytes).
    pub max_text_bytes: u64,
    /// Whether site icons are imported too.
    pub favicons: bool,
    /// An icon image larger than this is skipped (bytes).
    pub max_favicon_bytes: usize,
    /// At most this many site icons (one per host) are imported per profile or file.
    pub max_favicons: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_history_items: 20_000,
            max_bookmarks: 50_000,
            max_db_bytes: 512 * 1024 * 1024,
            max_text_bytes: 64 * 1024 * 1024,
            favicons: true,
            max_favicon_bytes: 256 * 1024,
            max_favicons: 10_000,
        }
    }
}
