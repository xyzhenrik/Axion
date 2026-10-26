use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path} is too large ({size} bytes, the limit is {limit})")]
    TooLarge { path: PathBuf, size: u64, limit: u64 },
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Format(String),
    #[error("store: {0}")]
    Store(#[from] axion_store::StoreError),
}

pub type Result<T> = std::result::Result<T, ImportError>;
