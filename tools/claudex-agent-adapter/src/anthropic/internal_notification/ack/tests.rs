use axum::body::to_bytes;
use serde_json::{Value, json};

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

#[tokio::test]
async fn acknowledges_internal_notification_without_echoing_it() {
    let request =
        request("<agent-message from=\"general-purpose\">worker output</agent-message>".into());
    let response = acknowledge(&request);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains("agent-message"));
    assert!(body.contains("worker output"));
    assert!(body.contains("\"stop_reason\":\"end_turn\""));

    let mut streaming = request;
    streaming.stream = true;
    let response = acknowledge(&streaming);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains("agent-message"));
    assert!(body.contains("worker output"));
    assert!(body.contains("content_block_delta"));
    assert!(body.contains("message_delta"));
    assert!(body.contains("message_stop"));
}

#[tokio::test]
async fn acknowledges_task_notification_with_summary_and_result_without_xml() {
    let request = request(
            "<task-notification><status>completed</status><summary>Agent \"worker\" finished</summary><result>worker result</result><note>internal</note></task-notification>".into(),
        );
    let response = acknowledge(&request);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    let response: Value = serde_json::from_str(&body).unwrap();
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("task-notification"));
    assert!(text.contains("Agent \"worker\" finished"));
    assert!(text.contains("worker result"));
    assert!(!text.contains("completed"));
    assert!(!text.contains("internal"));
}

#[tokio::test]
async fn acknowledges_previous_session_orphan_without_encouraging_taskstop() {
    let request = request(
            "<task-notification><status>stopped</status><summary>No completion record was found for 2 background agents from the previous session: \"Fix utterance split and paint key\" (a27155c79179347ce).</summary><note>internal</note></task-notification>".into(),
        );
    let response = acknowledge(&request);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("previous session"));
    assert!(text.contains("Do not TaskStop"));
    assert!(!text.contains("stopped</status>"));
}

#[tokio::test]
async fn acknowledges_empty_assistant_failure_without_cascade_stop() {
    let request = request(
            "<task-notification><status>failed</status><summary>Agent \"Parapper turn boundary split\" failed: No assistant messages found</summary></task-notification>".into(),
        );
    let response = acknowledge(&request);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("No assistant messages found"));
    assert!(text.contains("do not cascade TaskStop"));
    assert!(text.contains("in-flight scope key"));
}

#[tokio::test]
async fn acknowledge_with_blank_text_uses_default_notification() {
    let request = request("ignored".into());
    let response = acknowledge_with_text(&request, "   \n");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response["content"][0]["text"], DEFAULT_NOTIFICATION_TEXT);
}

#[tokio::test]
async fn task_notification_skips_empty_and_duplicate_summary_fields() {
    let request = request(
            "<task-notification><summary>   </summary><result>same note</result><summary>same note</summary></task-notification>"
                .into(),
        );
    let response = acknowledge(&request);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    let text = response["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, "same note");
}

#[tokio::test]
async fn enriches_no_completion_record_agent_failure_and_claude_stop() {
    let request = request(
            "<task-notification><summary>No completion record. Agent \"lane-a\" failed: timeout. Worker was stopped by Claude.</summary></task-notification>"
                .into(),
        );
    let response = acknowledge(&request);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("historical previous-session task IDs"));
    assert!(text.contains("do not cascade TaskStop"));
    assert!(text.contains("acknowledge the stop"));
}
