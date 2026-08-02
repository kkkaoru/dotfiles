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
mod records;
use records::{
    LaunchRecord, latest_user_text, launch_records, merge_launches, scope_similarity,
    update_status_from_notifications,
};

pub(crate) const MAX_SUBAGENTS_PER_SESSION_ENV: &str = "CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION";
pub(crate) const DEFAULT_MAX_SUBAGENTS_PER_SESSION: usize = 1_024;

const CACHE_FILE_NAME: &str = "subagent-recipients-v1.json";
const CACHE_VERSION: u8 = 1;
const METADATA_LIMIT_REACHED: &str = "_claudex_subagent_spawn_limit_reached";
const REUSE_GUIDANCE_MARKER: &str = "<claudex-subagent-reuse>";
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
        let Some(session_id) = session_id(request).map(str::to_owned) else {
            return;
        };
        let observed = launch_records(&request.messages);
        let mut states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let state = states.entry(session_id).or_default();
        // Runs before the merge so `merge_launches` sees current statuses: it only
        // folds a same-scope launch into an existing record while that record is
        // non-terminal, so a stale status would wrongly block relaunching a
        // finished worker. Runs again afterwards to status the newly pushed records.
        update_status_from_notifications(&mut state.launches, &request.messages);
        merge_launches(&mut state.launches, observed.iter());
        update_status_from_notifications(&mut state.launches, &request.messages);
        let limit_reached = state.launches.len() >= max_subagents_per_session();
        set_limit_metadata(request, limit_reached);

        let should_restore = observed.is_empty()
            && !state.launches.is_empty()
            && has_send_message_tool(&request.tools)
            && agent_teams_enabled(request)
            && !system_contains_marker(&request.system);
        let recipients =
            should_restore.then(|| reuse_recipients(&state.launches, &request.messages));
        let snapshot = states.clone();
        drop(states);
        self.persist(snapshot);

        if let Some(recipients) = recipients {
            append_reuse_guidance(&mut request.system, &recipients);
        }
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
                    .collect()
            })
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
    let mut sorted = launches.to_vec();
    sorted.sort_by_key(|launch| std::cmp::Reverse(scope_similarity(&launch.scope, &task)));
    sorted.iter().map(format_reuse_recipient).collect()
}

fn format_reuse_recipient(launch: &LaunchRecord) -> String {
    let scope = if launch.scope.is_empty() {
        "scope unknown"
    } else {
        launch.scope.as_str()
    };
    let model = launch.model.as_deref().unwrap_or("model unknown");
    format!(
        "{} ({}; {}; {})",
        launch.recipient, scope, model, launch.status
    )
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

fn session_id(request: &MessagesRequest) -> Option<&str> {
    request
        .metadata
        .pointer("/_claudex_transport_identity/session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
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

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| value_text(Some(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(object)) => object
            .get("text")
            .map(|text| value_text(Some(text)))
            .unwrap_or_else(|| {
                object
                    .values()
                    .map(|value| value_text(Some(value)))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        _ => String::new(),
    }
}

fn has_send_message_tool(tools: &[Value]) -> bool {
    tools.iter().any(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name == "SendMessage")
    })
}

/// A normal Agent/Task session must not use the mailbox transport for worker
/// results. Claude exposes SendMessage only for an explicit Agent Teams
/// session; treating its mere presence as an invitation causes raw
/// `<agent-message>` attachments to be queued as user input.
pub(super) fn agent_teams_enabled(request: &MessagesRequest) -> bool {
    let team_tool = request.tools.iter().any(|tool| {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
        matches!(
            name,
            "TeamCreate"
                | "TeamDelete"
                | "TeamList"
                | "TeamRename"
                | "TeamUpdate"
                | "TeamSendMessage"
        ) || name.starts_with("team_")
    });
    let explicit_team_text = request.messages.iter().any(|message| {
        let text = value_text(Some(message));
        text.contains("USE_NAMED_TEAM_MAILBOX") || text.contains("<teammate-message")
    });
    team_tool || explicit_team_text
}

fn system_contains_marker(system: &Value) -> bool {
    value_text(Some(system)).contains(REUSE_GUIDANCE_MARKER)
}

fn append_reuse_guidance(system: &mut Value, recipients: &[String]) {
    let recipients = recipients
        .iter()
        .take(32)
        .map(|recipient| format!("`{recipient}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let guidance = format!(
        "{REUSE_GUIDANCE_MARKER}\nThis resumed Agent Teams session already has compatible teammates and their recorded scopes: {recipients}. Choose the teammate whose recorded scope best matches the current task, then use TeamSendMessage with that exact recipient. Do not assign unrelated work to a teammate merely because it is available. Do not launch a replacement for the same scope. Create a new Agent/Task only for genuinely independent work or when the existing teammate is unavailable.\n</claudex-subagent-reuse>"
    );
    match system {
        Value::String(text) => {
            text.push_str("\n\n");
            text.push_str(&guidance);
        }
        Value::Array(blocks) => blocks.push(json!({"type":"text","text":guidance})),
        _ => *system = Value::String(guidance),
    }
}

#[cfg(test)]
include!("subagent_reuse_tests.rs");
