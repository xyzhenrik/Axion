//! The address bar's database lookups on a thread of their own: the suggestions under the field
//! (bookmarks and history matching what was typed) and the inline completion. Both scan the
//! history table; with tens of thousands of entries that took ~20 ms per key on the UI thread
//! (a dropped frame or two at every key).
//!
//! The worker owns a second, read-only use of the profile's database (SQLite in WAL mode reads
//! beside the main connection). Of several lookups waiting at once only the newest runs: typing
//! fast never builds a queue. Answers come back through [`SuggestWorker::poll`]; the event loop
//! is woken when one is ready.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use axion_engine::cef::Waker;
use axion_store::Store;

use crate::suggestions::{Suggestion, suggest};

/// A lookup for what is typed in the address field.
pub struct Request {
    /// Tells answers apart; only the newest request's answer is used.
    pub generation: u64,
    pub text: String,
    pub search_url: String,
    pub now: i64,
    /// Also find the inline completion (the user typed at the end of the field).
    pub complete: bool,
}

pub struct Answer {
    pub generation: u64,
    pub text: String,
    pub items: Vec<Suggestion>,
    pub completion: Option<String>,
}

/// The thread's handle. Dropping it ends the thread.
pub struct SuggestWorker {
    requests: Sender<Request>,
    answers: Receiver<Answer>,
}

impl SuggestWorker {
    /// Starts the worker on the database at `path`; `busy_timeout` is how long it waits for a
    /// lock of the main connection.
    pub fn spawn(path: PathBuf, busy_timeout: Duration, waker: Option<Waker>) -> Self {
        let (requests, queue) = channel::<Request>();
        let (report, answers) = channel::<Answer>();
        let spawned = std::thread::Builder::new().name("suggest-worker".into()).spawn(move || {
            let store = match Store::open(&path) {
                Ok(store) => store,
                Err(e) => {
                    tracing::error!("the address suggestions cannot open the database: {e}");
                    return;
                }
            };
            if let Err(e) = store.set_busy_timeout(busy_timeout) {
                tracing::warn!("setting the suggestion worker's database timeout failed: {e}");
            }
            while let Ok(first) = queue.recv() {
                // Only the newest of what is waiting.
                let request = queue.try_iter().last().unwrap_or(first);
                let items = suggest(&request.text, Some(&store), &request.search_url, request.now);
                let completion = if request.complete {
                    store
                        .autocomplete(&request.text)
                        .inspect_err(|e| tracing::warn!("address completion failed: {e}"))
                        .ok()
                        .flatten()
                } else {
                    None
                };
                let answer = Answer { generation: request.generation, text: request.text, items, completion };
                if report.send(answer).is_err() {
                    return;
                }
                if let Some(waker) = &waker {
                    waker.wake();
                }
            }
        });
        if let Err(e) = spawned {
            tracing::error!("starting the suggestion worker failed: {e}");
        }
        Self { requests, answers }
    }

    /// Asks for a lookup. Returns `false` when the worker is gone (it could not start or open
    /// the database): the caller looks up itself then.
    pub fn send(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }

    /// The answers that arrived since the last call.
    pub fn poll(&self) -> Vec<Answer> {
        self.answers.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(worker: &SuggestWorker) -> Answer {
        for _ in 0..500 {
            if let Some(answer) = worker.poll().pop() {
                return answer;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no answer from the suggestion worker");
    }

    #[test]
    fn suggestions_and_completion_come_from_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("axion.sqlite");
        let store = Store::open(&path).unwrap();
        store.record_visit("https://music.example.org/top", "Music", 1_000).unwrap();
        let worker = SuggestWorker::spawn(path, Duration::from_millis(500), None);
        assert!(worker.send(Request {
            generation: 3,
            text: "musi".into(),
            search_url: "https://search.example/?q={}".into(),
            now: 2_000,
            complete: true,
        }));
        let answer = wait_for(&worker);
        assert_eq!(answer.generation, 3);
        assert_eq!(answer.text, "musi");
        assert!(answer.items.iter().any(|s| s.url == "https://music.example.org/top"));
        assert_eq!(answer.completion.as_deref(), Some("music.example.org"));
    }
}
