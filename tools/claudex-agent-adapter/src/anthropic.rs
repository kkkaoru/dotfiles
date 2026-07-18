mod agent_effort;
mod content;
mod retention;
mod session;
mod stream;
mod stream_batch;
mod subscription;
mod subscription_stream;
mod team_protocol;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
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

const BRIDGE_INSTRUCTIONS: &str = r"You are the model inside the Claude Code agent harness. Claude Code owns all filesystem, shell, web, MCP, planning, approval, and user-interaction operations. Use only the dynamic tools whose names and schemas were supplied by Claude Code. Do not invoke Codex built-in tools. In particular, invoke Claude Code's supplied dynamic Agent tool directly; never substitute a Codex collaboration or spawn-agent tool for it. When the user specifies effort for a SubAgent, set that Agent call's claudex_effort field; map mid to medium. This controls only that SubAgent and must not change the main turn's effort. Omit claudex_effort when the user did not specify it. When the user explicitly specifies a SubAgent model, put its exact model ID in claudex_model. Provider models whose IDs begin with gpt or grok are supported. Otherwise omit claudex_model so the SubAgent inherits the current session model. Never infer a default SubAgent model. Return the answer directly when no Claude Code tool is needed. Treat tool output as the result of your own requested call and continue the same task.";
// Team and background Agent workflows can legitimately keep dozens of tool results pending at
// once. Four times the former limit accepts those bursts while retaining a finite upper bound for
// transcript memory; idle sessions are reclaimed both by TTL and immediately at capacity.
const MAX_SESSIONS: usize = 256;

#[derive(Debug, Deserialize)]
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
    subscription_slots: Arc<Semaphore>,
    subscription_max_processes: usize,
    subscription_timeout: std::time::Duration,
    agent_efforts: agent_effort::AgentEffortIntents,
}

struct Session {
    thread_id: String,
    model: String,
    signature: String,
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
            subscription_slots: Arc::new(Semaphore::new(subscription_limits.max_processes)),
            subscription_max_processes: subscription_limits.max_processes,
            subscription_timeout: subscription_limits.timeout,
            agent_efforts: agent_effort::AgentEffortIntents::default(),
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
        if intent.is_subagent {
            request.model = intent.model_override.unwrap_or_else(|| self.model.clone());
        }
        let effort = self.resolve_request_effort(&request, intent.effort);
        if !request.model.is_empty()
            && request.model != self.model
            && !self.app.supports_model(&request.model)
        {
            return self.subscription_messages(request, effort).await;
        }

        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        let turn = self.prepare_turn(&request, input_tokens, effort).await?;
        if request.stream {
            return Ok(self.streaming_response(turn));
        }
        self.non_streaming_response(turn).await
    }

    fn request_model(&self, request: &MessagesRequest) -> String {
        if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        }
    }
}

fn trace_request(request: &MessagesRequest) {
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
