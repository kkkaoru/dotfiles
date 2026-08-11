use std::{collections::VecDeque, sync::Mutex, time::Instant};

use serde_json::Value;

mod background_launch;
mod intents;
mod model;
mod names;
mod prepare;
mod terminal;
pub(in crate::anthropic) use super::agent_route_validation::BLOCKED_SUBAGENT_NOTICE;
#[cfg(test)]
pub(super) use super::agent_route_validation::validate_routed_agent_arguments;
pub(super) use super::agent_route_validation::validate_routed_agent_arguments_with_catalog;
#[cfg(test)]
use intents::retain_terminal_intent;
pub(super) use model::{disabled_subagent_model, is_agent_tool, requested_model};
#[cfg(test)]
pub(in crate::anthropic) use prepare::prepare_arguments;
pub(in crate::anthropic) use prepare::prepare_arguments_for_user;

pub(super) use super::AgentEffortRecord;
pub(super) use super::agent_effort_matching::is_subagent_request;
pub(super) use super::agent_intent_store::remove_expired;
use super::subscription::valid_effort;

pub(super) const INTENT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
pub(super) const MAX_PENDING_INTENTS: usize = 1_024;
pub(super) const ADAPTER_EFFORT: &str = "claudex_effort";
pub(super) const ADAPTER_MODEL: &str = "claudex_model";
pub(super) const IMPLICIT_MODEL: &str = "claudex_implicit_model";
#[derive(Clone)]
pub(super) struct AgentEffortIntent {
    pub(super) client_user_id: Option<String>,
    pub(super) prompt: String,
    pub(super) correlated: bool,
    pub(super) effort: Option<String>,
    pub(super) model_override: Option<String>,
    pub(super) model_is_inherited: bool,
    pub(super) run_in_background: bool,
    pub(super) tool_use_id: String,
    pub(super) created_at: Instant,
    pub(super) created_unix_seconds: u64,
}
pub(super) struct AgentEffortIntents {
    pub(super) pending: Mutex<VecDeque<AgentEffortIntent>>,
    pub(super) store: Option<super::agent_intent_store::AgentIntentStore>,
}
pub(super) enum AgentEffort {
    Unmatched,
    ConfiguredDefault,
    Explicit(String),
}
pub(super) struct AgentIntent {
    pub(super) effort: AgentEffort,
    pub(super) model_override: Option<String>,
    pub(super) model_is_inherited: bool,
    pub(super) run_in_background: bool,
    pub(super) is_subagent: bool,
    pub(super) matched: bool,
}
impl AgentIntent {
    pub(super) fn unmatched(is_subagent: bool) -> Self {
        Self {
            effort: AgentEffort::Unmatched,
            model_override: None,
            model_is_inherited: false,
            run_in_background: false,
            is_subagent,
            matched: false,
        }
    }
}

pub(super) fn agent_prompt<'a>(tool_name: &str, arguments: &'a Value) -> Option<&'a str> {
    is_agent_tool(tool_name)
        .then(|| arguments.get("prompt").and_then(Value::as_str))
        .flatten()
}

#[cfg(test)]
fn tool_schema(_tool_name: &str, schema: Value) -> Value {
    schema
}

pub(super) fn normalized_effort(value: &str) -> Option<&str> {
    let normalized = if value == "mid" { "medium" } else { value };
    valid_effort(normalized).then_some(normalized)
}

#[cfg(test)]
include!("agent_effort_tests.rs");
