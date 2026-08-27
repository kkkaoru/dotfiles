use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Instant,
};

use anyhow::Result;
use tokio::sync::{Mutex, Semaphore};

use super::{
    Bridge, MAX_SESSIONS, active_subagent_models, agent_effort, dynamic_effort, model_concurrency,
    retention, subagent_reuse, subagent_timeout, subscription, tool_schema_cache,
};
use crate::{agent_backend::AgentBackend, app_server::AppServer};

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
            subagent_reuse: Arc::new(subagent_reuse::SubagentReuseRegistry::persistent()),
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
            usage_limit_cache_home: None,
            sessions: Mutex::new(Vec::new()),
            detached_sessions: Mutex::new(Vec::new()),
            dynamic_effort: dynamic_effort::DynamicEffortManager::from_environment(),
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
            subagent_reuse: Arc::new(subagent_reuse::SubagentReuseRegistry::default()),
            tool_schemas: tool_schema_cache::ToolSchemaCache::default(),
            model_concurrency,
            active_subagent_models: Arc::new(
                active_subagent_models::ActiveSubagentModels::default(),
            ),
        }
    }
    fn with_legacy_main_route(mut self) -> Self {
        self.legacy_main_route = true;
        self
    }
}
