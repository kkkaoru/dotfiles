use serde_json::{Value, json};

use super::{
    DESCRIPTION_ALIASES, GROK_HIGH_PROFILE, PROMPT_ALIASES, SPAWN_SUBAGENT, nonempty_string,
    take_alias_string,
};

pub(in crate::anthropic::stream::acp_tool_bridge) fn normalize_launch_arguments(
    provider_name: &str,
    arguments: &Value,
) -> Value {
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
pub(in crate::anthropic::stream::acp_tool_bridge) fn launch_arguments_ready(
    arguments: &Value,
) -> bool {
    arguments
        .as_object()
        .and_then(|object| nonempty_string(object, "prompt"))
        .is_some()
}
