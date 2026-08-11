use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::agent_effort::{AgentEffortIntent, AgentEffortIntents};

#[path = "agent_intent_store_support.rs"]
mod support;
use support::{
    bound_intents, bound_vec, cache_read_failure, create_private_directory, is_fresh,
    parent_directory, restored_intent, stored_intent, valid_stored_intent, write_private,
};

pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(super) const CACHE_FILE_NAME: &str = "agent-intents-v2.json";
pub(super) const CACHE_VERSION: u8 = 2;
pub(super) const MAX_AGE_SECONDS: u64 = 2 * 60 * 60;
static NEXT_TEMPORARY_SUFFIX: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct StoredAgentIntent {
    pub(super) client_user_id: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) model_override: Option<String>,
    #[serde(default)]
    pub(super) model_is_inherited: bool,
    #[serde(default)]
    pub(super) run_in_background: bool,
    pub(super) tool_use_id: String,
    pub(super) created_unix_seconds: u64,
}

#[derive(Deserialize, Serialize)]
struct StoredAgentIntents {
    version: u8,
    intents: Vec<StoredAgentIntent>,
}

pub(super) struct AgentIntentStore {
    path: PathBuf,
}

impl AgentIntentStore {
    pub(super) fn for_current_user() -> Option<Self> {
        std::env::var_os("HOME").map(|home| Self {
            path: PathBuf::from(home)
                .join(".cache/claudex")
                .join(CACHE_FILE_NAME),
        })
    }

    #[cfg(test)]
    pub(super) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn load(&self) -> VecDeque<StoredAgentIntent> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) => return cache_read_failure(&self.path, error),
        };
        let Ok(stored) = serde_json::from_slice::<StoredAgentIntents>(&bytes) else {
            tracing::warn!(path = %self.path.display(), "could not decode persisted Agent intents");
            return VecDeque::new();
        };
        match stored.version == CACHE_VERSION {
            true => bound_intents(
                stored
                    .intents
                    .into_iter()
                    .filter(is_fresh)
                    .filter(valid_stored_intent)
                    .collect(),
            ),
            false => {
                tracing::warn!(path = %self.path.display(), "ignored incompatible persisted Agent intents");
                VecDeque::new()
            }
        }
    }

    pub(super) fn save(&self, intents: Vec<StoredAgentIntent>) {
        let document = StoredAgentIntents {
            version: CACHE_VERSION,
            intents: bound_vec(intents),
        };
        let temporary = self.temporary_path();
        let result = create_private_directory(parent_directory(&self.path))
            .and_then(|()| serde_json::to_vec(&document).map_err(std::io::Error::other))
            .and_then(|bytes| write_private(&temporary, &bytes))
            .and_then(|()| fs::rename(&temporary, &self.path));
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            tracing::warn!(%error, path = %self.path.display(), "could not persist Agent launch intents");
        }
    }

    fn temporary_path(&self) -> PathBuf {
        self.path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY_SUFFIX.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

impl Default for AgentEffortIntents {
    fn default() -> Self {
        Self {
            pending: std::sync::Mutex::new(VecDeque::new()),
            store: None,
        }
    }
}

impl AgentEffortIntents {
    pub(super) fn persistent() -> Self {
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
    pub(super) fn with_store(path: PathBuf) -> Self {
        let store = AgentIntentStore::at(path);
        let pending = store.load().into_iter().map(restored_intent).collect();
        Self {
            pending: std::sync::Mutex::new(pending),
            store: Some(store),
        }
    }

    pub(super) fn persist(&self, intents: Vec<StoredAgentIntent>) {
        let Some(store) = &self.store else {
            return;
        };
        store.save(intents);
    }
}

pub(super) fn persistence_snapshot(
    pending: &VecDeque<AgentEffortIntent>,
) -> Vec<StoredAgentIntent> {
    pending
        .iter()
        .filter(|intent| intent.correlated)
        .map(stored_intent)
        .collect()
}

pub(super) fn remove_expired(pending: &mut VecDeque<AgentEffortIntent>) {
    pending.retain(|intent| {
        intent.correlated || intent.created_at.elapsed() < super::agent_effort::INTENT_TTL
    });
}


#[cfg(test)]
include!("agent_intent_store_tests.rs");
