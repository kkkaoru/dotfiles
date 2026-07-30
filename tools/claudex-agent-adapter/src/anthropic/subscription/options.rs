use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::Value;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub(in crate::anthropic) struct SubscriptionOptions {
    pub(in crate::anthropic) effort: Option<String>,
    pub(in crate::anthropic) tools: Vec<String>,
    pub(in crate::anthropic) bridge_tools: bool,
    pub(in crate::anthropic) cwd: Option<PathBuf>,
    pub(in crate::anthropic) slots: Arc<Semaphore>,
    pub(in crate::anthropic) timeout: Duration,
    pub(in crate::anthropic) tool_context: Option<SubscriptionToolContext>,
}

#[derive(Clone)]
pub(in crate::anthropic) struct SubscriptionToolContext {
    pub(in crate::anthropic) agent_efforts: Arc<super::super::agent_effort::AgentEffortIntents>,
    pub(in crate::anthropic) model_catalog: crate::provider_config::ModelCatalog,
    pub(in crate::anthropic) child_executor: Option<SubscriptionChildExecutor>,
    pub(in crate::anthropic) client_user_id: Option<String>,
    pub(in crate::anthropic) parent_model: String,
    pub(in crate::anthropic) user_messages: Vec<Value>,
    pub(in crate::anthropic) system: Value,
}

#[derive(Clone)]
pub(in crate::anthropic) struct SubscriptionChildExecutor {
    pub(in crate::anthropic) program: PathBuf,
    pub(in crate::anthropic) slots: Arc<Semaphore>,
    pub(in crate::anthropic) timeout: Duration,
    pub(in crate::anthropic) cwd: Option<PathBuf>,
    pub(in crate::anthropic) tools: Vec<String>,
}

impl SubscriptionOptions {
    pub(in crate::anthropic) fn internal(slots: Arc<Semaphore>, timeout: Duration) -> Self {
        Self {
            effort: None,
            tools: Vec::new(),
            bridge_tools: true,
            cwd: None,
            slots,
            timeout,
            tool_context: None,
        }
    }
}
