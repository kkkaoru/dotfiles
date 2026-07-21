use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{self as acp};
use serde_json::{json, value::RawValue};

use super::{dispatch_extension, dispatch_notification};
use crate::app_server::events::{ThreadEventDispatcher, ThreadEvents};

async fn drain(receiver: &ThreadEvents) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await {
            Ok(Some(event)) => out.push(event),
            Ok(None) | Err(_) => break,
        }
    }
    out
}

#[tokio::test]
async fn forwards_thought_as_reasoning_and_tools_as_provider_cards() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    dispatch_notification(
        &events,
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new("thinking".into())),
        ),
    );
    dispatch_notification(
        &events,
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCall(
                acp::ToolCall::new("call-1", "read_file")
                    .kind(acp::ToolKind::Read)
                    .raw_input(json!({"path":"/tmp/a"})),
            ),
        ),
    );
    let thought = receiver.recv().await.unwrap();
    let tool = receiver.recv().await.unwrap();
    assert_eq!(thought["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thought["params"]["delta"], "thinking");
    assert_eq!(tool["method"], "item/providerTool/call");
    assert_eq!(tool["params"]["tool"], "Read");
    assert_eq!(tool["params"]["arguments"]["path"], "/tmp/a");
}

#[tokio::test]
async fn forwards_tool_status_updates_with_output() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    dispatch_notification(
        &events,
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "call-1",
                acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::Completed)
                    .title("Read")
                    .raw_output(json!("file contents here")),
            )),
        ),
    );
    dispatch_notification(
        &events,
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "call-2",
                acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::Failed)
                    .title("Bash")
                    .raw_output(json!("exit 1")),
            )),
        ),
    );
    let completed = receiver.recv().await.unwrap();
    let failed = receiver.recv().await.unwrap();
    assert_eq!(completed["method"], "item/providerTool/update");
    assert_eq!(completed["params"]["status"], "completed");
    assert_eq!(completed["params"]["output"], "file contents here");
    assert_eq!(failed["params"]["status"], "failed");
}

#[tokio::test]
async fn forwards_xai_subagent_lifecycle_as_visible_message() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for update in [
        json!({"sessionUpdate":"subagent_spawned","description":"Research AVITA",
            "model":"grok-4.5","reasoning_effort":"medium"}),
        json!({"sessionUpdate":"subagent_finished","status":"completed","duration_ms":1250}),
        json!({"sessionUpdate":"turn_completed","usage":{
            "inputTokens":10,"outputTokens":20,"reasoningTokens":3
        }}),
    ] {
        let params = json!({"sessionId":"session","update":update});
        let raw = RawValue::from_string(params.to_string()).unwrap();
        dispatch_extension(
            &events,
            acp::ExtNotification::new("_x.ai/session/update", Arc::from(raw)),
        );
    }
    let drained = drain(&receiver).await;
    let text: String = drained
        .iter()
        .filter(|e| e["method"] == "item/agentMessage/delta")
        .filter_map(|e| e["params"]["delta"].as_str())
        .collect();
    assert!(text.contains("grok-4.5"), "text={text}");
    assert!(text.contains("1.2s"), "text={text}");
    assert!(
        drained
            .iter()
            .any(|event| event["method"] == "thread/tokenUsage/updated")
    );
}

#[test]
fn ignores_unrelated_or_unstructured_extensions() {
    let events = ThreadEventDispatcher::default();
    for (method, payload) in [("other/method", "{}"), ("_x.ai/session/update", "\"text\"")] {
        let raw = RawValue::from_string(payload.to_owned()).unwrap();
        dispatch_extension(&events, acp::ExtNotification::new(method, Arc::from(raw)));
    }
}

#[tokio::test]
async fn covers_extension_defaults_retries_and_missing_usage() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for params in [
        json!({}),
        json!({"sessionId":"session"}),
        json!({"sessionId":"session","update":{}}),
        json!({"sessionId":"session","update":{"sessionUpdate":"subagent_spawned"}}),
        json!({"sessionId":"session","update":{"sessionUpdate":"subagent_finished"}}),
        json!({"sessionId":"session","update":{"sessionUpdate":"retry_state"}}),
        json!({"sessionId":"session","update":{"sessionUpdate":"retry_state",
            "attempt":2,"max_retries":4}}),
        json!({"sessionId":"session","update":{"sessionUpdate":"turn_completed"}}),
    ] {
        dispatch_raw_extension(&events, params);
    }

    let drained = drain(&receiver).await;
    let text: String = drained
        .iter()
        .filter(|e| e["method"] == "item/agentMessage/delta")
        .filter_map(|e| e["params"]["delta"].as_str())
        .collect();
    assert!(text.contains("SubAgent"), "text={text}");
    assert!(text.contains("Retrying"), "text={text}");
    assert!(text.contains("2/4"), "text={text}");
}

fn dispatch_raw_extension(events: &ThreadEventDispatcher, params: serde_json::Value) {
    let raw = RawValue::from_string(params.to_string()).unwrap();
    dispatch_extension(
        events,
        acp::ExtNotification::new("_x.ai/session/update", Arc::from(raw)),
    );
}
