use serde_json::json;

use super::*;

fn request(content: Value) -> MessagesRequest {
    MessagesRequest {
        model: "test-model".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"user", "content":content})],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

#[test]
fn recognizes_only_pure_internal_agent_notifications() {
    assert!(is_internal_notification_request(&request(
        "<agent-message from=\"general-purpose\">worker output</agent-message>".into()
    )));
    assert!(is_internal_notification_request(&request(
        json!([{"type":"text","text":"<task-notification>done</task-notification>"}])
    )));
    assert!(!is_internal_notification_request(&request(
        "Please inspect the literal <agent-message> tag".into()
    )));
    assert!(!is_internal_notification_request(&request(json!([{
        "type":"tool_result",
        "tool_use_id":"toolu_internal",
        "content":"<agent-message>result</agent-message>"
    }]))));
}

#[test]
fn recognizes_notification_with_trailing_transcript_elements() {
    let mut request =
        request("<task-notification><status>completed</status></task-notification>".into());
    request.messages.push(json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "acknowledged"}]
    }));

    assert!(is_internal_notification_request(&request));
}

#[test]
fn recognizes_monitor_notification_with_standard_hint_block() {
    assert!(is_internal_notification_request(&request(json!([
        {
            "type":"text",
            "text":"<task-notification><status>completed</status></task-notification>"
        },
        {
            "type":"text",
            "text":"If this event is something the user would act on now, send a PushNotification."
        }
    ]))));
}

#[test]
fn keeps_real_instruction_after_notification_as_a_user_turn() {
    assert!(!is_internal_notification_request(&request(json!([
        {
            "type":"text",
            "text":"<task-notification><status>completed</status></task-notification>"
        },
        {"type":"text","text":"continue with the requested change"}
    ]))));
}

#[test]
fn does_not_ack_notification_when_a_newer_user_turn_exists() {
    let mut request =
        request("<task-notification><status>completed</status></task-notification>".into());
    request.messages.push(json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "acknowledged"}]
    }));
    request
        .messages
        .push(json!({"role": "user", "content": "continue with the requested change"}));

    assert!(!is_internal_notification_request(&request));
}

#[test]
fn ignores_empty_user_history_after_notification() {
    let mut request =
        request("<task-notification><status>completed</status></task-notification>".into());
    request.messages.push(json!({"role":"user", "content":""}));
    request
        .messages
        .push(json!({"role":"assistant", "content":[]}));

    assert!(is_internal_notification_request(&request));
}

#[test]
fn ignores_empty_text_blocks_after_notification() {
    let mut request =
        request("<task-notification><status>completed</status></task-notification>".into());
    request.messages.push(json!({
        "role":"user",
        "content":[{"type":"text","text":""}]
    }));

    assert!(is_internal_notification_request(&request));
}

#[test]
fn keeps_tool_result_after_notification_as_a_real_turn() {
    let mut request =
        request("<task-notification><status>completed</status></task-notification>".into());
    request.messages.push(json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"toolu_real",
            "content":"provider result"
        }]
    }));

    assert!(!is_internal_notification_request(&request));
}

#[test]
fn removes_internal_history_and_keeps_real_user_blocks() {
    let mut request = request(json!([{
        "type":"text",
        "text":"real instruction"
    }]));
    request.messages = vec![
        json!({"role":"user","content":"first instruction"}),
        json!({"role":"user","content":"<agent-message from=\"worker\">done</agent-message>"}),
        json!({"role":"user","content":[
            {"type":"text","text":"<task-notification>done</task-notification>"},
            {"type":"text","text":"latest instruction"}
        ]}),
    ];

    remove_from_transcript(&mut request);

    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0]["content"], "first instruction");
    assert_eq!(
        request.messages[1]["content"][0]["text"],
        "latest instruction"
    );
}

#[test]
fn drops_empty_user_arrays_and_keeps_contentless_user_messages() {
    let mut req = request("keep me".into());
    req.messages = vec![
        json!({"role":"user"}),
        json!({"role":"user","content":[]}),
        json!({"role":"user","content":[
            {"type":"text","text":"  "},
            {"type":"text","text":"<agent-message from=\"w\">done</agent-message>"}
        ]}),
        json!({"role":"user","content":"keep me"}),
    ];
    assert!(is_internal_notification_request(&request(json!([
        {"type":"text","text":"  "},
        {"type":"text","text":"<agent-message from=\"w\">done</agent-message>"}
    ]))));
    assert!(!is_internal_notification_request(&request(json!([]))));
    remove_from_transcript(&mut req);
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[0], json!({"role":"user"}));
    assert_eq!(req.messages[1]["content"], "keep me");
}

#[test]
fn preserves_teammate_wrappers_that_carry_a_subagent_prompt() {
    let mut request = request(
        "<teammate-message>Use the assigned model and report the result.</teammate-message>".into(),
    );

    remove_from_transcript(&mut request);

    assert_eq!(request.messages.len(), 1);
    assert!(
        request.messages[0]["content"]
            .as_str()
            .is_some_and(|text| text.contains("Use the assigned model"))
    );
}
