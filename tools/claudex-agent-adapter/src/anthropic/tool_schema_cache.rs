#[cfg(test)]
use std::path::PathBuf;
use std::{collections::VecDeque, sync::Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use super::MessagesRequest;
use super::RequestIdentity;

const CACHE_FILE_NAME: &str = "tool-schemas-v1.json";
pub(super) const CACHE_VERSION: u8 = 1;
pub(super) const MAX_ENTRIES: usize = 1_024;
pub(super) const MAX_AGE_SECONDS: u64 = 2 * 60 * 60;

mod persist;
mod restore;
use persist::{bound_entries, retain_fresh};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SchemaIdentity {
    session_id: String,
    agent_id: Option<String>,
    parent_agent_id: Option<String>,
}

impl SchemaIdentity {
    fn from_request(identity: &RequestIdentity) -> Option<Self> {
        Some(Self {
            session_id: identity.session_id()?.to_owned(),
            agent_id: identity.agent_id().map(str::to_owned),
            parent_agent_id: identity.parent_agent_id().map(str::to_owned),
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct StoredSchema {
    pub(super) identity: SchemaIdentity,
    pub(super) tools: Vec<Value>,
    /// Schema generation. Reads must not advance this value because a stale
    /// daemon could otherwise replace a newer schema merely by accessing it.
    pub(super) updated_unix_seconds: u64,
    #[serde(default)]
    pub(super) accessed_unix_seconds: u64,
}

#[derive(Default, Deserialize, Serialize)]
pub(super) struct StoredSchemas {
    pub(super) version: u8,
    pub(super) entries: Vec<StoredSchema>,
}

pub(super) struct ToolSchemaCache {
    entries: Mutex<VecDeque<StoredSchema>>,
    store: Option<ToolSchemaStore>,
}

impl Default for ToolSchemaCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            store: None,
        }
    }
}

impl ToolSchemaCache {
    pub(super) fn persistent() -> Self {
        let Some(store) = ToolSchemaStore::for_current_user() else {
            return Self::default();
        };
        Self {
            entries: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    #[cfg(test)]
    fn with_store(path: PathBuf) -> Self {
        let store = ToolSchemaStore::at(path);
        Self {
            entries: Mutex::new(store.load()),
            store: Some(store),
        }
    }
}

mod store;
use store::ToolSchemaStore;

#[path = "tool_schema_cache_keys.rs"]
mod keys;
pub(super) use keys::{schema_sort_key, unix_seconds};

#[cfg(test)]
mod tests;
