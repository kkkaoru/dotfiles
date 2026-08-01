mod agent_batch;
mod agent_effort;
mod agent_effort_matching;
mod agent_intent_store;
mod agent_routing;
mod async_agent_handoff;
pub(crate) use async_agent_handoff::{exact_async_launch_acknowledgement, tool_round_ids};
mod content;
mod content_batch;
mod content_pending;
mod error;
mod health;
mod message_router;
mod model_concurrency;
mod request_identity;
mod request_routing;
mod retention;
mod segment;
mod session;
mod stream;
mod stream_batch;
mod subagent_continuation;
mod subagent_timeout;
// Runtime/daemon option plumbing imports these names once normalized CLI
// configuration is installed on the Bridge. Keep the shared literals available
// while individual library test targets compile without that plumbing.
#[allow(unused_imports)]
pub(crate) use subagent_timeout::{
    LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV, SUBAGENT_HARD_TIMEOUT_ENV,
};
mod subagent_visibility;
mod subscription;
mod subscription_activity;
mod subscription_frames;
pub(crate) mod subscription_request;
mod subscription_stream;
mod team_protocol;
mod tool_schema_cache;
mod turn_input;
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
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
    app_server::{AppServer, ThreadEvents},
};

pub use content::{error_response, token_count};
pub use request_identity::RequestIdentity;
use segment::{Segment, Usage, WebEvidenceSummary};
pub(crate) use subscription::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES};

const BRIDGE_INSTRUCTIONS: &str = r"You are the model inside the Claude Code agent harness. Claude Code owns all filesystem, shell, web, MCP, planning, approval, and user-interaction operations. Use only the dynamic tools whose names and schemas were supplied by Claude Code. When the active request explicitly requires live WebSearch, WebFetch, or actual page retrieval, invoke the supplied web tool and do not substitute memory, a guessed URL, or a URL copied from prose; claim search success only after a tool result, and report the tool as unavailable when no web tool is supplied. Do not invoke Codex built-in tools. The Codex app-server sandbox applies only to those disabled built-in tools; never infer from it that Claude Code or its SubAgent tasks are read-only. Preserve task-specific restrictions that the active user or applicable repository instructions explicitly require, but do not copy restrictions from an unrelated earlier task, investigation lane, teammate report, or closed probe. When the active request authorizes implementation, verification, commit, deployment, or another mutation, preserve that authority in SubAgent prompts and act through Claude Code's dynamic tools. Do not add or repeatedly announce read-only, no-edit, no-build, no-deploy, or similar restrictions unless they are explicitly active for the current task. In particular, invoke Claude Code's supplied dynamic SubAgent tool directly (Task in current versions, Agent in older versions); never substitute a Codex collaboration or spawn-agent tool for it. Omit the SubAgent name field for ordinary SubAgents and parallel delegation. Set name only when the active user explicitly supplies that teammate name; an invented name turns the SubAgent into a persistent mailbox teammate and can expose internal agent-message markup. Use only fields present in the exact Agent or Task schema supplied by Claude Code. The adapter correlates selected_workers routing outside the public schema; never invent adapter-only claudex_model or claudex_effort arguments when those fields are absent. When multiple independent workers are requested, emit one ordinary supplied Agent/Task tool call per intended worker in the same assistant message and tool round. Never invent or request an adapter-only batch tool. Do not announce a worker count unless that same response contains exactly that many native launch calls. Avoid serial heavy processing by one worker when capacity allows multi-worker fan-out. When results are needed in the current turn, use foreground launches. Use background launches only when a concrete independent next action is already identified and will start immediately, or the task must outlive the turn. After successful background launches, start that action or end the turn promptly with concise user-visible status; never keep reasoning while waiting for completion notifications. For related follow-ups, reuse compatible workers with SendMessage and the exact prior Agent/Task recipient instead of churning processes with fresh launches. Do not SendMessage merely to repeat scope or restrictions already present in the original delegation. A follow-up queued to a busy worker does not add parallel capacity; assign genuinely independent work to another routed worker when useful capacity exists. Treat disabled_subagent_models in the current routing context as an absolute SubAgent denylist across explicit selection, inheritance, nested launches, and reuse. This routed selection is authoritative, not an inferred default; never use generic claude or blindly inherit the parent provider when the current routing context selects a worker. An exact model explicitly requested by the active user still takes precedence only when it is not disabled. Provider models selected by the current routing context are supported. If no current routing context or explicit model is available, do not launch a SubAgent; report the missing route clearly instead of inheriting the current session model or inventing a route. Never claim that delegation occurred or reproduce a requested worker response without an actual SubAgent tool result. Return the answer directly when no Claude Code tool is needed. Treat tool output as the result of your own requested call and continue the same task.";
const CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS: &str = r"In Code tasks, avoid serializing independent operations. For each bounded stage, run independent calls, fetches, or checks in parallel and await them as one batch (for JavaScript, `Promise.all` / `Promise.allSettled` patterns, or the language-equivalent concurrency primitive). Keep sequential order only when output dependencies or side effects require it. This helps reduce unnecessary latency and aligns with the Codex parallel execution guidance.";
const MAX_SESSIONS: usize = 1_024;
const MAX_SIGNATURE_BUCKETS: usize = MAX_SESSIONS * 2;
type SignaturePool = StdMutex<HashMap<u64, Vec<Weak<str>>>>;
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
    tool_schemas: tool_schema_cache::ToolSchemaCache,
    model_concurrency: model_concurrency::ModelConcurrency,
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

