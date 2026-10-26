use std::path::Path;

/// Everything that can go wrong while looking for or applying an update.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("no update source is set (Settings › About › Repository)")]
    NoSource,
    #[error("{0:?} is not a repository: use owner/name")]
    InvalidRepository(String),
    #[error("{0:?} is not a valid address")]
    InvalidUrl(String),
    #[error("could not reach the update server: {0}")]
    Network(#[from] ureq::Error),
    #[error("the update server sent something unexpected: {0}")]
    Parse(String),
    #[error("{0:?} is not a version number")]
    Version(String),
    #[error("the release has no download for this installation")]
    NoAsset,
    #[error("the download is larger than the {limit_mb} MB allowed")]
    TooLarge { limit_mb: u32 },
    #[error("the download does not match its checksum (expected {expected}, got {actual})")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("{context}: {source}")]
    Io { context: String, source: std::io::Error },
    #[error("the update was cancelled")]
    Cancelled,
    #[error("the checkout has uncommitted changes; commit or stash them first")]
    DirtyTree,
    #[error("the checkout is on a detached HEAD or a branch without an upstream")]
    NoUpstream,
    #[error("the checkout's remote {remote} is not the configured repository {repository}")]
    RemoteMismatch { remote: String, repository: String },
    #[error("`{command}` failed ({status})")]
    CommandFailed { command: String, status: String },
    #[error("this installation cannot update itself")]
    NotSelfUpdating,
}

impl UpdateError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io { context: context.into(), source }
    }

    pub(crate) fn io_path(what: &str, path: &Path, source: std::io::Error) -> Self {
        Self::io(format!("{what} {}", path.display()), source)
    }
}
