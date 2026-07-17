use std::sync::Arc;

use agent_client_protocol::{self as acp, Client as _};
use serde_json::json;

use super::{AcpClient, grok_effort, input_text, provider_instructions, updates};
use crate::app_server::events::ThreadEventDispatcher;

#[test]
fn converts_backend_prompts_and_effort() {
    assert_eq!(input_text(&json!("hello")), "hello");
    assert_eq!(
        input_text(&json!([{"type":"text","text":"one"},{"content":"two"}])),
        "one\ntwo"
    );
    assert_eq!(grok_effort("low"), Some("low"));
    assert_eq!(grok_effort("mid"), Some("medium"));
    assert_eq!(grok_effort("xhigh"), Some("high"));
    assert_eq!(grok_effort("invalid"), None);
    assert_eq!(input_text(&serde_json::Value::Null), "");
    assert_eq!(input_text(&json!({"key":"value"})), r#"{"key":"value"}"#);
}

#[test]
fn removes_codex_only_bridge_instructions() {
    let params = json!({
        "baseInstructions":"project rules\n\nbackend-only",
        "developerInstructions":"backend-only"
    });
    assert!(provider_instructions(&params).starts_with("project rules\n\n"));
    assert!(provider_instructions(&params).contains("claudex-medium"));
    assert!(provider_instructions(&json!({})).contains("claudex-xhigh"));
}

#[tokio::test]
async fn falls_back_to_the_first_permission_or_cancels() {
    let client = AcpClient {
        events: Arc::new(ThreadEventDispatcher::default()),
    };
    let request = permission_request(vec![acp::PermissionOption::new(
        "reject",
        "Reject",
        acp::PermissionOptionKind::RejectOnce,
    )]);
    let selected = client.request_permission(request).await.unwrap();
    assert_eq!(
        serde_json::to_value(selected).unwrap()["outcome"]["optionId"],
        json!("reject")
    );
    let cancelled = client
        .request_permission(permission_request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(cancelled).unwrap()["outcome"]["outcome"],
        json!("cancelled")
    );
}

fn permission_request(options: Vec<acp::PermissionOption>) -> acp::RequestPermissionRequest {
    acp::RequestPermissionRequest::new(
        "session",
        acp::ToolCallUpdate::new("tool", acp::ToolCallUpdateFields::new()),
        options,
    )
}

#[tokio::test]
async fn ignores_non_agent_non_text_and_empty_notification_chunks() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for update in [
        acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("user"),
        ))),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Image(
            acp::ImageContent::new("data", "image/png"),
        ))),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(""),
        ))),
    ] {
        updates::dispatch_notification(&events, acp::SessionNotification::new("session", update));
    }
    updates::dispatch_notification(
        &events,
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new("visible"),
            ))),
        ),
    );
    assert_eq!(receiver.recv().await.unwrap()["params"]["delta"], "visible");
}
