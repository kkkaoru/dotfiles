//! Per-model in-flight SubAgent counts for routing demotion.
//!
//! Concurrent SubAgent launches otherwise keep selecting the same top-ranked
//! model because daemon `model_concurrency` only tracks configured limits.
//! Exposing live SubAgent occupancy lets `claudex-route-usage` soft-demote busy
//! models and fall through the weekly-remaining ranking.
//!
//! Agent IDs are published on `/health` so hot-swap sticky proxy can keep
//! in-flight SubAgents on the retained daemon while sending newly launched
//! agent IDs to the live binary.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

/// Live SubAgent occupancy keyed by request model id.
#[derive(Default)]
pub(super) struct ActiveSubagentModels {
    counts: Mutex<BTreeMap<String, usize>>,
    agent_ids: Mutex<BTreeMap<String, usize>>,
}

/// RAII guard that decrements the model count when the turn ends.
pub(super) struct ActiveSubagentGuard {
    registry: Arc<ActiveSubagentModels>,
    model: String,
    agent_id: Option<String>,
}

impl ActiveSubagentModels {
    pub(super) fn acquire(
        self: &Arc<Self>,
        model: &str,
        agent_id: Option<&str>,
    ) -> ActiveSubagentGuard {
        {
            let mut counts = self
                .counts
                .lock()
                .expect("active subagent model registry poisoned");
            *counts.entry(model.to_owned()).or_insert(0) += 1;
        }
        let agent_id = agent_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if let Some(agent_id) = agent_id.as_ref() {
            let mut agents = self
                .agent_ids
                .lock()
                .expect("active subagent agent registry poisoned");
            *agents.entry(agent_id.clone()).or_insert(0) += 1;
        }
        ActiveSubagentGuard {
            registry: Arc::clone(self),
            model: model.to_owned(),
            agent_id,
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

    pub(super) fn active_agent_ids(&self) -> Vec<String> {
        self.agent_ids
            .lock()
            .expect("active subagent agent registry poisoned")
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(agent_id, _)| agent_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn release(&self, model: &str, agent_id: Option<&str>) {
        decrement_count(&self.counts, model, "active subagent model registry poisoned");
        let Some(agent_id) = agent_id else {
            return;
        };
        decrement_count(
            &self.agent_ids,
            agent_id,
            "active subagent agent registry poisoned",
        );
    }
}

fn decrement_count(map: &Mutex<BTreeMap<String, usize>>, key: &str, poison: &str) {
    let mut counts = map.lock().expect(poison);
    let Some(count) = counts.get_mut(key) else {
        return;
    };
    *count = count.saturating_sub(1);
    if *count == 0 {
        counts.remove(key);
    }
}

impl Drop for ActiveSubagentGuard {
    fn drop(&mut self) {
        self.registry
            .release(&self.model, self.agent_id.as_deref());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn tracks_overlapping_subagent_occupancy() {
        let registry = Arc::new(ActiveSubagentModels::default());
        let first = registry.acquire("glm-5.2:cloud", Some("agent-a"));
        let second = registry.acquire("glm-5.2:cloud", Some("agent-b"));
        let other = registry.acquire("fugu", None);
        assert_eq!(registry.snapshot()["glm-5.2:cloud"], 2);
        assert_eq!(registry.snapshot()["fugu"], 1);
        assert_eq!(
            registry.active_agent_ids(),
            vec!["agent-a".to_owned(), "agent-b".to_owned()]
        );
        drop(first);
        assert_eq!(registry.snapshot()["glm-5.2:cloud"], 1);
        assert_eq!(registry.active_agent_ids(), vec!["agent-b".to_owned()]);
        drop(second);
        assert!(!registry.snapshot().contains_key("glm-5.2:cloud"));
        assert!(registry.active_agent_ids().is_empty());
        drop(other);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn release_ignores_models_that_were_never_acquired() {
        let registry = Arc::new(ActiveSubagentModels::default());
        registry.release("never-seen", Some("missing-agent"));
        assert!(registry.snapshot().is_empty());
        assert!(registry.active_agent_ids().is_empty());
    }

    #[test]
    fn blank_agent_ids_are_ignored() {
        let registry = Arc::new(ActiveSubagentModels::default());
        let guard = registry.acquire("gpt-test", Some("   "));
        assert!(registry.active_agent_ids().is_empty());
        drop(guard);
    }
}
