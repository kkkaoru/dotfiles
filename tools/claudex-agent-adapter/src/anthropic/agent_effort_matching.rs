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
    if value_contains_billing_marker(&request.system) {
        return true;
    }
    let has_tool_result = request.messages.iter().any(is_tool_result_message);
    request.messages.iter().any(|message| {
        message.get("role").and_then(Value::as_str) == Some("user")
            && value_contains_subagent_marker(message)
    }) || (has_tool_result
        && request
            .messages
            .iter()
            .any(value_contains_correlation_marker))
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

fn is_tool_result_message(value: &Value) -> bool {
    value.get("role").and_then(Value::as_str) == Some("user")
        && value
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
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
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use super::*;

    #[test]
    fn matches_a_correlation_marker_in_system_content() {
        let intent = AgentEffortIntent {
            client_user_id: None,
            prompt: String::new(),
            correlated: true,
            effort: None,
            model_override: Some("gpt-5.6-luna".to_owned()),
            model_is_inherited: false,
            run_in_background: false,
            tool_use_id: "toolu_system_marker".to_owned(),
            created_at: Instant::now(),
            created_unix_seconds: 0,
        };
        let system = json!([{
            "type": "text",
            "text": "cc_is_subagent=true\n<claudex-agent-id>toolu_system_marker</claudex-agent-id>"
        }]);
        let request = MessagesRequest {
            model: "claude-sonnet-5".to_owned(),
            system: system.clone(),
            messages: Vec::new(),
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };

        assert!(is_subagent_request(&request));
        assert!(request_matches_intent_with_system(
            &system,
            &request.messages,
            &intent
        ));
        let nested_messages = vec![json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "input": {
                    "prompt": "continue <claudex-agent-id>toolu_system_marker</claudex-agent-id>"
                }
            }]
        })];
        assert!(request_matches_intent(&nested_messages, &intent));
    }

    #[test]
    fn ignores_a_prior_assistant_correlation_marker_for_an_outer_follow_up() {
        let request = MessagesRequest {
            model: "main-model".to_owned(),
            system: json!("main session"),
            messages: vec![
                json!({"role":"user","content":"launch a worker"}),
                json!({
                    "role":"assistant",
                    "content":[{
                        "type":"tool_use",
                        "name":"Agent",
                        "input":{"prompt":"work <claudex-agent-id>worker-1</claudex-agent-id>"}
                    }]
                }),
                json!({"role":"user","content":"continue the main response"}),
            ],
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };

        assert!(!is_subagent_request(&request));
    }

    #[test]
    fn keeps_a_correlation_marker_for_a_tool_result_continuation() {
        let request = MessagesRequest {
            model: "worker-model".to_owned(),
            system: json!("child session"),
            messages: vec![
                json!({
                    "role":"assistant",
                    "content":[{
                        "type":"tool_use",
                        "name":"Agent",
                        "input":{"prompt":"work <claudex-agent-id>worker-1</claudex-agent-id>"}
                    }]
                }),
                json!({
                    "role":"user",
                    "content":[{"type":"tool_result","tool_use_id":"worker-1","content":"done"}]
                }),
            ],
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };

        assert!(is_subagent_request(&request));
    }
}
