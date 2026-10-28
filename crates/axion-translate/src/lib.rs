//! Page translation.
//!
//! - [`Translator`]: what a translation backend looks like to the browser (queue jobs, poll
//!   results), so backends can be swapped.
//! - [`local::LocalTranslator`]: Mozilla's Bergamot engine (WebAssembly, MPL-2.0) and its
//!   models, running in a hidden `axion://translate/` page of the browser. Nothing leaves the
//!   computer; the models are downloaded once, with the user's consent ([`models`]).
//! - [`endpoint::EndpointTranslator`]: an optional, user-configured LibreTranslate server.
//!   Off by default; page text is sent to it.
//! - [`page`]: the script that finds the text of a page, puts the translation back and
//!   restores the original, and its protocol.

pub mod endpoint;
pub mod helper;
pub mod lang;
pub mod local;
pub mod models;
pub mod page;

/// One request to translate pieces of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// Whatever the caller needs to recognise the answer by.
    pub tag: u64,
    /// Language codes (see [`lang`]).
    pub from: String,
    pub to: String,
    pub texts: Vec<String>,
}

/// The answer to a [`Job`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// One translation per text of the job, in order.
    Done { tag: u64, texts: Vec<String> },
    Failed { tag: u64, reason: String },
}

impl Outcome {
    pub fn tag(&self) -> u64 {
        match self {
            Outcome::Done { tag, .. } | Outcome::Failed { tag, .. } => *tag,
        }
    }
}

/// A translation backend.
pub trait Translator {
    /// Short name for logs.
    fn name(&self) -> &'static str;
    /// Whether the backend can translate `from` → `to` at all (it may still have to download
    /// something first).
    fn supports(&self, from: &str, to: &str) -> bool;
    /// Queues a job; the answer comes from [`Translator::poll`].
    fn submit(&mut self, job: Job);
    /// The answers that arrived since the last call.
    fn poll(&mut self) -> Vec<Outcome>;
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("this language pair can't be translated")]
    Unsupported,
    #[error("no model for {from} → {to} in the list")]
    NoModel { from: String, to: String },
    #[error("the model list is unusable: {0}")]
    Catalog(String),
    #[error("could not reach the server: {0}")]
    Network(#[from] ureq::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the downloaded file {0} does not match its checksum")]
    Corrupt(String),
    #[error("cancelled")]
    Cancelled,
}
