use serde_json::Value;

use super::{looks_like_launch_arguments, looks_like_launch_tool};

/// Tool labels only. Full shell titles (`schtasks`, "Search local agent history")
/// must not count as Agent/Task just because they contain those substrings.
pub(super) fn launch_name_candidates<'a>(tool: &'a str, title: &'a str) -> Vec<&'a str> {
    let mut labels = Vec::new();
    if !tool.is_empty() {
        labels.push(tool);
    }
    if !title.is_empty() && is_compact_tool_label(title) {
        labels.push(title);
    }
    labels
}

pub(super) fn is_compact_tool_label(candidate: &str) -> bool {
    let trimmed = candidate.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 64
        && !trimmed.contains('\n')
        && !trimmed.starts_with('`')
        && (!trimmed.chars().any(char::is_whitespace) || looks_like_launch_tool(trimmed))
}

pub(super) fn looks_like_mcp_surface(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower == "mcp"
        || lower.starts_with("mcp:")
        || lower.starts_with("mcp ")
        || lower.contains("claudex-launch")
        || (lower.contains("mcp") && (lower.contains("agent") || lower.contains("task")))
}

pub(super) fn trace_launch_shaped_event(event: &Value) {
    let Some(params) = event.get("params") else {
        return;
    };
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let status = params.get("status").and_then(Value::as_str).unwrap_or("");
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let interesting = [tool, title].into_iter().any(|candidate| {
        looks_like_launch_tool(candidate) || looks_like_mcp_surface(candidate) || {
            let lower = candidate.to_ascii_lowercase();
            // Exact / token matches only — "include-subagents" in a Bash
            // title must not count as an Agent/Task launch card.
            lower == "agent"
                || lower == "task"
                || lower.starts_with("agent ")
                || lower.starts_with("task ")
                || lower.starts_with("agent:")
                || lower.starts_with("task:")
        }
    }) || looks_like_launch_arguments(raw_args);
    if !interesting {
        return;
    }
    let arg_keys = raw_args
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let acp_method = event.get("method").and_then(Value::as_str).unwrap_or("");
    tracing::info!(
        acp_method,
        tool,
        title,
        status,
        ?arg_keys,
        "ACP providerTool launch-shaped event"
    );
}
