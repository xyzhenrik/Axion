//! The History sheet's database work on a thread of its own: reading the (possibly huge) list,
//! searching it, and deleting entries never block the window's frames.
//!
//! The worker owns a second connection to the profile's database (SQLite in WAL mode lets it
//! read and write beside the main one) and handles jobs in order, so a load that follows a
//! delete sees the deletion. Of several loads waiting at once only the newest runs. Results come
//! back through [`ListWorker::poll`]; the event loop is woken when one is ready.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use axion_engine::cef::Waker;
use axion_store::Store;

use crate::chrome::PanelItem;

/// Work for the worker thread.
pub enum Job {
    /// The history entries matching `query` (all when empty), newest first, as sheet rows.
    Load { generation: u64, query: String, limit: usize, now: i64 },
    /// Deletes the history entries of these addresses.
    DeleteHistory(Vec<String>),
    /// Deletes every history entry matching `query` (all when empty) except the addresses in
    /// `keep`, in one transaction ("Select All" in the History sheet: the rows beyond the loaded
    /// ones are deleted too, without ever being listed).
    DeleteHistoryMatching { query: String, keep: Vec<String> },
    /// Deletes all of the history.
    ClearHistory,
}

/// What the worker reports.
pub enum Done {
    /// `total` is how many entries match in the database (more than `items` when the list
    /// is capped by `history.sheet_max_entries`).
    Loaded { generation: u64, items: Vec<PanelItem>, total: usize, took: Duration },
}

/// The thread's handle. Dropping it ends the thread once its queue is empty.
pub struct ListWorker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl ListWorker {
    /// Starts the worker on the database at `path`; `busy_timeout` is how long it waits for a
    /// lock of the main connection.
    pub fn spawn(path: PathBuf, busy_timeout: Duration, waker: Option<Waker>) -> Self {
        let (jobs, queue) = channel::<Job>();
        let (report, done) = channel::<Done>();
        let spawned = std::thread::Builder::new().name("list-worker".into()).spawn(move || {
            let store = match Store::open(&path) {
                Ok(store) => store,
                Err(e) => {
                    tracing::error!("the list worker cannot open the database: {e}");
                    return;
                }
            };
            if let Err(e) = store.set_busy_timeout(busy_timeout) {
                tracing::warn!("setting the list worker's database timeout failed: {e}");
            }
            while let Ok(first) = queue.recv() {
                // Everything that is waiting now: deletes in order, only the newest load.
                let mut pending = vec![first];
                pending.extend(queue.try_iter());
                let newest_load = pending.iter().rposition(|j| matches!(j, Job::Load { .. }));
                for (i, job) in pending.into_iter().enumerate() {
                    match job {
                        Job::DeleteHistory(urls) => {
                            if let Err(e) = store.delete_history_urls(&urls) {
                                tracing::error!("deleting history entries failed: {e}");
                            }
                        }
                        Job::DeleteHistoryMatching { query, keep } => {
                            if let Err(e) = store.delete_history_matching(&query, &keep) {
                                tracing::error!("deleting the matching history entries failed: {e}");
                            }
                        }
                        Job::ClearHistory => {
                            if let Err(e) = store.clear_history_since(0) {
                                tracing::error!("clearing history failed: {e}");
                            }
                        }
                        Job::Load { generation, query, limit, now } if Some(i) == newest_load => {
                            let started = Instant::now();
                            let entries = store
                                .search_history_recent(&query, limit)
                                .inspect_err(|e| tracing::error!("reading history failed: {e}"))
                                .unwrap_or_default();
                            let total = store
                                .count_history_matching(&query)
                                .inspect_err(|e| tracing::error!("counting history failed: {e}"))
                                .unwrap_or(entries.len())
                                .max(entries.len());
                            let items = crate::history::panel_items(entries, now);
                            let took = started.elapsed();
                            if report.send(Done::Loaded { generation, items, total, took }).is_err() {
                                return;
                            }
                            if let Some(waker) = &waker {
                                waker.wake();
                            }
                        }
                        Job::Load { .. } => {}
                    }
                }
            }
        });
        if let Err(e) = spawned {
            tracing::error!("starting the list worker failed: {e}");
        }
        Self { jobs, done }
    }

    pub fn send(&self, job: Job) {
        // A worker that could not start has dropped its queue: the job is lost, the list stays as it is.
        let _ = self.jobs.send(job);
    }

    /// The results that arrived since the last call.
    pub fn poll(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(worker: &ListWorker) -> Done {
        for _ in 0..500 {
            if let Some(done) = worker.poll().pop() {
                return done;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the worker did not answer");
    }

    #[test]
    fn loads_deletes_in_order_and_answers_only_the_newest_load() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("axion.sqlite");
        let store = Store::open(&path).expect("store");
        for i in 0..5 {
            store.record_visit(&format!("https://site{i}.example/"), "Site", 1_000 + i).expect("visit");
        }
        let worker = ListWorker::spawn(path, Duration::from_millis(500), None);
        worker
            .send(Job::DeleteHistory(vec!["https://site1.example/".into(), "https://site3.example/".into()]));
        worker.send(Job::Load { generation: 1, query: String::new(), limit: 100, now: 2_000 });
        worker.send(Job::Load { generation: 2, query: "site".into(), limit: 100, now: 2_000 });
        let Done::Loaded { generation, items, .. } = wait_for(&worker);
        // The older load may have run before the second arrived; the newest always does.
        let (generation, items) = if generation == 1 {
            let Done::Loaded { generation, items, .. } = wait_for(&worker);
            (generation, items)
        } else {
            (generation, items)
        };
        assert_eq!(generation, 2);
        assert_eq!(items.len(), 3, "the two deleted entries are gone");
        // Delete everything matching except one address: one job, one transaction.
        worker.send(Job::DeleteHistoryMatching {
            query: "site".into(),
            keep: vec!["https://site4.example/".into(), "https://site0.example/".into()],
        });
        worker.send(Job::Load { generation: 3, query: String::new(), limit: 1, now: 2_000 });
        let Done::Loaded { generation, items, total, .. } = wait_for(&worker);
        assert_eq!(generation, 3);
        assert_eq!((items.len(), total), (1, 2), "the total counts what matches, not what was read");
        assert!(items.iter().all(|i| i.url != "https://site1.example/" && i.url != "https://site3.example/"));
    }
}
