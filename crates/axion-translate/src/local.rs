//! The local backend: jobs go to the engine page (`axion://translate/`, a view nobody sees)
//! as scripts, and its answers come back as console messages. The browser owns that view; this
//! type only keeps the queue and speaks the protocol.
//!
//! Page → browser: `"\u{1}axion:trh:" + JSON` ([`parse`]). Browser → page: JavaScript from
//! [`LocalTranslator::take_scripts`].

use std::collections::HashSet;

use serde::Deserialize;

use crate::{Job, Outcome, Translator, lang};

/// Console messages starting with this come from the engine page.
pub const MARKER: &str = "\u{1}axion:trh:";

/// What the engine page tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperMessage {
    /// The page is loaded and can take jobs.
    Ready,
    Done {
        tag: u64,
        texts: Vec<String>,
    },
    Error {
        tag: u64,
        reason: String,
    },
    /// A timing the engine measured (for the log).
    Log {
        what: String,
        ms: u64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Wire {
    Ready,
    Done {
        tag: u64,
        texts: Vec<String>,
    },
    Error {
        tag: u64,
        #[serde(default)]
        message: String,
    },
    Log {
        #[serde(default)]
        what: String,
        #[serde(default)]
        ms: u64,
    },
}

/// Parses a console message of the engine page (`None` for any other).
pub fn parse(console_message: &str) -> Option<HelperMessage> {
    let json = console_message.strip_prefix(MARKER)?;
    Some(match serde_json::from_str::<Wire>(json).ok()? {
        Wire::Ready => HelperMessage::Ready,
        Wire::Done { tag, texts } => HelperMessage::Done { tag, texts },
        Wire::Error { tag, message } => HelperMessage::Error { tag, reason: message },
        Wire::Log { what, ms } => HelperMessage::Log { what, ms },
    })
}

#[derive(Default)]
pub struct LocalTranslator {
    ready: bool,
    scripts: Vec<String>,
    waiting: HashSet<u64>,
    done: Vec<Outcome>,
    /// A warm-up is queued: the engine page is wanted although no job waits.
    warming: bool,
    workers: u32,
}

impl LocalTranslator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any job is waiting for the engine page (the browser then makes sure it exists).
    pub fn wants_helper(&self) -> bool {
        !self.waiting.is_empty() || self.warming
    }

    /// How many workers the engine page may use (0: it decides).
    pub fn set_workers(&mut self, workers: u32) {
        self.workers = workers;
    }

    /// Asks the engine page to start up and load the model of `from` → `to` now, so the first
    /// job doesn't wait for it.
    pub fn prewarm(&mut self, from: &str, to: &str) {
        let payload = serde_json::json!({ "from": from, "to": to, "workers": self.workers });
        self.warming = true;
        self.scripts.push(format!("window.__axionEngine && window.__axionEngine.prewarm({payload});"));
    }

    /// The scripts to run in the engine page now (none until it said it is ready).
    pub fn take_scripts(&mut self) -> Vec<String> {
        if self.ready {
            self.warming = false;
            std::mem::take(&mut self.scripts)
        } else {
            Vec::new()
        }
    }

    /// The engine page said something.
    pub fn on_message(&mut self, message: HelperMessage) {
        match message {
            HelperMessage::Ready => self.ready = true,
            HelperMessage::Log { what, ms } => tracing::info!(ms, "translation engine: {what}"),
            HelperMessage::Done { tag, texts } => {
                if self.waiting.remove(&tag) {
                    self.done.push(Outcome::Done { tag, texts });
                }
            }
            HelperMessage::Error { tag, reason } => {
                if self.waiting.remove(&tag) {
                    self.done.push(Outcome::Failed { tag, reason });
                }
            }
        }
    }

    /// The engine page is gone (closed or crashed): waiting jobs fail, the next job starts
    /// from a fresh page.
    pub fn helper_lost(&mut self) {
        self.ready = false;
        self.warming = false;
        self.scripts.clear();
        for tag in self.waiting.drain() {
            self.done.push(Outcome::Failed { tag, reason: "the translation engine stopped".into() });
        }
    }

    /// Drops the queued and running jobs of `tags` (their answers are ignored).
    pub fn forget(&mut self, tags: impl Fn(u64) -> bool) {
        self.waiting.retain(|t| !tags(*t));
    }
}

impl Translator for LocalTranslator {
    fn name(&self) -> &'static str {
        "local"
    }

    fn supports(&self, from: &str, to: &str) -> bool {
        lang::route(from, to).is_some()
    }

    fn submit(&mut self, job: Job) {
        let payload = serde_json::json!({
            "tag": job.tag, "from": job.from, "to": job.to, "texts": job.texts, "workers": self.workers,
        });
        self.waiting.insert(job.tag);
        self.scripts.push(format!("window.__axionEngine && window.__axionEngine.job({payload});"));
    }

    fn poll(&mut self) -> Vec<Outcome> {
        std::mem::take(&mut self.done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(tag: u64) -> Job {
        Job { tag, from: "de".into(), to: "en".into(), texts: vec!["Hallo \"Welt\"".into()] }
    }

    #[test]
    fn jobs_wait_for_the_engine_page() {
        let mut t = LocalTranslator::new();
        t.submit(job(1));
        assert!(t.wants_helper());
        assert!(t.take_scripts().is_empty());
        t.on_message(HelperMessage::Ready);
        let scripts = t.take_scripts();
        assert_eq!(scripts.len(), 1);
        assert!(scripts[0].contains(r#""tag":1"#) && scripts[0].contains(r#"Hallo \"Welt\""#));
        t.on_message(HelperMessage::Done { tag: 1, texts: vec!["Hello".into()] });
        assert_eq!(t.poll(), vec![Outcome::Done { tag: 1, texts: vec!["Hello".into()] }]);
        assert!(!t.wants_helper());
    }

    #[test]
    fn answers_to_forgotten_jobs_are_ignored() {
        let mut t = LocalTranslator::new();
        t.submit(job(7));
        t.forget(|tag| tag == 7);
        t.on_message(HelperMessage::Done { tag: 7, texts: vec![] });
        assert!(t.poll().is_empty());
    }

    #[test]
    fn a_lost_engine_fails_waiting_jobs() {
        let mut t = LocalTranslator::new();
        t.submit(job(3));
        t.helper_lost();
        assert!(matches!(t.poll().as_slice(), [Outcome::Failed { tag: 3, .. }]));
    }

    #[test]
    fn parses_helper_messages() {
        assert_eq!(parse(&format!(r#"{MARKER}{{"t":"ready"}}"#)), Some(HelperMessage::Ready));
        assert_eq!(
            parse(&format!(r#"{MARKER}{{"t":"done","tag":4,"texts":["a"]}}"#)),
            Some(HelperMessage::Done { tag: 4, texts: vec!["a".into()] })
        );
        assert_eq!(parse("\u{1}axion:{}"), None);
    }
}
