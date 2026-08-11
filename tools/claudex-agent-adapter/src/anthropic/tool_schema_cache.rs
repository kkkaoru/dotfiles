use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(test)]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{MessagesRequest, RequestIdentity};

const CACHE_FILE_NAME: &str = "tool-schemas-v1.json";
pub(super) const CACHE_VERSION: u8 = 1;
pub(super) const MAX_ENTRIES: usize = 1_024;
pub(super) const MAX_AGE_SECONDS: u64 = 2 * 60 * 60;

mod persist;
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

    pub(super) fn restore_or_remember(
        &self,
        identity: &RequestIdentity,
        request: &mut MessagesRequest,
        tools_were_provided: bool,
    ) {
        self.restore_or_remember_at(identity, request, tools_were_provided, unix_seconds());
    }

    fn restore_or_remember_at(
        &self,
        identity: &RequestIdentity,
        request: &mut MessagesRequest,
        tools_were_provided: bool,
        now: u64,
    ) {
        let Some(identity) = SchemaIdentity::from_request(identity) else {
            return;
        };
        let mut entries = self.entries.lock().expect("tool schema cache poisoned");
        retain_fresh(&mut entries, now);
        if !tools_were_provided {
            self.restore_cached_schema(&mut entries, &identity, request, now);
            return;
        }
        // An explicit empty array is an authoritative zero-capability request.
        // It neither restores nor mutates a previously remembered schema.
        if request.tools.is_empty() {
            return;
        }

        entries.retain(|entry| entry.identity != identity);
        let stored = self.persist(
            StoredSchema {
                identity,
                tools: request.tools.clone(),
                updated_unix_seconds: now,
                accessed_unix_seconds: now,
            },
            now,
        );
        entries.push_back(stored);
        bound_entries(&mut entries);
    }

    fn restore_cached_schema(
        &self,
        entries: &mut VecDeque<StoredSchema>,
        identity: &SchemaIdentity,
        request: &mut MessagesRequest,
        now: u64,
    ) {
        if !request.tools.is_empty() {
            return;
        }
        let Some(index) = entries
            .iter()
            .rposition(|entry| entry.identity == *identity)
        else {
            return;
        };
        let mut stored = entries.remove(index).expect("matched schema entry");
        stored.accessed_unix_seconds = now;
        let stored = self.persist(stored, now);
        request.tools.clone_from(&stored.tools);
        entries.push_back(stored);
        bound_entries(entries);
    }

    fn persist(&self, stored: StoredSchema, now: u64) -> StoredSchema {
        match &self.store {
            Some(store) => store.save(stored, now),
            None => stored,
        }
    }
}

mod store;
use store::ToolSchemaStore;

pub(super) fn schema_sort_key(tools: &[Value]) -> Vec<u8> {
    serde_json::to_vec(tools).unwrap_or_default()
}
pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests;
