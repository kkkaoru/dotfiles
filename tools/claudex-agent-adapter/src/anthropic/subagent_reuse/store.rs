use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    MessagesRequest,
    records::{LaunchRecord, reusable_status},
};

pub(super) const CACHE_FILE_NAME: &str = "subagent-recipients-v1.json";
pub(super) const CACHE_VERSION: u8 = 1;
pub(super) const METADATA_LIMIT_REACHED: &str = "_claudex_subagent_spawn_limit_reached";
const MAX_PERSISTED_RECIPIENTS: usize = 1_024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct SessionState {
    pub(super) launches: Vec<LaunchRecord>,
}

#[derive(Default, Deserialize, Serialize)]
pub(super) struct StoredStates {
    version: u8,
    #[serde(default)]
    sessions: HashMap<String, SessionState>,
}

pub(super) struct Store {
    pub(super) path: PathBuf,
    // `persist` is called after releasing the registry state lock, so multiple
    // concurrent requests can otherwise truncate/rename the same temp file.
    // Serialize the atomic replacement per adapter process.
    save_lock: Mutex<()>,
}

impl Store {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            save_lock: Mutex::new(()),
        }
    }

    pub(super) fn load(&self) -> HashMap<String, SessionState> {
        let Ok(bytes) = fs::read(&self.path) else {
            return HashMap::new();
        };
        let Ok(stored) = serde_json::from_slice::<StoredStates>(&bytes) else {
            tracing::warn!(path = %self.path.display(), "could not decode SubAgent reuse registry");
            return HashMap::new();
        };
        if stored.version != CACHE_VERSION {
            tracing::warn!(path = %self.path.display(), "ignored incompatible SubAgent reuse registry");
            return HashMap::new();
        }
        stored.sessions
    }

    pub(super) fn save(&self, mut states: HashMap<String, SessionState>) -> std::io::Result<()> {
        let _save_guard = self
            .save_lock
            .lock()
            .expect("SubAgent reuse store poisoned");
        states.values_mut().for_each(prune_persisted_state);
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec(&StoredStates {
            version: CACHE_VERSION,
            sessions: states,
        })
        .map_err(std::io::Error::other)?;
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)
    }
}

pub(super) fn reuse_recipients(launches: &[LaunchRecord], _messages: &[Value]) -> Vec<String> {
    // Omit empty agentId / failures; stable order keeps prompt-cache signatures.
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
    // Omit status so active→queued→completed churn does not bust prompt-cache.
    format!("{} ({}; {})", launch.recipient, scope, model)
}

fn prune_persisted_state(state: &mut SessionState) {
    let excess = state
        .launches
        .len()
        .saturating_sub(MAX_PERSISTED_RECIPIENTS);
    state.launches.drain(..excess);
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