impl Bridge {
    pub fn new(app: Arc<AppServer>, model: String) -> Self {
        Self::new_with_subscription_program(app, model, "claude")
    }

    pub fn new_with_backend(app: Arc<AgentBackend>, model: String) -> Self {
        Self::build(
            app,
            model,
            "claude".into(),
            None,
            None,
            subscription::subscription_limits(),
        )
        .with_legacy_main_route()
    }

    pub(crate) fn new_with_backend_limits(
        app: Arc<AgentBackend>,
        model: String,
        max_processes: usize,
        timeout_minutes: u64,
    ) -> Result<Self> {
        Ok(Self::build(
            app,
            model,
            "claude".into(),
            None,
            None,
            subscription::SubscriptionLimits::new(max_processes, timeout_minutes)?,
        ))
    }

    /// Retain live Claude Code Agent/Task launch correlations across adapter handover.
    #[must_use]
    pub(crate) fn with_persisted_agent_intents(self) -> Self {
        Self {
            agent_efforts: Arc::new(agent_effort::AgentEffortIntents::persistent()),
            tool_schemas: tool_schema_cache::ToolSchemaCache::persistent(),
            ..self
        }
    }

    pub fn new_with_subscription_program(
        app: Arc<AppServer>,
        model: String,
        subscription_program: impl Into<PathBuf>,
    ) -> Self {
        Self::new_with_subscription_program_and_models(app, model, subscription_program, None, None)
    }

    pub fn new_with_subscription_program_and_models(
        app: Arc<AppServer>,
        model: String,
        subscription_program: impl Into<PathBuf>,
        advisor_model_override: Option<String>,
        collaborator_model_override: Option<String>,
    ) -> Self {
        let subscription_limits = subscription::subscription_limits();
        Self::build(
            AgentBackend::codex(app),
            model,
            subscription_program.into(),
            advisor_model_override,
            collaborator_model_override,
            subscription_limits,
        )
        .with_legacy_main_route()
    }

