use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::MessagesRequest;
mod guidance;
mod records;
mod records_scope;
#[cfg(test)]
use guidance::REUSE_GUIDANCE_MARKER;
pub(super) use guidance::{agent_teams_enabled, value_text};
use guidance::{append_reuse_guidance, has_send_message_tool, system_contains_marker};
pub(in crate::anthropic) use records::live_agent_task_ids;
use records::{
    LaunchRecord, already_has_resume, apply_transcript, find_reusable_launch, latest_user_text,
    launch_model, launch_records, reusable_status, scope_is_occupied, scope_similarity,
    summarize_scope,
};

pub(crate) const MAX_SUBAGENTS_PER_SESSION_ENV: &str = "CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION";
pub(crate) const DEFAULT_MAX_SUBAGENTS_PER_SESSION: usize = 1_024;

const CACHE_FILE_NAME: &str = "subagent-recipients-v1.json";
const CACHE_VERSION: u8 = 1;
const METADATA_LIMIT_REACHED: &str = "_claudex_subagent_spawn_limit_reached";
const MAX_PERSISTED_RECIPIENTS: usize = 1_024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SessionState {
    launches: Vec<LaunchRecord>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredStates {
    version: u8,
    sessions: HashMap<String, SessionState>,
}

pub(super) struct SubagentReuseRegistry {
    states: Mutex<HashMap<String, SessionState>>,
    store: Option<Store>,
}

struct Store {
    path: PathBuf,
    // `persist` is called after releasing the registry state lock, so multiple
    // concurrent requests can otherwise truncate/rename the same temp file.
    // Serialize the atomic replacement per adapter process.
    save_lock: Mutex<()>,
}

impl Default for SubagentReuseRegistry {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            store: None,
        }
    }
}

impl SubagentReuseRegistry {
    pub(super) fn persistent() -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        let store = Store {
            path: PathBuf::from(home)
                .join(".cache/claudex")
                .join(CACHE_FILE_NAME),
            save_lock: Mutex::new(()),
        };
        Self {
            states: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    #[cfg(test)]
    pub(super) fn with_store(path: PathBuf) -> Self {
        let store = Store {
            path,
            save_lock: Mutex::new(()),
        };
        Self {
            states: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    pub(super) fn observe_and_restore(&self, request: &mut MessagesRequest) {
        self.observe_and_restore_with_reuse(request, reuse_enabled());
    }

    fn observe_and_restore_with_reuse(&self, request: &mut MessagesRequest, reuse: bool) {
        let Some(session_id) = session_id(request) else {
            return;
        };
        let observed = launch_records(&request.messages);
        let mut states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let state = states.entry(session_id).or_default();
        let previous_launches = state.launches.clone(); // Avoid fsync when the transcript is unchanged.
        // Chronological: a later resume launch result must win over an earlier
        // completion notification still present in the transcript.
        apply_transcript(&mut state.launches, &request.messages);
        let limit_reached = state.launches.len() >= max_subagents_per_session();
        set_limit_metadata(request, limit_reached);
        let should_restore = reuse
            && observed.is_empty()
            && !state.launches.is_empty()
            && !system_contains_marker(&request.system);
        let teams = agent_teams_enabled(request) && has_send_message_tool(&request.tools);
        let recipients =
            should_restore.then(|| reuse_recipients(&state.launches, &request.messages));
        let launches_changed = state.launches != previous_launches;
        let snapshot = states.clone();
        drop(states);
        if launches_changed { self.persist(snapshot); }
        if let Some(recipients) = recipients {
            append_reuse_guidance(&mut request.system, &recipients, teams);
        }
    }

    pub(super) fn rewrite_launch_input(
        &self,
        session_id: &str,
        arguments: &mut Value,
    ) -> Option<String> {
        self.rewrite_launch_input_with_reuse(session_id, arguments, reuse_enabled())
    }

    fn rewrite_launch_input_with_reuse(
        &self,
        session_id: &str,
        arguments: &mut Value,
        reuse: bool,
    ) -> Option<String> {
        if !reuse || session_id.is_empty() || already_has_resume(arguments) {
            return None;
        }
        let states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let launch = find_reusable_launch(&states.get(session_id)?.launches, arguments)?;
        // Skip resume injection if recipient is empty (pending or in-flight without confirmation)
        if launch.recipient.is_empty() {
            return None;
        }
        let recipient = launch.recipient.clone();
        drop(states);
        let object = arguments.as_object_mut()?;
        object.insert("resume".to_owned(), json!(recipient));
        tracing::info!(
            session_id,
            recipient,
            "rewrote SubAgent launch into resume of a compatible worker"
        );
        Some(recipient)
    }

    pub(super) fn scope_is_occupied(&self, session_id: &str, arguments: &Value) -> bool {
        let scope_key = records::launch_scope_key(arguments);
        if session_id.is_empty() || scope_key.is_empty() {
            return false;
        }
        let model = launch_model(arguments);
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session_id)
            .is_some_and(|state| scope_is_occupied(&state.launches, &scope_key, model))
    }

    /// Remember a just-forwarded launch before its tool_result exists so a
    /// same-turn duplicate cannot spawn another same-model worker.
    pub(super) fn note_inflight_launch(
        &self,
        session_id: &str,
        arguments: &Value,
        tool_use_id: &str,
    ) {
        if !reuse_enabled() || session_id.is_empty() || tool_use_id.is_empty() {
            return;
        }
        let scope = summarize_scope(arguments);
        if scope.is_empty() {
            return;
        }
        let model = launch_model(arguments).map(str::to_owned);
        let mut states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let state = states.entry(session_id.to_owned()).or_default();
        records::merge_launches(
            &mut state.launches,
            std::iter::once(&LaunchRecord {
                key: tool_use_id.to_owned(),
                recipient: String::new(),
                scope,
                model,
                status: "pending".to_owned(),
            }),
        );
    }

    #[cfg(test)]
    pub(super) fn observe_and_restore_for_test(&self, request: &mut MessagesRequest, reuse: bool) {
        self.observe_and_restore_with_reuse(request, reuse);
    }

    #[cfg(test)]
    pub(super) fn rewrite_launch_input_for_test(
        &self,
        session_id: &str,
        arguments: &mut Value,
        reuse: bool,
    ) -> Option<String> {
        self.rewrite_launch_input_with_reuse(session_id, arguments, reuse)
    }

    #[cfg(test)]
    pub(super) fn state_for(&self, session: &str) -> Option<Vec<String>> {
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session)
            .map(|state| {
                state
                    .launches
                    .iter()
                    .map(|launch| launch.recipient.clone())
                    .filter(|recipient| !recipient.is_empty())
                    .collect()
            })
    }

    #[cfg(test)]
    pub(super) fn status_for(&self, session: &str, recipient: &str) -> Option<String> {
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session)?
            .launches
            .iter()
            .find(|launch| launch.recipient == recipient)
            .map(|launch| launch.status.clone())
    }

