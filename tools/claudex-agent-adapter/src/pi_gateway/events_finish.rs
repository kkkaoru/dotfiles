use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use super::{EventTranslateState, ToolCallBuffer};

const AGENT: &str = "Agent";
const SEND_MESSAGE: &str = "SendMessage";
const GROK_MEDIUM_PROFILE: &str = "grok-native-medium-plugin-v3:claudex-medium";
const RECIPIENT_KEYS: [&str; 3] = ["to", "resume_from", "resume"];
const MESSAGE_KEYS: [&str; 6] = ["prompt", "message", "task", "instruction", "query", "input"];

pub(super) fn finished_tool_arguments(event: &Value, tool: &ToolCallBuffer) -> Result<Value> {
    let event_args = event
        .pointer("/toolCall/arguments")
        .or_else(|| event.get("arguments"))
        .cloned();
    let buffer_args = serde_json::from_str(&tool.arguments).ok();
    if let Some(arguments) = event_args
        .as_ref()
        .filter(|value| tool_arguments_usable(&tool.name, value))
    {
        return Ok(arguments.clone());
    }
    if let Some(arguments) = buffer_args.filter(|value| tool_arguments_usable(&tool.name, value)) {
        return Ok(arguments);
    }
    if let Some(arguments) = event_args {
        return Ok(arguments);
    }
    serde_json::from_str(&tool.arguments).context("decode Pi tool call arguments")
}

/// Claude Code launch is Agent; continue is SendMessage({to, message}), not Agent resume.
pub(super) fn mapped_start_tool_name(name: &str) -> &str {
    if is_spawn_subagent(name) { AGENT } else { name }
}

pub(in crate::pi_gateway) fn event_translate_state(request: &Value) -> super::EventTranslateState {
    super::EventTranslateState {
        listed_tools: listed_claude_tool_names(request),
        ..super::EventTranslateState::default()
    }
}

pub(super) fn current_tool_arguments(event: &Value, tool: &ToolCallBuffer) -> Value {
    event
        .pointer("/toolCall/arguments")
        .or_else(|| event.get("arguments"))
        .cloned()
        .or_else(|| serde_json::from_str(&tool.arguments).ok())
        .unwrap_or_else(|| json!({}))
}

pub(super) fn listed_claude_tool_names(request: &Value) -> HashSet<String> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// Emit SendMessage only when `listed_tools` contains `"SendMessage"`.
/// Otherwise fail closed: no Agent/{to,message} fallback.
pub(super) fn mapped_claude_code_tool(
    name: &str,
    arguments: Value,
    listed_tools: &HashSet<String>,
) -> Option<(String, Value)> {
    if let Some(follow_up) = send_message_arguments(name, &arguments) {
        return listed_tools
            .contains(SEND_MESSAGE)
            .then(|| (SEND_MESSAGE.to_owned(), follow_up));
    }
    if is_spawn_subagent(name) {
        return Some((AGENT.to_owned(), normalize_spawn_subagent_launch(arguments)));
    }
    if is_subagent_lifecycle_tool(name) {
        return Some((name.to_owned(), strip_removed_resume_fields(arguments)));
    }
    Some((name.to_owned(), arguments))
}

fn tool_arguments_usable(name: &str, arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    if name.eq_ignore_ascii_case("Bash") {
        return bash_command_present(object);
    }
    if is_subagent_lifecycle_tool(name) {
        return subagent_arguments_usable(name, arguments);
    }
    !object.is_empty()
}

fn bash_command_present(object: &Map<String, Value>) -> bool {
    object
        .get("command")
        .or_else(|| object.get("cmd"))
        .or_else(|| object.get("script"))
        .or_else(|| object.get("bash"))
        .and_then(Value::as_str)
        .is_some_and(|command| !command.is_empty())
}

fn subagent_arguments_usable(name: &str, arguments: &Value) -> bool {
    send_message_arguments(name, arguments).is_some()
        || arguments
            .as_object()
            .and_then(|object| first_nonempty(object, &MESSAGE_KEYS))
            .is_some()
}

fn send_message_arguments(name: &str, arguments: &Value) -> Option<Value> {
    if !is_subagent_lifecycle_tool(name) {
        return None;
    }
    let object = arguments.as_object()?;
    let to = first_nonempty(object, &RECIPIENT_KEYS)?;
    let message = first_nonempty(object, &MESSAGE_KEYS)?;
    Some(json!({"to": to, "message": message}))
}