    fn build(
        app: Arc<AgentBackend>,
        model: String,
        subscription_program: PathBuf,
        advisor_model_override: Option<String>,
        collaborator_model_override: Option<String>,
        subscription_limits: subscription::SubscriptionLimits,
    ) -> Self {
        let model_concurrency =
            model_concurrency::ModelConcurrency::new(app.configured_concurrency_limits());
        Self {
            app,
            model,
            legacy_main_route: false,
            model_catalog: crate::provider_config::ModelCatalog::default(),
            advisor_model_override,
            collaborator_model_override,
            subscription_program,
            settings_path: subscription::claude_settings_path(),
            sessions: Mutex::new(Vec::new()),
            detached_sessions: Mutex::new(Vec::new()),
            session_slots: Arc::new(Semaphore::new(MAX_SESSIONS)),
            next_session_sweep: std::sync::Mutex::new(
                Instant::now() + retention::SESSION_SWEEP_INTERVAL,
            ),
            signature_pool: StdMutex::new(HashMap::new()),
            subscription_slots: Arc::new(Semaphore::new(subscription_limits.max_processes)),
            subscription_max_processes: subscription_limits.max_processes,
            subscription_timeout: subscription_limits.timeout,
            subagent_hard_timeout: subagent_timeout::subagent_hard_timeout(),
            #[cfg(test)]
            subagent_hard_timeout_cancel_attempts: std::sync::atomic::AtomicUsize::new(0),
            agent_efforts: Arc::new(agent_effort::AgentEffortIntents::default()),
            tool_schemas: tool_schema_cache::ToolSchemaCache::default(),
            model_concurrency,
        }
    }
    fn with_legacy_main_route(mut self) -> Self {
        self.legacy_main_route = true;
        self
    }

    /// Overrides the Claude Code settings source, primarily for isolated runtimes and tests.
    #[must_use]
    pub fn with_settings_path(self, settings_path: impl Into<PathBuf>) -> Self {
        Self {
            settings_path: Some(settings_path.into()),
            ..self
        }
    }

    /// Install the normalized hard timeout used only for native background
    /// SubAgent requests. `None` deliberately leaves the Claude Code Agent
    /// lifecycle unbounded by claudex.
    #[must_use]
    pub(crate) fn with_subagent_hard_timeout(
        self,
        subagent_hard_timeout: Option<std::time::Duration>,
    ) -> Self {
        Self {
            subagent_hard_timeout,
            ..self
        }
    }

    pub(crate) fn subagent_hard_timeout_seconds(&self) -> Option<u64> {
        self.subagent_hard_timeout.map(|timeout| timeout.as_secs())
    }

    /// Install the config-declared model catalog used for unrouted-provider remaps.
    #[must_use]
    pub fn with_model_catalog(self, model_catalog: crate::provider_config::ModelCatalog) -> Self {
        Self {
            model_catalog,
            ..self
        }
    }

    pub(super) fn request_model(&self, request: &MessagesRequest) -> String {
        if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        }
    }

    fn intern_signature(&self, signature: String) -> Arc<str> {
        intern_signature(&self.signature_pool, signature)
    }
}

fn intern_signature(pool: &SignaturePool, signature: String) -> Arc<str> {
    let mut hasher = DefaultHasher::new();
    signature.hash(&mut hasher);
    let mut pool = pool.lock().expect("signature pool poisoned");
    if pool.len() >= MAX_SIGNATURE_BUCKETS {
        pool.retain(|_, candidates| {
            candidates.retain(|candidate| candidate.strong_count() > 0);
            !candidates.is_empty()
        });
    }
    let candidates = pool.entry(hasher.finish()).or_default();
    let mut matched = None;
    candidates.retain(|candidate| {
        let Some(candidate) = candidate.upgrade() else {
            return false;
        };
        if candidate.as_ref() == signature {
            matched = Some(candidate);
        }
        true
    });
    matched.unwrap_or_else(|| {
        let signature = Arc::<str>::from(signature);
        candidates.push(Arc::downgrade(&signature));
        signature
    })
}

fn trace_request(request: &MessagesRequest) -> bool {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return false;
    }
    tracing::debug!(
        request_model = %request.model,
        stream = request.stream,
        system_bytes = serialized_len(&request.system),
        message_bytes = serialized_len(&request.messages),
        tool_count = request.tools.len(),
        tool_bytes = serialized_len(&request.tools),
        output_config = %request.output_config,
        "received Claude Code Messages request"
    );
    true
}

fn serialized_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod subscription_tests;
#[cfg(test)]
mod tests;
