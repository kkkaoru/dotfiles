use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Instant,
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::{agent_backend::AgentBackend, app_server::ThreadEvents};

use super::{
    active_subagent_models, agent_effort, model_concurrency, subagent_reuse, tool_schema_cache,
};

pub(crate) struct AgentEffortRecord<'a> {
    pub(crate) client_user_id: Option<&'a str>,
    pub(crate) tool_name: &'a str,
    pub(crate) tool_use_id: String,
    pub(crate) parent_model: &'a str,
    pub(crate) arguments: &'a Value,
    pub(crate) user_messages: &'a [Value],
    pub(crate) system: &'a Value,
}
pub(crate) const MAX_SESSIONS: usize = 1_024;
pub(crate) const MAX_SIGNATURE_BUCKETS: usize = MAX_SESSIONS * 2;
pub(crate) type SignaturePool = StdMutex<HashMap<u64, Vec<Weak<str>>>>;
#[derive(Clone, Debug, Deserialize)]
pub struct MessagesRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub system: Value,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub output_config: Value,
    #[serde(default)]
    pub metadata: Value,
    #[serde(skip)]
    pub working_directory: Option<PathBuf>,
    #[serde(skip)]
    pub disabled_subagent_models: BTreeSet<String>,
    #[serde(default)]
    pub claudex_collaborator_model: Option<String>,
}

pub struct Bridge {
    pub(in crate::anthropic) app: Arc<AgentBackend>,
    pub(in crate::anthropic) model: String,
    pub(in crate::anthropic) legacy_main_route: bool,
    pub(in crate::anthropic) model_catalog: crate::provider_config::ModelCatalog,
    pub(in crate::anthropic) advisor_model_override: Option<String>,
    pub(in crate::anthropic) collaborator_model_override: Option<String>,
    pub(in crate::anthropic) subscription_program: PathBuf,
    pub(in crate::anthropic) settings_path: Option<PathBuf>,
    pub(in crate::anthropic) usage_limit_cache_home: Option<PathBuf>,
    pub(in crate::anthropic) sessions: Mutex<Vec<Arc<Session>>>,
    /// Sessions whose SubAgent response was handed back to the caller while the
    /// provider turn continues in the background. They must remain discoverable
    /// for late Claude tool results, but must not be considered for a new main
    /// turn or hold up active-session matching.
    pub(in crate::anthropic) detached_sessions: Mutex<Vec<Arc<Session>>>,
    pub(in crate::anthropic) session_slots: Arc<Semaphore>,
    pub(in crate::anthropic) next_session_sweep: std::sync::Mutex<Instant>,
    pub(in crate::anthropic) signature_pool: SignaturePool,
    pub(in crate::anthropic) subscription_slots: Arc<Semaphore>,
    pub(in crate::anthropic) subscription_max_processes: usize,
    pub(in crate::anthropic) subscription_timeout: std::time::Duration,
    pub(in crate::anthropic) subagent_hard_timeout: Option<std::time::Duration>,
    #[cfg(test)]
    pub(in crate::anthropic) subagent_hard_timeout_cancel_attempts: std::sync::atomic::AtomicUsize,
    pub(in crate::anthropic) agent_efforts: Arc<agent_effort::AgentEffortIntents>,
    pub(in crate::anthropic) subagent_reuse: Arc<subagent_reuse::SubagentReuseRegistry>,
    pub(in crate::anthropic) tool_schemas: tool_schema_cache::ToolSchemaCache,
    pub(in crate::anthropic) model_concurrency: model_concurrency::ModelConcurrency,
    pub(in crate::anthropic) active_subagent_models:
        Arc<active_subagent_models::ActiveSubagentModels>,
}

pub(crate) struct Session {
    pub(crate) thread_id: String,
    pub(crate) model: String,
    pub(crate) disabled_subagent_models: BTreeSet<String>,
    pub(crate) signature: Arc<str>,
    pub(crate) transcript: Mutex<Vec<Value>>,
    pub(crate) pending_tools: Mutex<HashMap<String, Value>>,
    pub(crate) consumed_tool_ids: Mutex<HashSet<String>>,
    pub(crate) external_tool_names: HashMap<String, String>,
    pub(crate) client_user_id: Option<String>,
    pub(crate) claude_session_id: Option<String>,
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) last_activity: std::sync::Mutex<Instant>,
    pub(crate) pending_since: std::sync::Mutex<Option<Instant>>,
    pub(crate) _slot: OwnedSemaphorePermit,
}

pub(crate) struct SelectedSession {
    pub(crate) session: Arc<Session>,
    pub(crate) existing_len: usize,
    pub(crate) recovered: bool,
    pub(crate) gate: tokio::sync::OwnedMutexGuard<()>,
}

pub(crate) struct ActiveTurn {
    pub(crate) session: Arc<Session>,
    pub(crate) events: Arc<ThreadEvents>,
    pub(crate) response_model: String,
    pub(crate) extras: Vec<Value>,
    pub(crate) routing_system: Value,
    pub(crate) input_tokens: u64,
    pub(crate) retry: Option<ContextRetry>,
    pub(crate) gate: tokio::sync::OwnedMutexGuard<()>,
    pub(crate) detached: bool,
}

#[derive(Clone)]
pub(crate) struct ContextRetry {
    pub(crate) request: MessagesRequest,
    pub(crate) effort: Option<String>,
    pub(crate) advisor_model: Option<String>,
    pub(crate) collaborator_model: Option<String>,
}
