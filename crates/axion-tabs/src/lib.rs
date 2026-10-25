//! Tab lifecycle: which background tabs to freeze or discard, by how long they have been in
//! the background and by the browser's memory use against the user's budget.
//!
//! The planner is pure — the browser describes its tabs, gets [`Transition`]s back and carries
//! them out through the engine. [`MemoryMonitor`] measures what Chromium uses.

mod memory;

use std::time::{Duration, Instant};

pub use memory::{MemoryMonitor, system_memory};

/// Where a tab is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lifecycle {
    /// Shown in the window.
    Active,
    /// Hidden but running (throttled by Chromium).
    #[default]
    Background,
    /// Hidden and paused: no JavaScript, timers or network until it is shown again.
    Frozen,
    /// The page is closed to free its memory; the URL and a snapshot are kept and it reloads
    /// when shown again.
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Policy {
    /// Freeze tabs that have been in the background this long (`None`: never).
    pub freeze_after: Option<Duration>,
    /// Discard tabs that have been in the background this long (`None`: never).
    pub discard_after: Option<Duration>,
    /// Discard background tabs while the browser uses more than this (bytes).
    pub memory_budget: Option<u64>,
    /// Tabs playing audio or video are never frozen or discarded.
    pub keep_media_awake: bool,
}

/// What the planner needs to know about a tab. `K` identifies it for the caller.
#[derive(Clone, Copy, Debug)]
pub struct TabInfo<K> {
    pub key: K,
    pub lifecycle: Lifecycle,
    /// When it was last hidden (`None` while active).
    pub hidden_since: Option<Instant>,
    pub playing_media: bool,
    /// Still loading: freezing now would leave it half-loaded.
    pub loading: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition<K> {
    Freeze(K),
    Discard(K),
}

/// What to do with the tabs now. Call every few seconds; over budget, it discards one tab per
/// call (the one hidden longest) so the next memory reading can show the effect.
pub fn plan<K: Copy + PartialEq>(
    policy: &Policy,
    tabs: &[TabInfo<K>],
    now: Instant,
    memory_used: Option<u64>,
) -> Vec<Transition<K>> {
    let idle =
        |t: &TabInfo<K>| t.hidden_since.map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
    let sleepable: Vec<&TabInfo<K>> = tabs
        .iter()
        .filter(|t| {
            t.lifecycle != Lifecycle::Active
                && t.lifecycle != Lifecycle::Discarded
                && t.hidden_since.is_some()
                && !t.loading
                && !(policy.keep_media_awake && t.playing_media)
        })
        .collect();
    let mut out = Vec::new();
    for tab in &sleepable {
        if policy.discard_after.is_some_and(|d| idle(tab) >= d) {
            out.push(Transition::Discard(tab.key));
        } else if tab.lifecycle == Lifecycle::Background
            && policy.freeze_after.is_some_and(|d| idle(tab) >= d)
        {
            out.push(Transition::Freeze(tab.key));
        }
    }
    let over_budget =
        matches!((memory_used, policy.memory_budget), (Some(used), Some(budget)) if used > budget);
    if over_budget
        && !out.iter().any(|t| matches!(t, Transition::Discard(_)))
        && let Some(tab) = sleepable.iter().max_by_key(|t| idle(t))
    {
        out.retain(|t| *t != Transition::Freeze(tab.key));
        out.push(Transition::Discard(tab.key));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn policy() -> Policy {
        Policy {
            freeze_after: Some(5 * MIN),
            discard_after: Some(60 * MIN),
            memory_budget: Some(1000),
            keep_media_awake: true,
        }
    }

    fn tab(key: u32, lifecycle: Lifecycle, hidden_for: Option<Duration>, now: Instant) -> TabInfo<u32> {
        TabInfo {
            key,
            lifecycle,
            hidden_since: hidden_for.map(|d| now - d),
            playing_media: false,
            loading: false,
        }
    }

    #[test]
    fn freezes_then_discards_by_idle_time() {
        let now = Instant::now() + 120 * MIN;
        let tabs = [
            tab(0, Lifecycle::Active, None, now),
            tab(1, Lifecycle::Background, Some(MIN), now),
            tab(2, Lifecycle::Background, Some(6 * MIN), now),
            tab(3, Lifecycle::Frozen, Some(61 * MIN), now),
            tab(4, Lifecycle::Frozen, Some(10 * MIN), now),
        ];
        assert_eq!(plan(&policy(), &tabs, now, Some(10)), [Transition::Freeze(2), Transition::Discard(3)]);
    }

    #[test]
    fn media_and_loading_tabs_stay_awake() {
        let now = Instant::now() + 120 * MIN;
        let mut playing = tab(1, Lifecycle::Background, Some(90 * MIN), now);
        playing.playing_media = true;
        let mut loading = tab(2, Lifecycle::Background, Some(90 * MIN), now);
        loading.loading = true;
        assert!(plan(&policy(), &[playing, loading], now, Some(5000)).is_empty());
        let mut lax = policy();
        lax.keep_media_awake = false;
        assert_eq!(plan(&lax, &[playing], now, None), [Transition::Discard(1)]);
    }

    #[test]
    fn over_budget_discards_the_longest_hidden() {
        let now = Instant::now() + 120 * MIN;
        let tabs = [
            tab(0, Lifecycle::Active, None, now),
            tab(1, Lifecycle::Background, Some(MIN), now),
            tab(2, Lifecycle::Background, Some(6 * MIN), now),
            tab(3, Lifecycle::Discarded, Some(100 * MIN), now),
        ];
        assert_eq!(plan(&policy(), &tabs, now, Some(2000)), [Transition::Discard(2)]);
        // Never the active tab, even if nothing else is left.
        assert!(plan(&policy(), &tabs[..1], now, Some(2000)).is_empty());
    }

    #[test]
    fn disabled_policy_does_nothing() {
        let now = Instant::now() + 120 * MIN;
        let off =
            Policy { freeze_after: None, discard_after: None, memory_budget: None, keep_media_awake: true };
        assert!(
            plan(&off, &[tab(1, Lifecycle::Background, Some(100 * MIN), now)], now, Some(u64::MAX))
                .is_empty()
        );
    }
}
