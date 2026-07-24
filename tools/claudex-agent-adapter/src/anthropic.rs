mod agent_effort;
mod agent_routing;
mod content;
mod retention;
mod session;
mod stream;
mod stream_batch;
mod subscription;
mod subscription_frames;
mod subscription_request;
mod subscription_stream;
mod team_protocol;
mod turn_input;

use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Instant,
};

use anyhow::Result;
use axum::{body::Body, http::Response};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::{
    agent_backend::AgentBackend,
    app_server::{AppServer, ThreadEvents},
};

pub use content::{error_response, token_count};
pub(crate) use subscription::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES};

const BRIDGE_INSTRUCTIONS: &str = r"You are the model inside the Claude Code agent harness. Claude Code owns all filesystem, shell, web, MCP, planning, approval, and user-interaction operations. Use only the dynamic tools whose names and schemas were supplied by Claude Code. Do not invoke Codex built-in tools. The Codex app-server sandbox applies only to those disabled built-in tools; never infer from it that Claude Code or its SubAgent tasks are read-only. Preserve task-specific restrictions that the active user or applicable repository instructions explicitly require, but do not copy restrictions from an unrelated earlier task, investigation lane, teammate report, or closed probe. When the active request authorizes implementation, verification, commit, deployment, or another mutation, preserve that authority in SubAgent prompts and act through Claude Code's dynamic tools. Do not add or repeatedly announce read-only, no-edit, no-build, no-deploy, or similar restrictions unless they are explicitly active for the current task. In particular, invoke Claude Code's supplied dynamic SubAgent tool directly (Task in current versions, Agent in older versions); never substitute a Codex collaboration or spawn-agent tool for it. Omit the SubAgent name field for ordinary SubAgents and parallel delegation. Set name only when the active user explicitly supplies that teammate name; an invented name turns the SubAgent into a persistent mailbox teammate and can expose internal agent-message markup. Apply the current selected_workers routing to every Agent or Task launch, including a nested launch from a worker: choose the corresponding claudex worker agent and pass its exact claudex_model and claudex_effort. This routed selection is authoritative, not an inferred default; never use generic claude or blindly inherit the parent provider when the current routing context selects a worker. An exact model explicitly requested by the active user still takes precedence. Provider models whose IDs begin with gpt or grok are supported. If no current routing context or explicit model is available, omit claudex_model so the SubAgent inherits the current session model, and never invent a route. Never claim that delegation occurred or reproduce a requested worker response without an actual SubAgent tool result. Return the answer directly when no Claude Code tool is needed. Treat tool output as the result of your own requested call and continue the same task.";
// Team and background Agent workflows can legitimately keep many tool results pending at once.
// This accepts large bursts while retaining a finite upper bound for transcript memory; idle
// sessions are reclaimed both by TTL and immediately at capacity.
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
    #[serde(default)]
    pub claudex_collaborator_model: Option<String>,
}

pub struct Bridge {
    app: Arc<AgentBackend>,
    model: String,
    advisor_model_override: Option<String>,
    collaborator_model_override: Option<String>,
    subscription_program: PathBuf,
    settings_path: Option<PathBuf>,
    sessions: Mutex<Vec<Arc<Session>>>,
    session_slots: Arc<Semaphore>,
    next_session_sweep: std::sync::Mutex<Instant>,
    signature_pool: SignaturePool,
    subscription_slots: Arc<Semaphore>,
    subscription_max_processes: usize,
    subscription_timeout: std::time::Duration,
    agent_efforts: Arc<agent_effort::AgentEffortIntents>,
}

struct Session {
    thread_id: String,
    model: String,
    signature: Arc<str>,
    transcript: Mutex<Vec<Value>>,
    pending_tools: Mutex<HashMap<String, Value>>,
    consumed_tool_ids: Mutex<HashSet<String>>,
    internal_tools: HashMap<String, String>,
    external_tool_names: HashMap<String, String>,
    client_user_id: Option<String>,
    gate: Arc<Mutex<()>>,
    last_activity: std::sync::Mutex<Instant>,
    pending_since: std::sync::Mutex<Option<Instant>>,
    _slot: OwnedSemaphorePermit,
}