fn first_nonempty(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn normalize_spawn_subagent_launch(arguments: Value) -> Value {
    let mut mapped = match arguments {
        Value::Object(map) => Value::Object(map),
        other => json!({"value": other}),
    };
    let Some(object) = mapped.as_object_mut() else {
        return mapped;
    };
    strip_removed_resume_keys(object);
    object.remove("cwd");
    object.remove("background");
    object.remove("capability_mode");
    remap_grok_subagent_type(object);
    object.insert("run_in_background".to_owned(), json!(true));
    mapped
}

fn strip_removed_resume_fields(arguments: Value) -> Value {
    let Value::Object(mut object) = arguments else {
        return arguments;
    };
    strip_removed_resume_keys(&mut object);
    Value::Object(object)
}

fn strip_removed_resume_keys(object: &mut Map<String, Value>) {
    object.remove("resume");
    object.remove("resume_from");
}

fn remap_grok_subagent_type(object: &mut Map<String, Value>) {
    let Some(subagent_type) = object
        .get("subagent_type")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    if subagent_type != GROK_MEDIUM_PROFILE && !subagent_type.ends_with(":claudex-medium") {
        return;
    }
    object.insert("subagent_type".to_owned(), json!("claudex-grok"));
}

fn is_spawn_subagent(name: &str) -> bool {
    name.to_ascii_lowercase().contains("spawn_subagent")
}

fn is_subagent_lifecycle_tool(name: &str) -> bool {
    is_spawn_subagent(name)
        || name.eq_ignore_ascii_case("agent")
        || name.eq_ignore_ascii_case("task")
        || name.eq_ignore_ascii_case("sendmessage")
}

pub(super) fn anthropic_stop_reason(event: &Value) -> Result<&'static str> {
    match event.get("reason").and_then(Value::as_str) {
        Some("stop") => Ok("end_turn"),
        Some("length") => Ok("max_tokens"),
        Some("toolUse") => Ok("tool_use"),
        Some("deferred") => Ok("pause_turn"),
        Some(reason) => bail!("unsupported Pi gateway stop reason `{reason}`"),
        None => bail!("Pi gateway done event omitted reason"),
    }
}

pub(super) fn event_index(event: &Value) -> Result<u64> {
    event
        .get("index")
        .or_else(|| event.get("contentIndex"))
        .and_then(Value::as_u64)
        .context("Pi gateway content event omitted index")
}

pub(super) fn tool_id(event: &Value) -> Option<&str> {
    event
        .get("toolCallId")
        .or_else(|| event.pointer("/toolCall/id"))
        .or_else(|| event.pointer("/block/id"))
        .and_then(Value::as_str)
}

pub(super) fn tool_name(event: &Value) -> Option<&str> {
    event
        .get("name")
        .or_else(|| event.pointer("/toolCall/name"))
        .or_else(|| event.pointer("/block/name"))
        .and_then(Value::as_str)
}

pub(super) fn start_tool_call(
    event: &Value,
    tools: &mut HashMap<u64, ToolCallBuffer>,
) -> Result<()> {
    let index = event_index(event)?;
    if tools.contains_key(&index) {
        bail!("Pi gateway repeated toolcall_start index {index}");
    }
    tools.insert(
        index,
        ToolCallBuffer {
            id: tool_id(event).unwrap_or_default().to_owned(),
            name: tool_name(event).unwrap_or_default().to_owned(),
            arguments: String::new(),
            start_emitted: false,
        },
    );
    Ok(())
}

pub(super) fn mark_streamed(state: &mut EventTranslateState, event: &Value) {
    if let Ok(index) = event_index(event) {
        state.streamed_content.insert(index);
    }
}

pub(super) fn append_tool_call(
    event: &Value,
    tools: &mut HashMap<u64, ToolCallBuffer>,
) -> Result<()> {
    let index = event_index(event)?;
    let delta = event
        .get("delta")
        .and_then(Value::as_str)
        .context("Pi toolcall_delta omitted delta")?;
    tools
        .get_mut(&index)
        .context("Pi toolcall_delta did not match toolcall_start")?
        .arguments
        .push_str(delta);
    Ok(())
}
