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
            system: json!("<claudex-agent-id>worker-1</claudex-agent-id>"),
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

    #[test]
    fn ignores_historical_agent_markers_when_a_main_resume_continues() {
        let request = MessagesRequest {
            model: "claude-opus-5".to_owned(),
            system: json!("main session"),
            messages: vec![
                json!({"role":"user","content":"launch workers"}),
                json!({
                    "role":"assistant",
                    "content":[{
                        "type":"tool_use",
                        "name":"Agent",
                        "id":"toolu_worker-1",
                        "input":{"prompt":"worker task\nclaudex_launch_id: toolu_worker-1\n<claudex-agent-id>toolu_worker-1</claudex-agent-id>"}
                    }]
                }),
                json!({
                    "role":"user",
                    "content":[{"type":"tool_result","tool_use_id":"toolu_worker-1","content":"worker result"}]
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
    fn plain_and_nested_values_without_markers_stay_main_session_requests() {
        assert!(!value_contains_subagent_marker(&serde_json::json!(
            "ordinary user text"
        )));
        assert!(value_contains_subagent_marker(&serde_json::json!(
            "cc_is_subagent=true"
        )));
        assert!(value_contains_subagent_marker(&serde_json::json!(
            "<claudex-agent-id>worker</claudex-agent-id>"
        )));
        assert!(!value_contains_subagent_marker(&serde_json::json!([
            null,
            {"content": ["ordinary", 7, false]}
        ])));
        assert!(!is_subagent_request(&MessagesRequest {
            model: "main-model".to_owned(),
            system: serde_json::json!(null),
            messages: vec![serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "ordinary user text"}]
            })],
            tools: Vec::new(),
            stream: false,
            output_config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }));
    }

    #[test]
    fn native_session_header_overrides_historical_child_markers_for_main() {
        let mut request: MessagesRequest = serde_json::from_value(json!({
            "model":"claude-opus-5",
            "system":"main session",
            "messages":[{"role":"user","content":"continue <claudex-agent-id>archived</claudex-agent-id>"}]
        }))
        .expect("request");
        super::super::RequestIdentity::new(Some("session-main".to_owned()), None, None)
            .attach(&mut request);

        assert!(!is_subagent_request(&request));
    }

    #[test]
    fn native_agent_header_identifies_a_child_without_body_markers() {
        let mut request: MessagesRequest = serde_json::from_value(json!({
            "model":"worker-model",
            "system":"ordinary system",
            "messages":[{"role":"user","content":"ordinary task"}]
        }))
        .expect("request");
        super::super::RequestIdentity::new(
            Some("session-child".to_owned()),
            Some("agent-child".to_owned()),
            None,
        )
        .attach(&mut request);

        assert!(is_subagent_request(&request));
    }

    #[test]
    fn native_parent_header_identifies_nested_child() {
        let mut request: MessagesRequest = serde_json::from_value(json!({
            "model":"worker-model",
            "messages":[{"role":"user","content":"nested task"}]
        }))
        .expect("request");
        super::super::RequestIdentity::new(
            Some("session-nested".to_owned()),
            None,
            Some("agent-parent".to_owned()),
        )
        .attach(&mut request);

        assert!(is_subagent_request(&request));
    }
}
