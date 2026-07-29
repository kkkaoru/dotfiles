use serde_json::Value;

use super::agent_effort::AgentEffortIntent;
use crate::anthropic::MessagesRequest;

pub(super) const CORRELATION_TAG: &str = "claudex-agent-id";

pub(super) fn request_matches_intent(messages: &[Value], intent: &AgentEffortIntent) -> bool {
    if intent.correlated {
        let marker = format!(
            "<{CORRELATION_TAG}>{}</{CORRELATION_TAG}>",
            intent.tool_use_id
        );
        return message_texts(messages)
            .any(|text| text.contains(&marker) || contains_launch_id(text, &intent.tool_use_id));
    }
    message_texts(messages).any(|text| text == intent.prompt)
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
    value_texts(&request.system).any(|text| text.contains("cc_is_subagent=true"))
        || request
            .messages
            .iter()
            .filter_map(|message| message.get("content"))
            .flat_map(value_texts)
            .any(has_correlation_marker)
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

fn message_texts(messages: &[Value]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
}

fn contains_launch_id(text: &str, tool_use_id: &str) -> bool {
    let expected = format!("claudex_launch_id: {tool_use_id}");
    text.lines().any(|line| line.trim() == expected)
}
