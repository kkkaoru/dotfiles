use serde_json::{Map, Value};

const BASH_COMMAND_KEYS: [&str; 4] = ["command", "cmd", "script", "bash"];
const SEND_MESSAGE_RECIPIENT_KEYS: [&str; 3] = ["to", "resume_from", "resume"];
const SEND_MESSAGE_BODY_KEYS: [&str; 6] =
    ["prompt", "message", "task", "instruction", "query", "input"];
const READ_PATH_KEYS: [&str; 2] = ["file_path", "path"];
const PATTERN_KEYS: [&str; 1] = ["pattern"];
const SKILL_NAME_KEYS: [&str; 1] = ["skill"];
const WEB_SEARCH_QUERY_KEYS: [&str; 1] = ["query"];
const WEB_FETCH_URL_KEYS: [&str; 1] = ["url"];
const WEB_FETCH_PROMPT_KEYS: [&str; 1] = ["prompt"];
const COMPLETE_JSON_TOOLS: [&str; 8] = [
    "Bash",
    "SendMessage",
    "Read",
    "Grep",
    "Glob",
    "Skill",
    "WebSearch",
    "WebFetch",
];
const INCOMPLETE_BASH_JSON: &str =
    "Incomplete Bash tool JSON was not flushed; a non-empty command is required.";
const INCOMPLETE_SEND_MESSAGE_JSON: &str =
    "Incomplete SendMessage tool JSON was not flushed; non-empty to and message are required.";
const INCOMPLETE_READ_JSON: &str =
    "Incomplete Read tool JSON was not flushed; a non-empty file_path or path is required.";
const INCOMPLETE_GREP_JSON: &str =
    "Incomplete Grep tool JSON was not flushed; a non-empty pattern is required.";
const INCOMPLETE_GLOB_JSON: &str =
    "Incomplete Glob tool JSON was not flushed; a non-empty pattern is required.";
const INCOMPLETE_SKILL_JSON: &str =
    "Incomplete Skill tool JSON was not flushed; a non-empty skill is required.";
const INCOMPLETE_WEB_SEARCH_JSON: &str =
    "Incomplete WebSearch tool JSON was not flushed; a non-empty query is required.";
const INCOMPLETE_WEB_FETCH_JSON: &str =
    "Incomplete WebFetch tool JSON was not flushed; non-empty url and prompt are required.";
const INCOMPLETE_TOOL_JSON: &str =
    "Incomplete tool JSON was not flushed; required keys are missing.";
const INCOMPLETE_JSON_ERRORS: [(&str, &str); 8] = [
    ("Bash", INCOMPLETE_BASH_JSON),
    ("SendMessage", INCOMPLETE_SEND_MESSAGE_JSON),
    ("Read", INCOMPLETE_READ_JSON),
    ("Grep", INCOMPLETE_GREP_JSON),
    ("Glob", INCOMPLETE_GLOB_JSON),
    ("Skill", INCOMPLETE_SKILL_JSON),
    ("WebSearch", INCOMPLETE_WEB_SEARCH_JSON),
    ("WebFetch", INCOMPLETE_WEB_FETCH_JSON),
];

pub(super) enum ToolJsonReadiness {
    Truncated,
    Incomplete,
    Ready,
}

pub(super) fn tool_json_readiness(name: &str, raw: &str) -> ToolJsonReadiness {
    if raw.trim().is_empty() {
        return ToolJsonReadiness::Incomplete;
    }
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ToolJsonReadiness::Truncated;
    };
    match tool_arguments_ready(name, &value) {
        true => ToolJsonReadiness::Ready,
        false => ToolJsonReadiness::Incomplete,
    }
}

pub(super) fn tool_input_ready(name: &str, partial_json: &str, arguments: &Value) -> bool {
    tool_arguments_ready(name, arguments)
        || matches!(
            tool_json_readiness(name, partial_json),
            ToolJsonReadiness::Ready
        )
}

pub(super) fn tool_arguments_ready(name: &str, arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    if name.eq_ignore_ascii_case("Bash") {
        return bash_command_present(object);
    }
    if name.eq_ignore_ascii_case("SendMessage") {
        return send_message_ready(object);
    }
    if name.eq_ignore_ascii_case("Read") {
        return first_nonempty(object, &READ_PATH_KEYS).is_some();
    }
    if name.eq_ignore_ascii_case("Grep") || name.eq_ignore_ascii_case("Glob") {
        return first_nonempty(object, &PATTERN_KEYS).is_some();
    }
    if name.eq_ignore_ascii_case("Skill") {
        return first_nonempty(object, &SKILL_NAME_KEYS).is_some();
    }
    if name.eq_ignore_ascii_case("WebSearch") {
        return first_nonempty(object, &WEB_SEARCH_QUERY_KEYS).is_some();
    }
    if name.eq_ignore_ascii_case("WebFetch") {
        return first_nonempty(object, &WEB_FETCH_URL_KEYS).is_some()
            && first_nonempty(object, &WEB_FETCH_PROMPT_KEYS).is_some();
    }
    true
}

fn bash_command_present(object: &Map<String, Value>) -> bool {
    BASH_COMMAND_KEYS.iter().any(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|command| !command.is_empty())
    })
}

fn send_message_ready(object: &Map<String, Value>) -> bool {
    first_nonempty(object, &SEND_MESSAGE_RECIPIENT_KEYS).is_some()
        && first_nonempty(object, &SEND_MESSAGE_BODY_KEYS).is_some()
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

pub(super) fn requires_complete_tool_json(name: &str) -> bool {
    COMPLETE_JSON_TOOLS
        .iter()
        .any(|tool| name.eq_ignore_ascii_case(tool))
}

pub(super) fn incomplete_tool_json_error(name: &str) -> &'static str {
    INCOMPLETE_JSON_ERRORS
        .iter()
        .find_map(|(tool, message)| name.eq_ignore_ascii_case(tool).then_some(*message))
        .unwrap_or(INCOMPLETE_TOOL_JSON)
}

pub(super) fn finished_tool_json_payload(
    name: &str,
    partial_json: &str,
    claude_arguments: &Value,
) -> String {
    match tool_arguments_ready(name, claude_arguments) {
        true => claude_arguments.to_string(),
        false => partial_json.to_owned(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "streaming_tool_ready_tests.rs"]
mod tests;
