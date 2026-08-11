mod active_subagent_models;
mod agent_batch;
mod agent_effort;
mod agent_effort_matching;
mod agent_intent_store;
mod agent_route_validation;
mod agent_routing;
mod async_agent_handoff;
pub(crate) use async_agent_handoff::{agent_tool_round_ids, exact_async_launch_acknowledgement};
mod config;
mod content;
mod content_batch;
mod content_pending;
mod error;
mod exhausted_subagent;
mod health;
mod internal_notification;
mod message_router;
mod message_router_dispatch;
mod model_concurrency;
mod pasted_text;
mod request_identity;
mod request_routing;
pub(crate) use request_routing::official_claude_haiku_model;
mod retention;
mod routing_quota;
mod segment;
mod session;
mod stream;
mod stream_batch;
mod subagent_continuation;
mod subagent_reuse;
mod subagent_timeout;
mod task_ids;
// Runtime/daemon option plumbing imports these names once normalized CLI
// configuration is installed on the Bridge. Keep the shared literals available
// while individual library test targets compile without that plumbing.
#[allow(unused_imports)]
pub(crate) use subagent_timeout::{
    LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV, SUBAGENT_HARD_TIMEOUT_ENV,
};
mod provider_auth;
mod provider_auth_cooldown;
mod subscription;
mod subscription_activity;
mod subscription_frames;
mod subscription_oauth;
pub(crate) mod subscription_request;
mod subscription_stream;
mod team_protocol;
mod tool_schema_cache;
mod turn_input;
mod usage_limit_cooldown;
mod usage_limit_failover;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Instant,
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

pub(super) struct AgentEffortRecord<'a> {
    pub(super) client_user_id: Option<&'a str>,
    pub(super) tool_name: &'a str,
    pub(super) tool_use_id: String,
    pub(super) parent_model: &'a str,
    pub(super) arguments: &'a Value,
    pub(super) user_messages: &'a [Value],
    pub(super) system: &'a Value,
}

use crate::{
    agent_backend::AgentBackend,
    app_server::ThreadEvents,
};

pub use content::{error_response, token_count};
pub use request_identity::RequestIdentity;
use segment::{Segment, Usage, WebEvidenceSummary};
pub(crate) use subscription::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES};

mod bridge_instructions;
mod bridge_helpers;
use bridge_instructions::{
    BRIDGE_INSTRUCTIONS, CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS, SUBAGENT_RESULT_PROTOCOL,
};
use bridge_helpers::{intern_signature, trace_request};

const MAX_SESSIONS: usize = 1_024;
pub(super) const MAX_SIGNATURE_BUCKETS: usize = MAX_SESSIONS * 2;
pub(super) type SignaturePool = StdMutex<HashMap<u64, Vec<Weak<str>>>>;
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
    app: Arc<AgentBackend>,
    model: String,
    legacy_main_route: bool,
    model_catalog: crate::provider_config::ModelCatalog,
    advisor_model_override: Option<String>,
    collaborator_model_override: Option<String>,
    subscription_program: PathBuf,
    settings_path: Option<PathBuf>,
    usage_limit_cache_home: Option<PathBuf>,
    sessions: Mutex<Vec<Arc<Session>>>,
    /// Sessions whose SubAgent response was handed back to the caller while the
    /// provider turn continues in the background. They must remain discoverable
    /// for late Claude tool results, but must not be considered for a new main
    /// turn or hold up active-session matching.
    detached_sessions: Mutex<Vec<Arc<Session>>>,
    session_slots: Arc<Semaphore>,
    next_session_sweep: std::sync::Mutex<Instant>,
    signature_pool: SignaturePool,
    subscription_slots: Arc<Semaphore>,
    subscription_max_processes: usize,
    subscription_timeout: std::time::Duration,
    subagent_hard_timeout: Option<std::time::Duration>,
    #[cfg(test)]
    subagent_hard_timeout_cancel_attempts: std::sync::atomic::AtomicUsize,
    agent_efforts: Arc<agent_effort::AgentEffortIntents>,
    subagent_reuse: Arc<subagent_reuse::SubagentReuseRegistry>,
    tool_schemas: tool_schema_cache::ToolSchemaCache,
    model_concurrency: model_concurrency::ModelConcurrency,
    active_subagent_models: Arc<active_subagent_models::ActiveSubagentModels>,
}

struct Session {
    thread_id: String,
    model: String,
    disabled_subagent_models: BTreeSet<String>,
    signature: Arc<str>,
    transcript: Mutex<Vec<Value>>,
    pending_tools: Mutex<HashMap<String, Value>>,
    consumed_tool_ids: Mutex<HashSet<String>>,
    external_tool_names: HashMap<String, String>,
    client_user_id: Option<String>,
    claude_session_id: Option<String>,
    gate: Arc<Mutex<()>>,
    last_activity: std::sync::Mutex<Instant>,
    pending_since: std::sync::Mutex<Option<Instant>>,
    _slot: OwnedSemaphorePermit,
}

struct SelectedSession {
    session: Arc<Session>,
    existing_len: usize,
    recovered: bool,
    gate: tokio::sync::OwnedMutexGuard<()>,
}

struct ActiveTurn {
    session: Arc<Session>,
    events: Arc<ThreadEvents>,
    response_model: String,
    extras: Vec<Value>,
    routing_system: Value,
    input_tokens: u64,
    retry: Option<ContextRetry>,
    gate: tokio::sync::OwnedMutexGuard<()>,
    detached: bool,
}

#[derive(Clone)]
struct ContextRetry {
    request: MessagesRequest,
    effort: Option<String>,
    advisor_model: Option<String>,
    collaborator_model: Option<String>,
}

mod bridge_ctors;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod subscription_tests;
#[cfg(test)]
mod tests;
