use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use serde_json::Value;
use tokio::sync::Semaphore;

#[cfg(test)]
use crate::anthropic::subscription_stream::{ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY};

// A healthy pipe reaches EOF immediately after the last writer exits. One
// second permits normal descriptor teardown without retaining an orphaned
// process group when a descendant accidentally keeps stderr open.
pub(in crate::anthropic) const DEFAULT_STDERR_DRAIN_GRACE: Duration = Duration::from_secs(1);
// SIGKILL normally makes wait(2) immediately reapable. Keep a finite five
// second ceiling for abnormal kernels/process supervisors instead of waiting
// forever during response cleanup.
pub(in crate::anthropic) const DEFAULT_TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::anthropic) struct SubscriptionOptions {
    pub(in crate::anthropic) effort: Option<String>,
    pub(in crate::anthropic) is_subagent: bool,
    pub(in crate::anthropic) tools: Vec<String>,
    pub(in crate::anthropic) disable_tools: bool,
    pub(in crate::anthropic) json_schema: Option<String>,
    pub(in crate::anthropic) cwd: Option<PathBuf>,
    pub(in crate::anthropic) slots: Arc<Semaphore>,
    pub(in crate::anthropic) timeout: Duration,
    pub(in crate::anthropic) initial_activity_delay: Duration,
    pub(in crate::anthropic) activity_keepalive_interval: Duration,
    pub(in crate::anthropic) stderr_drain_grace: Duration,
    pub(in crate::anthropic) termination_timeout: Duration,
    pub(in crate::anthropic) tool_context: Option<SubscriptionToolContext>,
}

#[derive(Clone)]
pub(in crate::anthropic) struct SubscriptionToolContext {
    pub(in crate::anthropic) agent_efforts: Arc<crate::anthropic::agent_effort::AgentEffortIntents>,
    pub(in crate::anthropic) model_catalog: crate::provider_config::ModelCatalog,
    pub(in crate::anthropic) client_user_id: Option<String>,
    pub(in crate::anthropic) parent_model: String,
    pub(in crate::anthropic) user_messages: Vec<Value>,
    pub(in crate::anthropic) system: Value,
    pub(in crate::anthropic) session_id: Option<String>,
    pub(in crate::anthropic) subagent_reuse:
        Arc<crate::anthropic::subagent_reuse::SubagentReuseRegistry>,
    pub(in crate::anthropic) auth_cache: Option<PathBuf>,
    pub(in crate::anthropic) disabled_subagent_models: BTreeSet<String>,
}

impl SubscriptionToolContext {
    #[cfg(test)]
    pub(in crate::anthropic) fn for_tests(
        agent_efforts: Arc<crate::anthropic::agent_effort::AgentEffortIntents>,
        model_catalog: crate::provider_config::ModelCatalog,
        client_user_id: Option<String>,
        parent_model: impl Into<String>,
        user_messages: Vec<Value>,
        system: Value,
    ) -> Self {
        Self {
            agent_efforts,
            model_catalog,
            client_user_id,
            parent_model: parent_model.into(),
            user_messages,
            system,
            session_id: None,
            subagent_reuse: Arc::new(
                crate::anthropic::subagent_reuse::SubagentReuseRegistry::default(),
            ),
            auth_cache: None,
            disabled_subagent_models: BTreeSet::new(),
        }
    }

    pub(in crate::anthropic) fn launch_model_is_exhausted(&self, model: &str) -> bool {
        let now = SystemTime::now();
        let cache = self.auth_cache.as_deref();
        if crate::anthropic::provider_auth_cooldown::scope_is_cooling_down_at(cache, model, now) {
            return true;
        }
        if self
            .model_catalog
            .usage_provider_for_model(model)
            .is_some_and(|provider| {
                crate::anthropic::provider_auth_cooldown::scope_is_cooling_down_at(
                    cache, provider, now,
                )
            })
        {
            return true;
        }
        let routing_cache = cache.map(|path| path.with_file_name("usage-routing.json"));
        crate::anthropic::routing_quota::live_cache_marks_model_exhausted(
            routing_cache.as_deref(),
            model,
            now,
        )
    }
}

impl SubscriptionOptions {
    #[cfg(test)]
    pub(in crate::anthropic) fn internal(slots: Arc<Semaphore>, timeout: Duration) -> Self {
        Self {
            effort: None,
            is_subagent: false,
            tools: Vec::new(),
            disable_tools: false,
            json_schema: None,
            cwd: None,
            slots,
            timeout,
            initial_activity_delay: INITIAL_ACTIVITY_DELAY,
            activity_keepalive_interval: ACTIVITY_KEEPALIVE_INTERVAL,
            stderr_drain_grace: DEFAULT_STDERR_DRAIN_GRACE,
            termination_timeout: DEFAULT_TERMINATION_TIMEOUT,
            tool_context: None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::provider_config::ModelCatalog;

    #[test]
    fn launch_model_is_exhausted_when_auth_scope_is_cooling_down() {
        let root = tempfile::tempdir().expect("auth cooldown fixture");
        let cache = crate::anthropic::provider_auth_cooldown::cache_path_for_home(root.path());
        crate::anthropic::provider_auth_cooldown::record_at(
            Some(cache.as_path()),
            "auto",
            "auth expired",
            SystemTime::now(),
        )
        .expect("record cooldown");
        let mut context = SubscriptionToolContext::for_tests(
            Arc::new(crate::anthropic::agent_effort::AgentEffortIntents::default()),
            ModelCatalog::default(),
            None,
            "main",
            Vec::new(),
            Value::Null,
        );
        context.auth_cache = Some(cache);
        assert!(context.launch_model_is_exhausted("auto"));
        assert!(!context.launch_model_is_exhausted("fresh-model"));
    }
}
