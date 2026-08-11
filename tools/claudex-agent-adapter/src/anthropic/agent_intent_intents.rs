use std::collections::VecDeque;
#[cfg(test)]
use std::path::PathBuf;

use super::{
    AgentEffortIntent, AgentEffortIntents, AgentIntentStore, StoredAgentIntent, restored_intent,
    stored_intent,
};

impl Default for AgentEffortIntents {
    fn default() -> Self {
        Self {
            pending: std::sync::Mutex::new(VecDeque::new()),
            store: None,
        }
    }
}

impl AgentEffortIntents {
    pub(in crate::anthropic) fn persistent() -> Self {
        let Some(store) = AgentIntentStore::for_current_user() else {
            return Self::default();
        };
        let pending = store.load().into_iter().map(restored_intent).collect();
        Self {
            pending: std::sync::Mutex::new(pending),
            store: Some(store),
        }
    }

    #[cfg(test)]
    pub(in crate::anthropic) fn with_store(path: PathBuf) -> Self {
        let store = AgentIntentStore::at(path);
        let pending = store.load().into_iter().map(restored_intent).collect();
        Self {
            pending: std::sync::Mutex::new(pending),
            store: Some(store),
        }
    }

    pub(in crate::anthropic) fn persist(&self, intents: Vec<StoredAgentIntent>) {
        let Some(store) = &self.store else {
            return;
        };
        store.save(intents);
    }
}

pub(in crate::anthropic) fn persistence_snapshot(
    pending: &VecDeque<AgentEffortIntent>,
) -> Vec<StoredAgentIntent> {
    pending
        .iter()
        .filter(|intent| intent.correlated)
        .map(stored_intent)
        .collect()
}

pub(in crate::anthropic) fn remove_expired(pending: &mut VecDeque<AgentEffortIntent>) {
    pending.retain(|intent| {
        intent.correlated || intent.created_at.elapsed() < super::super::agent_effort::INTENT_TTL
    });
}
