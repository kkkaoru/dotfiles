use serde_json::Value;

use super::agent_effort::AgentEffortIntent;
use crate::anthropic::MessagesRequest;

pub(super) const CORRELATION_TAG: &str = "claudex-agent-id";

pub(super) fn request_matches_intent(messages: &[Value], intent: &AgentEffortIntent) -> bool {
    messages
        .iter()
        .any(|message| value_matches_intent(message, intent))
}

pub(super) fn request_matches_intent_with_system(
    system: &Value,
    messages: &[Value],
    intent: &AgentEffortIntent,
) -> bool {
    value_matches_intent(system, intent) || request_matches_intent(messages, intent)
}

fn value_matches_intent(value: &Value, intent: &AgentEffortIntent) -> bool {
    match value {
        Value::String(text) => text_matches_intent(text, intent),
        Value::Array(values) => values
            .iter()
            .any(|value| value_matches_intent(value, intent)),
        Value::Object(values) => values
            .values()
            .any(|value| value_matches_intent(value, intent)),
        _ => false,
    }
}

fn text_matches_intent(text: &str, intent: &AgentEffortIntent) -> bool {
    if intent.correlated {
        let marker = format!(
            "<{CORRELATION_TAG}>{}</{CORRELATION_TAG}>",
            intent.tool_use_id
        );
        return text.contains(&marker) || contains_launch_id(text, &intent.tool_use_id);
    }
    text == intent.prompt
}

pub(super) fn has_correlation_marker(prompt: &str) -> bool {
    prompt.contains(&format!("<{CORRELATION_TAG}>"))
}

pub(super) fn correlated_prompt(prompt: &str, tool_use_id: &str, model: Option<&str>) -> String {
    let model_header = model.map_or_else(String::new, |model| format!("\nclaudex_model: {model}"));
    format!(
        "{prompt}\n\nclaudex_launch_id: {tool_use_id}{model_header}\n\n<{CORRELATION_TAG}>{tool_use_id}</{CORRELATION_TAG}>"
    )
}

pub(super) fn is_subagent_request(request: &MessagesRequest) -> bool {
    if let Some(is_subagent) = super::request_identity::authoritative_is_subagent(request) {
        // session_id alone is "probably main", but CC 2.1 SubAgent SSE also
        // sends x-claude-code-session-id. Live launch chrome must still win or
        // Muse Spark stays on repeating "Thought for Xs".
        return is_subagent || has_live_subagent_launch_marker(request);
    }
    if value_contains_billing_marker(&request.system)
        || value_contains_correlation_marker(&request.system)
    {
        return true;
    }
    // A resumed main session can contain completed Agent tool calls and their
    // correlation markers in its historical transcript.  Only the current
    // user turn is authoritative; never classify the main session from an old
    // assistant/tool-result pair.
    current_turn_user_messages(&request.messages).any(value_contains_subagent_marker)
}

fn has_live_subagent_launch_marker(request: &MessagesRequest) -> bool {
    value_contains_live_launch_marker(&request.system)
        || current_turn_user_messages(&request.messages).any(value_contains_live_launch_marker)
}

/// Claude Code injects skills / hook context as extra user messages after the
/// delegated prompt. The latest user blob is often `ctx-agent-history-search`
/// without `claudex_launch_id`, which used to hide live SubAgent chrome.
fn current_turn_user_messages(messages: &[Value]) -> impl Iterator<Item = &Value> {
    let start = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .map_or(0, |index| index + 1);
    messages[start..]
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
}

fn value_contains_live_launch_marker(value: &Value) -> bool {
    match value {
        Value::String(text) => text_contains_live_launch_marker(text),
        Value::Array(values) => values.iter().any(value_contains_live_launch_marker),
        Value::Object(values) => values.values().any(value_contains_live_launch_marker),
        _ => false,
    }
}

fn text_contains_live_launch_marker(text: &str) -> bool {
    text.contains("cc_is_subagent=true")
        || text
            .lines()
            .any(|line| line.trim().starts_with("claudex_launch_id:"))
}

fn value_contains_subagent_marker(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains("cc_is_subagent=true") || has_correlation_marker(text),
        Value::Array(values) => values.iter().any(value_contains_subagent_marker),
        Value::Object(values) => values.values().any(value_contains_subagent_marker),
        _ => false,
    }
}

fn value_contains_billing_marker(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains("cc_is_subagent=true"),
        Value::Array(values) => values.iter().any(value_contains_billing_marker),
        Value::Object(values) => values.values().any(value_contains_billing_marker),
        _ => false,
    }
}

fn value_contains_correlation_marker(value: &Value) -> bool {
    match value {
        Value::String(text) => has_correlation_marker(text),
        Value::Array(values) => values.iter().any(value_contains_correlation_marker),
        Value::Object(values) => values.values().any(value_contains_correlation_marker),
        _ => false,
    }
}

pub(super) fn value_texts(value: &Value) -> impl Iterator<Item = &str> {
    let direct = value.as_str().into_iter();
    let blocks = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str));
    direct.chain(blocks)
}

fn contains_launch_id(text: &str, tool_use_id: &str) -> bool {
    let expected = format!("claudex_launch_id: {tool_use_id}");
    text.lines().any(|line| line.trim() == expected)
}

#[cfg(test)]
#[path = "agent_effort_matching_tests.rs"]
mod tests;
