//! How much memory the browser uses: the proportional set size (PSS) of this process and all
//! its descendants (Chromium's GPU, network and renderer processes). PSS splits shared pages
//! between the processes sharing them, so the sum is what the browser really costs.
//!
//! Reading `smaps_rollup` walks page tables, which takes milliseconds for big renderers, so
//! measuring happens on a background thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Measures the browser's memory use periodically in the background.
pub struct MemoryMonitor {
    used: Arc<AtomicU64>,
}

impl MemoryMonitor {
    /// Starts measuring every `interval`.
    pub fn start(interval: Duration) -> Self {
        let used = Arc::new(AtomicU64::new(0));
        let shared = Arc::clone(&used);
        let spawned = std::thread::Builder::new().name("memory-monitor".into()).spawn(move || {
            loop {
                if let Some(bytes) = process_tree_pss(std::process::id()) {
                    shared.store(bytes, Ordering::Relaxed);
                }
                // Stops with the process; the monitor lives as long as the browser.
                if Arc::strong_count(&shared) == 1 {
                    return;
                }
                std::thread::sleep(interval);
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("memory monitoring unavailable: {e}");
        }
        Self { used }
    }

    /// Bytes in use by the browser and its helper processes (`None` until the first reading,
    /// or where measuring isn't supported).
    pub fn used(&self) -> Option<u64> {
        Some(self.used.load(Ordering::Relaxed)).filter(|&b| b > 0)
    }
}

/// Total physical memory in bytes.
pub fn system_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    kilobytes(&text, "MemTotal:").map(|kb| kb * 1024)
}

fn kilobytes(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with(key))?;
    line[key.len()..].split_whitespace().next()?.parse().ok()
}

/// Sum of PSS over `root` and its descendants, from `/proc` (Linux).
fn process_tree_pss(root: u32) -> Option<u64> {
    let mut parents: HashMap<u32, u32> = HashMap::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if let Some(ppid) =
            std::fs::read_to_string(entry.path().join("stat")).ok().and_then(|s| parent_of(&s))
        {
            parents.insert(pid, ppid);
        }
    }
    let in_tree = |mut pid: u32| {
        // Walk up to init; bounded in case of a cycle in a racy snapshot.
        for _ in 0..64 {
            if pid == root {
                return true;
            }
            match parents.get(&pid) {
                Some(&p) if p != 0 && p != pid => pid = p,
                _ => return false,
            }
        }
        false
    };
    let total = parents
        .keys()
        .filter(|&&pid| in_tree(pid))
        .filter_map(|pid| std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok())
        .filter_map(|text| kilobytes(&text, "Pss:"))
        .sum::<u64>();
    Some(total * 1024)
}

/// The parent pid from `/proc/<pid>/stat` ("pid (comm) state ppid …"; comm may contain spaces
/// and parentheses, so parse after the last ')').
fn parent_of(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_and_meminfo() {
        assert_eq!(parent_of("123 (Web Content (x)) S 77 123 0"), Some(77));
        assert_eq!(kilobytes("MemTotal:       32768 kB\nMemFree: 1 kB", "MemTotal:"), Some(32768));
    }

    #[test]
    fn measures_this_process() {
        if cfg!(target_os = "linux") {
            assert!(process_tree_pss(std::process::id()).is_some_and(|b| b > 0));
            assert!(system_memory().is_some_and(|b| b > 0));
        }
    }
}
