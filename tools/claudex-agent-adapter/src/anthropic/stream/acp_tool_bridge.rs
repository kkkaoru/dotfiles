//! ACP provider tools → Claude Code `tool_use` only for client-executed tools.
//!
//! Provider-native tools (bash/read/edit/…) stay as WIP progress. Bridging them would
//! double-execute under Claude Code. Launch tools (`Agent`/`Task`) and Grok-native
//! `spawn_subagent` (mapped onto Agent when Claude Code supplied Agent) become
//! Anthropic `tool_use`.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::ToolCall;

/// Pending-tool request id marker: ACP cannot accept Codex-style tool results on the
/// app-server channel; follow-up turns continue via transcript + `turn/start`.
pub(super) const ACP_BRIDGE_MARKER: &str = "acpBridge";

const SPAWN_SUBAGENT: &str = "spawn_subagent";
const GROK_HIGH_PROFILE: &str = "grok-native-high-plugin-v3:claudex-high";
const PROMPT_ALIASES: &[&str] = &["prompt", "task", "message", "instruction", "query", "input"];
const DESCRIPTION_ALIASES: &[&str] = &["description", "title", "name", "summary", "subject"];

pub(super) fn is_client_executed_bridge_tool(original_name: &str) -> bool {
    matches!(original_name, "Agent" | "Task")
}

pub(in crate::anthropic) fn is_acp_bridge_request_id(id: &Value) -> bool {
    id.get(ACP_BRIDGE_MARKER).and_then(Value::as_bool) == Some(true)
}

pub(super) fn acp_bridge_request_id(call_id: &str) -> Value {
    json!({ ACP_BRIDGE_MARKER: true, "callId": call_id })
}

fn bridgeable_status(status: Option<&str>) -> bool {
    // Cursor often opens Task incomplete, then fills args on update/completed.
    // Allow completed so a late-ready launch still becomes Claude Code tool_use.
    match status.unwrap_or("pending") {
        "pending" | "in_progress" | "started" | "completed" => true,
        "failed" | "cancelled" => false,
        _ => true,
    }
}

fn requested_original_name<'a>(
    names: &'a HashMap<String, String>,
    provider_name: &str,
) -> Option<&'a str> {
    names.get(provider_name).map(String::as_str).or_else(|| {
        names
            .values()
            .find(|name| name.as_str() == provider_name)
            .map(String::as_str)
    })
}

fn has_agent_tool(names: &HashMap<String, String>) -> bool {
    names.values().any(|name| name == "Agent")
        || names.contains_key("Agent")
        || names
            .keys()
            .any(|key| key.ends_with("Agent") || key.contains("Agent"))
}

fn looks_like_launch_tool(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower == "agent"
        || lower == "task"
        || lower == SPAWN_SUBAGENT
        || lower == "mcp"
        || lower.contains("claudex-launch")
        || lower.contains("mcp__claudex")
        || lower.contains("mcp__") && (lower.contains("agent") || lower.contains("task"))
        || lower.ends_with("__agent")
        || lower.ends_with("__task")
        || lower.contains("spawn_subagent")
}

/// Cursor surfaces injected MCP tools as title `MCP` (kind=other). Detect launch
/// intent from argument shape so those calls still become Claude Code Agent/Task.
fn looks_like_launch_arguments(arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    let has_prompt = PROMPT_ALIASES
        .iter()
        .any(|key| nonempty_string(object, key).is_some());
    if !has_prompt {
        return false;
    }
    object.contains_key("subagent_type")
        || object.contains_key("run_in_background")
        || object.contains_key("claudex_model")
        || object.contains_key("claudex_effort")
        || object.contains_key("_toolName")
        || DESCRIPTION_ALIASES
            .iter()
            .any(|key| nonempty_string(object, key).is_some())
}

