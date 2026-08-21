#![allow(clippy::excessive_nesting)]

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    MessagesRequest,
    records::{LaunchRecord, reusable_status},
};

mod claims;
mod io;
mod merge;

pub(super) use super::records::occupancy_matches;
pub(crate) use claims::{current_pid, unix_seconds};
use io::{StoreLock, create_private_directory};
use merge::{bound_document, merge_session_state, prune_persisted_state};

#[cfg(test)]
#[path = "store/merge_tests.rs"]
mod merge_tests;

pub(super) const CACHE_FILE_NAME: &str = "subagent-recipients-v1.json";
pub(super) const CACHE_VERSION: u8 = 2;
pub(super) const LEGACY_CACHE_VERSION: u8 = 1;
pub(super) const METADATA_LIMIT_REACHED: &str = "_claudex_subagent_spawn_limit_reached";
pub(super) const CLAIM_TTL_SECONDS: u64 = 5 * 60;
static NEXT_TEMPORARY_SUFFIX: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct SessionState {
    pub(super) launches: Vec<LaunchRecord>,
}

/// Claims are admission leases, not transcript facts. They live outside the
/// canonical session/tombstone maps so stale snapshots cannot overwrite them.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct ClaimRecord {
    pub(super) session_id: String,
    pub(super) scope: String,
    #[serde(default)]
    pub(super) model: Option<String>,
    pub(super) owner: String,
    pub(super) pid: u32,
    pub(super) created_revision: u64,
    pub(super) expires_unix_seconds: u64,
    #[serde(default)]
    pub(super) tool_use_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct ClaimRequest {
    pub(super) session_id: String,
    pub(super) scope: String,
    pub(super) model: Option<String>,
    pub(super) owner: String,
    pub(super) pid: u32,
    pub(super) tool_use_id: String,
    pub(super) expires_unix_seconds: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LoadedStates {
    pub(super) sessions: HashMap<String, SessionState>,
    pub(super) session_revisions: HashMap<String, u64>,
    #[allow(dead_code)]
    pub(super) tombstones: HashMap<String, u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct StoredStates {
    pub(super) version: u8,
    #[serde(default)]
    pub(super) revision: u64,
    #[serde(rename = "sessions", default)]
    pub(super) sessions: HashMap<String, SessionState>,
    #[serde(default)]
    pub(super) session_revisions: HashMap<String, u64>,
    #[serde(default)]
    pub(super) tombstones: HashMap<String, u64>,
    #[serde(default)]
    pub(super) claims: HashMap<String, ClaimRecord>,
}

/// Literal v1 field names are retained while migrating. Deserializing v1 via
/// the v2 defaults would silently treat an old cache as a current snapshot.
#[derive(Clone, Debug, Default, Deserialize)]
struct StoredStatesV1 {
    #[serde(rename = "version")]
    version: u8,
    #[serde(rename = "sessions", default)]
    sessions: HashMap<String, SessionState>,
}

pub(super) struct Store {
    pub(super) path: PathBuf,
    save_lock: Mutex<()>,
}

impl Store {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            save_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn load(&self) -> HashMap<String, SessionState> {
        self.load_snapshot().sessions
    }

    pub(super) fn load_snapshot(&self) -> LoadedStates {
        self.read_document()
            .map(|document| LoadedStates {
                sessions: document.sessions,
                session_revisions: document.session_revisions,
                tombstones: document.tombstones,
            })
            .unwrap_or_default()
    }

    /// Compatibility path for callers that still provide a complete snapshot.
    /// New registry code uses the revisioned per-session delta below.
    #[cfg(test)]
    pub(super) fn save(&self, states: HashMap<String, SessionState>) -> std::io::Result<()> {
        self.with_locked_document(|document| {
            for (session_id, mut incoming) in states {
                prune_persisted_state(&mut incoming);
                if document.tombstones.contains_key(&session_id) {
                    continue;
                }
                match document.sessions.get_mut(&session_id) {
                    Some(current) => merge_session_state(current, &incoming),
                    None => {
                        document.sessions.insert(session_id.clone(), incoming);
                    }
                }
                document.revision = document.revision.saturating_add(1);
                document
                    .session_revisions
                    .insert(session_id, document.revision);
            }
            Ok(())
        })
    }

    /// Apply one canonical session delta. A stale compacted transcript cannot
    /// clear a newer tombstone because its base revision is fenced.
    pub(super) fn save_session_delta(
        &self,
        session_id: &str,
        mut state: SessionState,
        base_revision: u64,
    ) -> std::io::Result<bool> {
        if session_id.is_empty() {
            return Ok(false);
        }
        self.with_locked_document(|document| {
            if document
                .tombstones
                .get(session_id)
                .is_some_and(|revision| *revision > base_revision)
            {
                return Ok(false);
            }
            prune_persisted_state(&mut state);
            if let Some(current) = document.sessions.get_mut(session_id) {
                if document
                    .session_revisions
                    .get(session_id)
                    .is_some_and(|revision| *revision > base_revision)
                {
                    merge_session_state(current, &state);
                } else {
                    *current = state;
                }
            } else {
                document.sessions.insert(session_id.to_owned(), state);
            }
            document.tombstones.remove(session_id);
            document.revision = document.revision.saturating_add(1);
            document
                .session_revisions
                .insert(session_id.to_owned(), document.revision);
            Ok(true)
        })
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn delete_session(
        &self,
        session_id: &str,
        base_revision: u64,
    ) -> std::io::Result<bool> {
        if session_id.is_empty() {
            return Ok(false);
        }
        self.with_locked_document(|document| {
            if document
                .session_revisions
                .get(session_id)
                .is_some_and(|revision| *revision > base_revision)
            {
                return Ok(false);
            }
            document.sessions.remove(session_id);
            document.revision = document.revision.saturating_add(1);
            document
                .session_revisions
                .insert(session_id.to_owned(), document.revision);
            document
                .tombstones
                .insert(session_id.to_owned(), document.revision);
            Ok(true)
        })
    }

    fn read_document(&self) -> Option<StoredStates> {
        let bytes = fs::read(&self.path).ok()?;
        let value = serde_json::from_slice::<Value>(&bytes).ok()?;
        let version = value.get("version").and_then(Value::as_u64)? as u8;
        match version {
            CACHE_VERSION => serde_json::from_value(value).ok(),
            LEGACY_CACHE_VERSION => {
                let legacy = serde_json::from_value::<StoredStatesV1>(value).ok()?;
                (legacy.version == LEGACY_CACHE_VERSION).then(|| StoredStates {
                    version: CACHE_VERSION,
                    revision: 0,
                    sessions: legacy.sessions,
                    ..StoredStates::default()
                })
            }
            _ => {
                tracing::warn!(path = %self.path.display(), "ignored incompatible SubAgent reuse registry");
                None
            }
        }
    }

    fn with_locked_document<T>(
        &self,
        operation: impl FnOnce(&mut StoredStates) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let _save_guard = self
            .save_lock
            .lock()
            .expect("SubAgent reuse store poisoned");
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        create_private_directory(parent)?;
        let _lock = StoreLock::acquire(&self.path)?;
        let mut document = self.read_document().unwrap_or_else(|| StoredStates {
            version: CACHE_VERSION,
            ..StoredStates::default()
        });
        document.version = CACHE_VERSION;
        let result = operation(&mut document)?;
        bound_document(&mut document);
        self.write_document(&document)?;
        Ok(result)
    }

    fn write_document(&self, document: &StoredStates) -> std::io::Result<()> {
        let temporary = self.path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY_SUFFIX.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec(document).map_err(std::io::Error::other)?;
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

pub(super) fn set_limit_metadata(request: &mut MessagesRequest, reached: bool) {
    if !request.metadata.is_object() {
        request.metadata = Value::Object(Map::new());
    }
    request
        .metadata
        .as_object_mut()
        .expect("metadata object")
        .insert(METADATA_LIMIT_REACHED.to_owned(), Value::Bool(reached));
}

pub(super) fn reuse_recipients(launches: &[LaunchRecord], _messages: &[Value]) -> Vec<String> {
    let mut sorted = launches
        .iter()
        .filter(|launch| reusable_status(&launch.status) && !launch.recipient.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        left.recipient
            .cmp(&right.recipient)
            .then(left.key.cmp(&right.key))
    });
    sorted.iter().map(format_reuse_recipient).collect()
}

fn format_reuse_recipient(launch: &LaunchRecord) -> String {
    let scope = if launch.scope.is_empty() {
        "scope unknown"
    } else {
        launch.scope.as_str()
    };
    let model = launch.model.as_deref().unwrap_or("model unknown");
    format!("{} ({}; {})", launch.recipient, scope, model)
}
