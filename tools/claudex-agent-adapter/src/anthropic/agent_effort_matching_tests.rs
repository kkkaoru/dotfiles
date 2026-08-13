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
fn own_launch_ids_come_from_system_not_nested_assistant_tool_use() {
    let request = MessagesRequest {
        model: "gpt-5.6-luna".to_owned(),
        system: json!(
            "cc_is_subagent=true\nclaudex_launch_id: tool-luna\n<claudex-agent-id>tool-luna</claudex-agent-id>"
        ),
        messages: vec![
            json!({"role":"user","content":"parent\nclaudex_launch_id: tool-luna"}),
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "input":{"prompt":"nested\nclaudex_launch_id: tool-nested\n<claudex-agent-id>tool-nested</claudex-agent-id>"}
                }]
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
    assert_eq!(
        request_own_launch_ids(&request),
        vec!["tool-luna".to_owned()]
    );
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

fn matching_request(
    system: serde_json::Value,
    messages: Vec<serde_json::Value>,
) -> MessagesRequest {
    serde_json::from_value(json!({
        "model": "main-model",
        "system": system,
        "messages": messages
    }))
    .expect("messages request")
}

#[test]
fn own_launch_ids_cover_missing_user_unclosed_tag_and_empty_id() {
    assert!(
        request_own_launch_ids(&matching_request(json!("no ids"), Vec::new())).is_empty(),
        "no user message means first_user is None"
    );
    assert_eq!(
        request_own_launch_ids(&matching_request(
            json!(""),
            vec![json!({"role":"user","content":"<claudex-agent-id>orphan"})]
        )),
        Vec::<String>::new()
    );
    assert_eq!(
        request_own_launch_ids(&matching_request(
            json!(""),
            vec![
                json!({"role":"user","content":"claudex_launch_id: \nclaudex_launch_id: same\nclaudex_launch_id: same"})
            ]
        )),
        vec!["same".to_owned()]
    );
}