fn launch_tool_name_from_arguments(
    arguments: &Value,
    names: &HashMap<String, String>,
) -> Option<String> {
    if !has_agent_tool(names) {
        return None;
    }
    let object = arguments.as_object()?;
    let tool_name = object
        .get("_toolName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if tool_name == "task"
        || tool_name.ends_with("__task")
        || tool_name.contains("task") && names.values().any(|name| name == "Task")
    {
        if names.values().any(|name| name == "Task") {
            return Some("Task".to_owned());
        }
    }
    Some("Agent".to_owned())
}

fn map_launch_name(candidate: &str, names: &HashMap<String, String>) -> Option<String> {
    if let Some(original) = requested_original_name(names, candidate)
        && is_client_executed_bridge_tool(original)
    {
        return Some(original.to_owned());
    }
    if looks_like_launch_tool(candidate) && has_agent_tool(names) {
        if candidate.eq_ignore_ascii_case("task")
            || candidate.to_ascii_lowercase().ends_with("__task")
        {
            if names.values().any(|name| name == "Task") {
                return Some("Task".to_owned());
            }
        }
        return Some("Agent".to_owned());
    }
    None
}

fn nonempty_string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn take_alias_string(object: &mut Map<String, Value>, aliases: &[&str]) -> Option<String> {
    for key in aliases {
        if let Some(value) = nonempty_string(object, key).map(str::to_owned) {
            if *key != aliases[0] {
                object.remove(*key);
            }
            return Some(value);
        }
    }
    None
}

fn normalize_launch_arguments(provider_name: &str, arguments: &Value) -> Value {
    let mut mapped = match arguments {
        Value::Object(map) => Value::Object(map.clone()),
        other => json!({"value": other}),
    };
    let Some(object) = mapped.as_object_mut() else {
        return mapped;
    };
    // Cursor ACP Task/Agent meta — never part of Claude Code's launch schema.
    object.remove("_toolName");
    object.remove("_tool_name");

    if let Some(prompt) = take_alias_string(object, PROMPT_ALIASES) {
        object.insert("prompt".to_owned(), json!(prompt));
    }
    if let Some(description) = take_alias_string(object, DESCRIPTION_ALIASES) {
        object.insert("description".to_owned(), json!(description));
    }
    if nonempty_string(object, "description").is_none()
        && let Some(prompt) = nonempty_string(object, "prompt")
    {
        let description: String = prompt.chars().take(60).collect();
        object.insert("description".to_owned(), json!(description));
    }

    if provider_name.eq_ignore_ascii_case(SPAWN_SUBAGENT)
        || provider_name
            .to_ascii_lowercase()
            .contains("spawn_subagent")
    {
        if let Some(subagent_type) = object.get("subagent_type").and_then(Value::as_str) {
            if subagent_type == GROK_HIGH_PROFILE || subagent_type.ends_with(":claudex-high") {
                object.insert("subagent_type".to_owned(), json!("claudex-grok"));
            }
        }
        object.insert("run_in_background".to_owned(), json!(true));
    }
    mapped
}

/// Claude Code Agent/Task require a real prompt. Cursor often opens native Task
/// cards with only `_toolName` / `run_in_background` (or empty rawInput); bridging
/// those yields InputValidationError and aborts the main-session turn.
fn launch_arguments_ready(arguments: &Value) -> bool {
    arguments
        .as_object()
        .and_then(|object| nonempty_string(object, "prompt"))
        .is_some()
}

/// True when this providerTool event is a launch-shaped Agent/Task/spawn_subagent
/// card that will not become Claude Code tool_use (incomplete args, or missing
/// Agent mapping). Suppress WIP text for those so Cursor `auto` cannot fake
/// `▶ Task` / `▶ MCP` / `✓ Task` as if Claudex workers started.
pub(super) fn is_unbridged_launch_progress(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
) -> bool {
    let Some(params) = event.get("params") else {
        return false;
    };
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let launch_shaped = launch_name_candidates(tool, title)
        .into_iter()
        .any(|candidate| {
            looks_like_launch_tool(candidate)
                || map_launch_name(candidate, external_tool_names).is_some()
        })
        || looks_like_launch_arguments(raw_args);
    if !launch_shaped {
        return false;
    }
    bridge_provider_tool_call(external_tool_names, event).is_none()
}

/// If this providerTool event is a request-supplied Agent/Task launch (or Grok
/// spawn_subagent when Agent is available), return a ToolCall for Claude Code.
pub(super) fn bridge_provider_tool_call(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
) -> Option<ToolCall> {
    bridge_provider_tool_call_inner(external_tool_names, event, false, None)
}

/// Like [`bridge_provider_tool_call`], but also consults the MCP launch queue when
/// Cursor later emits a generic `provider tool` update for a known MCP call id.
pub(super) fn bridge_provider_tool_call_with_mcp_hint(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
    launch_owner: Option<&str>,
) -> Option<ToolCall> {
    bridge_provider_tool_call_inner(external_tool_names, event, true, launch_owner)
}

fn bridge_provider_tool_call_inner(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
    force_mcp_queue: bool,
    launch_owner: Option<&str>,
) -> Option<ToolCall> {
    trace_launch_shaped_event(event);
    let params = event.get("params")?;
    if !bridgeable_status(params.get("status").and_then(Value::as_str)) {
        return None;
    }
    let call_id = params.get("callId").and_then(Value::as_str)?;
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let mcp_shaped = force_mcp_queue
        || [tool, title]
            .into_iter()
            .any(|candidate| looks_like_mcp_surface(candidate));
    let normalized_raw = normalize_launch_arguments("Agent", raw_args);
    let queued = if mcp_shaped && !launch_arguments_ready(&normalized_raw) {
        super::acp_launch_queue::peek_pending_launch_arguments_for(launch_owner)
    } else {
        None
    };
    let effective_args = queued.as_ref().unwrap_or(raw_args);
    let (provider_label, name) = [tool, title]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            map_launch_name(candidate, external_tool_names).map(|name| (candidate, name))
        })
        .or_else(|| {
            if !looks_like_launch_arguments(effective_args)
                && ![tool, title]
                    .into_iter()
                    .any(|candidate| looks_like_launch_tool(candidate))
            {
                return None;
            }
            let name = launch_tool_name_from_arguments(effective_args, external_tool_names)?;
            let label = [tool, title]
                .into_iter()
                .find(|candidate| !candidate.is_empty())
                .unwrap_or("Agent");
            Some((label, name))
        })?;
    let arguments = normalize_launch_arguments(provider_label, effective_args);
    if !launch_arguments_ready(&arguments) {
        return None;
    }
    if queued.is_some() {
        let _ = super::acp_launch_queue::take_pending_launch_arguments_for(launch_owner);
        tracing::info!(
            launch_owner,
            "using queued claudex-launch MCP arguments for ACP bridge"
        );
    }
    Some(ToolCall {
        call_id: call_id.to_owned(),
        name,
        arguments,
        request_id: acp_bridge_request_id(call_id),
    })
}