struct Segment {
    blocks: Vec<Value>,
    stop_reason: &'static str,
    usage: Usage,
}

#[derive(Clone, Copy, Default)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

struct SelectedSession {
    session: Arc<Session>,
    existing_len: usize,
    recovered: bool,
    gate: tokio::sync::OwnedMutexGuard<()>,
}

struct ActiveTurn {
    session: Arc<Session>,
    events: ThreadEvents,
    response_model: String,
    extras: Vec<Value>,
    input_tokens: u64,
    gate: tokio::sync::OwnedMutexGuard<()>,
}

impl Bridge {
    pub fn is_alive(&self) -> bool {
        self.app.is_alive()
    }

    pub fn subscription_max_processes(&self) -> usize {
        self.subscription_max_processes
    }

    pub const fn session_capacity(&self) -> usize {
        MAX_SESSIONS
    }

    pub fn used_session_slots(&self) -> usize {
        MAX_SESSIONS - self.session_slots.available_permits()
    }

    pub fn subscription_timeout_minutes(&self) -> u64 {
        self.subscription_timeout.as_secs() / 60
    }

    pub fn backend_routes(&self) -> Vec<String> {
        self.app.route_descriptions()
    }

    pub fn routed_models(&self) -> Vec<String> {
        let models = self.app.models();
        if models.is_empty() {
            vec![self.model.clone()]
        } else {
            models
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        self.app.started_models()
    }

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
    }

    fn build(
        app: Arc<AgentBackend>,
        model: String,
        subscription_program: PathBuf,
        advisor_model_override: Option<String>,
        collaborator_model_override: Option<String>,
        subscription_limits: subscription::SubscriptionLimits,
    ) -> Self {
        Self {
            app,
            model,
            advisor_model_override,
            collaborator_model_override,
            subscription_program,
            settings_path: subscription::claude_settings_path(),
            sessions: Mutex::new(Vec::new()),
            session_slots: Arc::new(Semaphore::new(MAX_SESSIONS)),
            next_session_sweep: std::sync::Mutex::new(
                Instant::now() + retention::SESSION_SWEEP_INTERVAL,
            ),
            signature_pool: StdMutex::new(HashMap::new()),
            subscription_slots: Arc::new(Semaphore::new(subscription_limits.max_processes)),
            subscription_max_processes: subscription_limits.max_processes,
            subscription_timeout: subscription_limits.timeout,
            agent_efforts: Arc::new(agent_effort::AgentEffortIntents::default()),
        }
    }

    /// Overrides the Claude Code settings source, primarily for isolated runtimes and tests.
    #[must_use]
    pub fn with_settings_path(self, settings_path: impl Into<PathBuf>) -> Self {
        Self {
            settings_path: Some(settings_path.into()),
            ..self
        }
    }

    pub async fn messages(
        self: &Arc<Self>,
        mut request: MessagesRequest,
    ) -> Result<Response<Body>> {
        trace_request(&request);
        self.sweep_idle_sessions().await;
        let intent = self.agent_efforts.take(&request);
        let is_subagent = intent.is_subagent;
        if is_subagent {
            request.model = intent.model_override.unwrap_or_else(|| self.model.clone());
        }
        let effort = self.resolve_request_effort(&request, intent.effort);
        tracing::debug!(
            request_model = %request.model,
            request_effort = ?effort,
            is_subagent,
            "resolved request routing"
        );
        if !request.model.is_empty()
            && request.model != self.model
            && !self.app.supports_model(&request.model)
        {
            return self
                .subscription_messages(request, effort, is_subagent)
                .await;
        }

        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        // Open the SSE body before prepare_turn so Claude Code receives
        // message_start + ping keepalives while session/provider startup runs.
        // Waiting until prepare_turn finishes is what produced 5-minute
        // "operation timed out" / "Response stalled mid-stream" errors.
        if request.stream {
            return Ok(self.streaming_messages(request, input_tokens, effort));
        }
        let turn = self.prepare_turn(&request, input_tokens, effort).await?;
        self.non_streaming_response(turn).await
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
