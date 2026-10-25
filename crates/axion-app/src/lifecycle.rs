//! Settings → tab lifecycle policy, and the memory budget.

use std::time::Duration;

use axion_config::Config;
use axion_tabs::Policy;

/// Share of the computer's memory used as budget when `tabs.memory_budget_mb` is 0.
const AUTO_BUDGET_SHARE: f64 = 0.4;
const MIB: u64 = 1024 * 1024;

fn minutes(m: f32) -> Option<Duration> {
    (m > 0.0).then(|| Duration::from_secs_f32(m * 60.0))
}

/// The memory budget in bytes, or `None` when memory isn't limited.
pub fn memory_budget(config: &Config) -> Option<u64> {
    let tabs = &config.tabs;
    if !tabs.limit_memory {
        return None;
    }
    match tabs.memory_budget_mb {
        0 => axion_tabs::system_memory().map(|total| (total as f64 * AUTO_BUDGET_SHARE) as u64),
        mb => Some(u64::from(mb) * MIB),
    }
}

pub fn policy(config: &Config) -> Policy {
    Policy {
        freeze_after: minutes(config.tabs.freeze_after_minutes),
        discard_after: minutes(config.tabs.discard_after_minutes),
        memory_budget: memory_budget(config),
        keep_media_awake: config.tabs.keep_media_awake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_means_never_and_automatic() {
        let mut config = Config::default();
        config.tabs.freeze_after_minutes = 0.0;
        config.tabs.memory_budget_mb = 2048;
        let p = policy(&config);
        assert_eq!(p.freeze_after, None);
        assert_eq!(p.discard_after, Some(Duration::from_secs(120 * 60)));
        assert_eq!(p.memory_budget, Some(2048 * MIB));
        config.tabs.limit_memory = false;
        assert_eq!(memory_budget(&config), None);
    }
}