    fn persist(&self, states: HashMap<String, SessionState>) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(error) = store.save(states) {
            tracing::warn!(%error, path = %store.path.display(), "could not persist SubAgent reuse registry");
        }
    }
}

impl Store {
    fn load(&self) -> HashMap<String, SessionState> {
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

    fn save(&self, mut states: HashMap<String, SessionState>) -> std::io::Result<()> {
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

pub(super) fn max_subagents_per_session() -> usize {
    std::env::var(MAX_SUBAGENTS_PER_SESSION_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_SUBAGENTS_PER_SESSION)
}

pub(super) fn should_expose_launch_tools(request: &MessagesRequest) -> bool {
    request
        .metadata
        .get(METADATA_LIMIT_REACHED)
        .and_then(Value::as_bool)
        .is_none_or(|reached| !reached)
}

fn reuse_recipients(launches: &[LaunchRecord], messages: &[Value]) -> Vec<String> {
    let task = latest_user_text(messages);
    // Skip inflight placeholders (empty agentId) and terminal failures so
    // resume / prompt-cache guidance only names workers rewrite can actually use.
    let mut sorted = launches
        .iter()
        .filter(|launch| reusable_status(&launch.status) && !launch.recipient.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    sorted.sort_by_key(|launch| std::cmp::Reverse(scope_similarity(&launch.scope, &task)));
    sorted.iter().map(format_reuse_recipient).collect()
}

fn format_reuse_recipient(launch: &LaunchRecord) -> String {
    let scope = launch.scope.as_str();
    let scope = if scope.is_empty() { "scope unknown" } else { scope };
    let model = launch.model.as_deref().unwrap_or("model unknown");
    format!("{} ({}; {}; {})", launch.recipient, scope, model, launch.status)
}

fn prune_persisted_state(state: &mut SessionState) {
    let excess = state
        .launches
        .len()
        .saturating_sub(MAX_PERSISTED_RECIPIENTS);
    state.launches.drain(..excess);
}

pub(super) fn is_launch_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

pub(super) fn reuse_enabled() -> bool {
    match std::env::var(crate::parallel_scheduler::SUBAGENT_REUSE_ENV) {
        Ok(value) => matches!(
            value.as_str(),
            "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => true,
    }
}

pub(super) fn session_id(request: &MessagesRequest) -> Option<String> {
    super::request_identity::claude_session_id(request)
}

fn set_limit_metadata(request: &mut MessagesRequest, reached: bool) {
    if !request.metadata.is_object() {
        request.metadata = Value::Object(Map::new());
    }
    request
        .metadata
        .as_object_mut()
        .expect("metadata object")
        .insert(METADATA_LIMIT_REACHED.to_owned(), Value::Bool(reached));
}

#[cfg(test)]
#[path = "subagent_reuse_tests.rs"]
mod tests;
