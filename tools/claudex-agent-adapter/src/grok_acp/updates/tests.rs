use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{self as acp};
use serde_json::{json, value::RawValue};

use super::{dispatch_error, dispatch_extension, dispatch_notification};
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

fn joined_deltas(events: &[serde_json::Value]) -> String {
    events
        .iter()
        .filter(|event| event["method"] == "item/agentMessage/delta")
        .filter_map(|event| event["params"]["delta"].as_str())
        .collect()
}

#[tokio::test]
async fn forwards_thought_as_reasoning_and_tool_progress_as_message() {
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
            acp::SessionUpdate::ToolCall(acp::ToolCall::new("call", "Search the web")),
        ),
    );
    let thought = receiver.recv().await.unwrap();
    let tool = receiver.recv().await.unwrap();
    assert_eq!(thought["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thought["params"]["delta"], "thinking");
    // Tool progress must stay on agentMessage so it is not dropped after the
    // first visible text block (thinking path is gated by has_visible_output).
    assert_eq!(tool["method"], "item/agentMessage/delta");
    assert!(
        tool["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("Search the web")
    );
}

#[tokio::test]
async fn forwards_terminal_and_in_progress_tool_updates() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for fields in [
        acp::ToolCallUpdateFields::new(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
        acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::Pending)
            .title("Pending job"),
        acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::InProgress)
            .title("Running job"),
        acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::Completed)
            .title("Search complete"),
        acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::Failed)
            .title("Search failed"),
    ] {
        dispatch_notification(
            &events,
            acp::SessionNotification::new(
                "session",
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new("call", fields)),
            ),
        );
    }
    dispatch_error(&events, "session", "provider failed".to_owned());

    // Same-method agentMessage deltas coalesce in the event queue.
    let progress = receiver.recv().await.unwrap();
    let error = receiver.recv().await.unwrap();
    assert_eq!(progress["method"], "item/agentMessage/delta");
    let delta = progress["params"]["delta"].as_str().unwrap();
    assert!(delta.contains("Pending"), "delta={delta}");
    assert!(
        delta.contains("Running") || delta.contains("Completed"),
        "delta={delta}"
    );
    assert!(delta.contains("Failed"), "delta={delta}");
    assert_eq!(error["params"]["error"]["message"], "provider failed");
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
    let text = joined_deltas(&drained);
    assert!(text.contains("grok-4.5"), "text={text}");
    assert!(text.contains("medium effort"), "text={text}");
    assert!(text.contains("1.2s"), "text={text}");
    assert!(
        drained
            .iter()
            .any(|event| event["method"] == "thread/tokenUsage/updated")
    );
    assert_eq!(
        drained
            .iter()
            .find(|event| event["method"] == "thread/tokenUsage/updated")
            .unwrap()["params"]["tokenUsage"]["last"]["reasoningOutputTokens"],
        3
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
    let text = joined_deltas(&drained);
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
