//! Per-model in-flight SubAgent counts for routing demotion.
//!
//! Concurrent SubAgent launches otherwise keep selecting the same top-ranked
//! model because daemon `model_concurrency` only tracks configured limits.
//! Exposing live SubAgent occupancy lets `claudex-route-usage` soft-demote busy
//! models and fall through the weekly-remaining ranking.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Live SubAgent occupancy keyed by request model id.
#[derive(Default)]
pub(super) struct ActiveSubagentModels {
    counts: Mutex<BTreeMap<String, usize>>,
}

/// RAII guard that decrements the model count when the turn ends.
pub(super) struct ActiveSubagentGuard {
    registry: Arc<ActiveSubagentModels>,
    model: String,
}

impl ActiveSubagentModels {
    pub(super) fn acquire(self: &Arc<Self>, model: &str) -> ActiveSubagentGuard {
        {
            let mut counts = self
                .counts
                .lock()
                .expect("active subagent model registry poisoned");
            *counts.entry(model.to_owned()).or_insert(0) += 1;
        }
        ActiveSubagentGuard {
            registry: Arc::clone(self),
            model: model.to_owned(),
        }
    }

    pub(super) fn snapshot(&self) -> BTreeMap<String, usize> {
        self.counts
            .lock()
            .expect("active subagent model registry poisoned")
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(model, count)| (model.clone(), *count))
            .collect()
    }

    fn release(&self, model: &str) {
        let mut counts = self
            .counts
            .lock()
            .expect("active subagent model registry poisoned");
        let Some(count) = counts.get_mut(model) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(model);
        }
    }
}

impl Drop for ActiveSubagentGuard {
    fn drop(&mut self) {
        self.registry.release(&self.model);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn tracks_overlapping_subagent_occupancy() {
        let registry = Arc::new(ActiveSubagentModels::default());
        let first = registry.acquire("glm-5.2:cloud");
        let second = registry.acquire("glm-5.2:cloud");
        let other = registry.acquire("fugu");
        assert_eq!(registry.snapshot()["glm-5.2:cloud"], 2);
        assert_eq!(registry.snapshot()["fugu"], 1);
        drop(first);
        assert_eq!(registry.snapshot()["glm-5.2:cloud"], 1);
        drop(second);
        assert!(!registry.snapshot().contains_key("glm-5.2:cloud"));
        drop(other);
        assert!(registry.snapshot().is_empty());
    }
}
