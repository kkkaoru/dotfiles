use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::agent_effort::{AgentEffortIntent, AgentEffortIntents};
use super::subscription::valid_effort;

const CACHE_FILE_NAME: &str = "agent-intents-v2.json";
const CACHE_VERSION: u8 = 2;
const MAX_AGE_SECONDS: u64 = 2 * 60 * 60;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct StoredAgentIntent {
    pub(super) client_user_id: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) model_override: Option<String>,
    #[serde(default)]
    pub(super) model_is_inherited: bool,
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
        self.path
            .with_extension(format!("{}.tmp", std::process::id()))
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

fn bound_intents(mut intents: VecDeque<StoredAgentIntent>) -> VecDeque<StoredAgentIntent> {
    while intents.len() > super::agent_effort::MAX_PENDING_INTENTS {
        intents.pop_front();
    }
    intents
}

fn bound_vec(mut intents: Vec<StoredAgentIntent>) -> Vec<StoredAgentIntent> {
    let excess = intents
        .len()
        .saturating_sub(super::agent_effort::MAX_PENDING_INTENTS);
    if excess > 0 {
        intents.drain(..excess);
    }
    intents
}

fn valid_stored_intent(intent: &StoredAgentIntent) -> bool {
    !intent.tool_use_id.is_empty()
        && intent.effort.as_deref().is_none_or(valid_effort)
        && intent
            .model_override
            .as_deref()
            .is_none_or(|model| !model.is_empty())
}

fn is_fresh(intent: &StoredAgentIntent) -> bool {
    unix_seconds().saturating_sub(intent.created_unix_seconds) <= MAX_AGE_SECONDS
}

pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn cache_read_failure(path: &Path, error: std::io::Error) -> VecDeque<StoredAgentIntent> {
    if error.kind() != std::io::ErrorKind::NotFound {
        tracing::warn!(%error, path = %path.display(), "could not restore persisted Agent intents");
    }
    VecDeque::new()
}

fn restored_intent(stored: StoredAgentIntent) -> AgentEffortIntent {
    AgentEffortIntent {
        client_user_id: stored.client_user_id,
        prompt: String::new(),
        correlated: true,
        effort: stored.effort,
        model_override: stored.model_override,
        model_is_inherited: stored.model_is_inherited,
        tool_use_id: stored.tool_use_id,
        created_at: std::time::Instant::now(),
        created_unix_seconds: stored.created_unix_seconds,
    }
}

fn stored_intent(intent: &AgentEffortIntent) -> StoredAgentIntent {
    StoredAgentIntent {
        client_user_id: intent.client_user_id.clone(),
        effort: intent.effort.clone(),
        model_override: intent.model_override.clone(),
        model_is_inherited: intent.model_is_inherited,
        tool_use_id: intent.tool_use_id.clone(),
        created_unix_seconds: intent.created_unix_seconds,
    }
}

fn parent_directory(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

#[cfg(test)]
include!("agent_intent_store_tests.rs");
