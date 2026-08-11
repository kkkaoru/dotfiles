use std::collections::HashMap;

use serde_json::{Map, Value, json};

const SPAWN_SUBAGENT: &str = "spawn_subagent";
const GROK_HIGH_PROFILE: &str = "grok-native-high-plugin-v3:claudex-high";
const PROMPT_ALIASES: &[&str] = &["prompt", "task", "message", "instruction", "query", "input"];
const DESCRIPTION_ALIASES: &[&str] = &["description", "title", "name", "summary", "subject"];

pub(super) fn requested_original_name<'a>(
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

pub(super) fn has_agent_tool(names: &HashMap<String, String>) -> bool {
    names.values().any(|name| name == "Agent")
        || names.contains_key("Agent")
        || names
            .keys()
            .any(|key| key.ends_with("Agent") || key.contains("Agent"))
}

pub(super) fn looks_like_launch_tool(candidate: &str) -> bool {
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
pub(super) fn looks_like_launch_arguments(arguments: &Value) -> bool {
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

pub(super) fn launch_tool_name_from_arguments(
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
    if (tool_name == "task"
        || tool_name.ends_with("__task")
        || tool_name.contains("task") && names.values().any(|name| name == "Task"))
        && names.values().any(|name| name == "Task")
    {
        return Some("Task".to_owned());
    }
    Some("Agent".to_owned())
}

pub(super) fn map_launch_name(candidate: &str, names: &HashMap<String, String>) -> Option<String> {
    if let Some(original) = requested_original_name(names, candidate)
        && super::is_client_executed_bridge_tool(original)
    {
        return Some(original.to_owned());
    }
    if looks_like_launch_tool(candidate) && has_agent_tool(names) {
        if (candidate.eq_ignore_ascii_case("task")
            || candidate.to_ascii_lowercase().ends_with("__task"))
            && names.values().any(|name| name == "Task")
        {
            return Some("Task".to_owned());
        }
        return Some("Agent".to_owned());
    }
    None
}

pub(super) fn nonempty_string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn take_alias_string(
    object: &mut Map<String, Value>,
    aliases: &[&str],
) -> Option<String> {
    let (index, value) = aliases.iter().enumerate().find_map(|(index, key)| {
        nonempty_string(object, key).map(|value| (index, value.to_owned()))
    })?;
    if index != 0 {
        object.remove(aliases[index]);
    }
    Some(value)
}

pub(super) fn normalize_launch_arguments(provider_name: &str, arguments: &Value) -> Value {
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
        if let Some(subagent_type) = object.get("subagent_type").and_then(Value::as_str)
            && (subagent_type == GROK_HIGH_PROFILE || subagent_type.ends_with(":claudex-high"))
        {
            object.insert("subagent_type".to_owned(), json!("claudex-grok"));
        }
        object.insert("run_in_background".to_owned(), json!(true));
    }
    mapped
}

/// Claude Code Agent/Task require a real prompt. Cursor often opens native Task
/// cards with only `_toolName` / `run_in_background` (or empty rawInput); bridging
/// those yields InputValidationError and aborts the main-session turn.
pub(super) fn launch_arguments_ready(arguments: &Value) -> bool {
    arguments
        .as_object()
        .and_then(|object| nonempty_string(object, "prompt"))
        .is_some()
}
