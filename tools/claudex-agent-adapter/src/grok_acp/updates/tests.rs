use std::sync::Arc;

use agent_client_protocol::{self as acp};
use serde_json::{json, value::RawValue};

use super::{dispatch_error, dispatch_extension, dispatch_notification};
use crate::app_server::events::ThreadEventDispatcher;

#[tokio::test]
async fn forwards_thought_and_tool_progress_without_actionable_tool_calls() {
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
    assert_eq!(tool["method"], "item/reasoning/summaryTextDelta");
    assert!(
        tool["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("Search the web")
    );
}

#[tokio::test]
async fn forwards_terminal_tool_updates_and_errors() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for fields in [
        acp::ToolCallUpdateFields::new(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
        acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::Pending)
            .title("Pending"),
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

    let progress = receiver.recv().await.unwrap();
    let error = receiver.recv().await.unwrap();
    assert!(
        progress["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("Completed")
    );
    assert!(
        progress["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("Failed")
    );
    assert_eq!(error["params"]["error"]["message"], "provider failed");
}

#[tokio::test]
async fn forwards_xai_subagent_lifecycle_and_usage() {
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
    let lifecycle = receiver.recv().await.unwrap();
    let usage = receiver.recv().await.unwrap();
    assert!(
        lifecycle["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("grok-4.5")
    );
    assert!(
        lifecycle["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("medium effort")
    );
    assert!(
        lifecycle["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("1.2s")
    );
    assert_eq!(
        usage["params"]["tokenUsage"]["last"]["reasoningOutputTokens"],
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

    let lifecycle = receiver.recv().await.unwrap();
    let retry = receiver.recv().await.unwrap();
    assert!(
        lifecycle["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("SubAgent")
    );
    assert!(
        retry["params"]["delta"]
            .as_str()
            .unwrap()
            .contains("Retrying")
    );
}

fn dispatch_raw_extension(events: &ThreadEventDispatcher, params: serde_json::Value) {
    let raw = RawValue::from_string(params.to_string()).unwrap();
    dispatch_extension(
        events,
        acp::ExtNotification::new("_x.ai/session/update", Arc::from(raw)),
    );
}