/// Tool labels only. Full shell titles (`schtasks`, "Search local agent history")
/// must not count as Agent/Task just because they contain those substrings.
fn launch_name_candidates<'a>(tool: &'a str, title: &'a str) -> Vec<&'a str> {
    let mut labels = Vec::new();
    if !tool.is_empty() {
        labels.push(tool);
    }
    if !title.is_empty() && is_compact_tool_label(title) {
        labels.push(title);
    }
    labels
}

fn is_compact_tool_label(candidate: &str) -> bool {
    let trimmed = candidate.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 64
        && !trimmed.contains('\n')
        && !trimmed.starts_with('`')
        && (!trimmed.chars().any(char::is_whitespace) || looks_like_launch_tool(trimmed))
}

fn looks_like_mcp_surface(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower == "mcp"
        || lower.starts_with("mcp:")
        || lower.starts_with("mcp ")
        || lower.contains("claudex-launch")
        || (lower.contains("mcp") && (lower.contains("agent") || lower.contains("task")))
}

fn trace_launch_shaped_event(event: &Value) {
    let Some(params) = event.get("params") else {
        return;
    };
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let status = params.get("status").and_then(Value::as_str).unwrap_or("");
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let interesting = [tool, title].into_iter().any(|candidate| {
        let lower = candidate.to_ascii_lowercase();
        lower == "mcp"
            || lower.contains("agent")
            || lower.contains("task")
            || looks_like_launch_tool(candidate)
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

#[cfg(test)]
include!("acp_tool_bridge_tests.rs");
