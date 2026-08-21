use anyhow::Error;
use serde_json::{Map, Value};

pub(crate) const CONSECUTIVE_UNUSABLE_TOOL_LIMIT: u8 = 3;
pub(crate) const MAX_TOOL_RESULT_BYTES: usize = 32 * 1_024;
const TOOL_RESULT_TRUNCATION_NOTICE: &str = "\n[tool_result truncated to fit token budget]";
const INPUT_VALIDATION_ERROR_MARKER: &str = "inputvalidationerror";
const COOLING_DOWN_MARKER: &str = "cooling down";
const BASH_COMMAND_KEYS: [&str; 4] = ["command", "cmd", "script", "bash"];
const SEND_MESSAGE_RECIPIENT_KEYS: [&str; 3] = ["to", "resume_from", "resume"];
const SEND_MESSAGE_BODY_KEYS: [&str; 6] =
    ["prompt", "message", "task", "instruction", "query", "input"];
const READ_PATH_KEYS: [&str; 2] = ["file_path", "path"];
const PATTERN_KEYS: [&str; 1] = ["pattern"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnusableCircuitDecision {
    Emit { consecutive: u8 },
    Skip { consecutive: u8, end_turn: bool },
}

pub(crate) fn circuit_is_open(consecutive: u8) -> bool {
    consecutive >= CONSECUTIVE_UNUSABLE_TOOL_LIMIT
}

pub(crate) fn record_unusable(consecutive: u8) -> u8 {
    consecutive
        .saturating_add(1)
        .min(CONSECUTIVE_UNUSABLE_TOOL_LIMIT)
}

pub(crate) fn decide_tool_emit(
    consecutive: u8,
    from_messages: u8,
    arguments_unusable: bool,
) -> UnusableCircuitDecision {
    if circuit_is_open(from_messages) {
        return UnusableCircuitDecision::Skip {
            consecutive: from_messages,
            end_turn: true,
        };
    }
    if circuit_is_open(consecutive) {
        return UnusableCircuitDecision::Skip {
            consecutive,
            end_turn: true,
        };
    }
    if arguments_unusable {
        let consecutive = record_unusable(consecutive);
        return UnusableCircuitDecision::Skip {
            consecutive,
            end_turn: circuit_is_open(consecutive),
        };
    }
    UnusableCircuitDecision::Emit { consecutive: 0 }
}

pub(crate) fn is_input_validation_error(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains(INPUT_VALIDATION_ERROR_MARKER)
}

pub(crate) fn is_provider_cooldown_error(text: &str) -> bool {
    text.to_ascii_lowercase().contains(COOLING_DOWN_MARKER)
}

pub(crate) fn should_retry_provider_failure(error: &Error) -> bool {
    !error.chain().any(|cause| {
        let message = cause.to_string();
        is_provider_cooldown_error(&message) || is_input_validation_error(&message)
    })
}

pub(crate) fn truncated_tool_result_text(text: &str) -> String {
    if text.len() <= MAX_TOOL_RESULT_BYTES {
        return text.to_owned();
    }
    let keep = MAX_TOOL_RESULT_BYTES.saturating_sub(TOOL_RESULT_TRUNCATION_NOTICE.len());
    format!("{}{TOOL_RESULT_TRUNCATION_NOTICE}", utf8_prefix(text, keep))
}

pub(crate) fn consecutive_unusable_from_messages(messages: &[Value]) -> u8 {
    let flags = trailing_user_input_validation_flags(messages);
    let consecutive = flags
        .iter()
        .rev()
        .take_while(|is_unusable| **is_unusable)
        .count();
    u8::try_from(consecutive)
        .unwrap_or(CONSECUTIVE_UNUSABLE_TOOL_LIMIT)
        .min(CONSECUTIVE_UNUSABLE_TOOL_LIMIT)
}

pub(crate) fn tool_arguments_are_unusable(name: &str, arguments: &Value) -> bool {
    !tool_arguments_usable(name, arguments)
}

fn tool_arguments_usable(name: &str, arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    if name.eq_ignore_ascii_case("Bash") {
        return bash_command_present(object);
    }
    if is_subagent_lifecycle_tool(name) {
        return subagent_arguments_usable(object);
    }
    if name.eq_ignore_ascii_case("Read") {
        return first_nonempty(object, &READ_PATH_KEYS).is_some();
    }
    if name.eq_ignore_ascii_case("Grep") || name.eq_ignore_ascii_case("Glob") {
        return first_nonempty(object, &PATTERN_KEYS).is_some();
    }
    !object.is_empty()
}

fn bash_command_present(object: &Map<String, Value>) -> bool {
    BASH_COMMAND_KEYS.iter().any(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|command| !command.is_empty())
    })
}

fn subagent_arguments_usable(object: &Map<String, Value>) -> bool {
    send_message_ready(object) || first_nonempty(object, &SEND_MESSAGE_BODY_KEYS).is_some()
}

fn send_message_ready(object: &Map<String, Value>) -> bool {
    first_nonempty(object, &SEND_MESSAGE_RECIPIENT_KEYS).is_some()
        && first_nonempty(object, &SEND_MESSAGE_BODY_KEYS).is_some()
}

fn is_subagent_lifecycle_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("spawn_subagent")
        || lower == "agent"
        || lower == "task"
        || lower == "sendmessage"
}

fn first_nonempty<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn trailing_user_input_validation_flags(messages: &[Value]) -> Vec<bool> {
    trailing_user_blocks(messages)
        .filter_map(tool_result_is_input_validation)
        .collect()
}

fn trailing_user_blocks(messages: &[Value]) -> impl Iterator<Item = &Value> {
    let start = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) != Some("user"))
        .map_or(0, |index| index + 1);
    messages[start..]
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
}

fn tool_result_is_input_validation(block: &Value) -> Option<bool> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
    Some(is_error && is_input_validation_error(&tool_result_text(block)))
}

fn tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "token_efficiency_tests.rs"]
mod tests;
