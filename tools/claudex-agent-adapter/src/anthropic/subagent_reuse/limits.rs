use serde_json::Value;

use super::super::MessagesRequest;
use super::store::METADATA_LIMIT_REACHED;

pub(in crate::anthropic) fn max_subagents_per_session() -> usize {
    std::env::var(super::MAX_SUBAGENTS_PER_SESSION_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(super::DEFAULT_MAX_SUBAGENTS_PER_SESSION)
}

pub(in crate::anthropic) fn should_expose_launch_tools(request: &MessagesRequest) -> bool {
    request
        .metadata
        .get(METADATA_LIMIT_REACHED)
        .and_then(Value::as_bool)
        .is_none_or(|reached| !reached)
}

pub(in crate::anthropic) fn is_launch_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

pub(in crate::anthropic) fn reuse_enabled() -> bool {
    match std::env::var(crate::parallel_scheduler::SUBAGENT_REUSE_ENV) {
        Ok(value) => matches!(
            value.as_str(),
            "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => true,
    }
}

pub(in crate::anthropic) fn session_id(request: &MessagesRequest) -> Option<String> {
    super::super::request_identity::claude_session_id(request)
}
