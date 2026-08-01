use std::{path::PathBuf, sync::Arc, time::Duration};

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
}

impl SubscriptionOptions {
    #[cfg(test)]
    pub(in crate::anthropic) fn internal(slots: Arc<Semaphore>, timeout: Duration) -> Self {
        Self {
            effort: None,
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
