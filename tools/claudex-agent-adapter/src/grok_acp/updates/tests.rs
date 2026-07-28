use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{self as acp};
use serde_json::{json, value::RawValue};

use super::{ThoughtUnits, dispatch_extension, dispatch_notification};
use crate::app_server::events::{ThreadEventDispatcher, ThreadEvents};

fn thoughts() -> ThoughtUnits {
    ThoughtUnits::default()
}

#[test]
fn thought_units_handle_empty_chunks_and_bare_paragraph_breaks() {
    let units = thoughts();
    assert!(units.partition("session", "").is_empty());
    assert_eq!(
        units.partition("session", "open"),
        vec![(0, "open".to_owned())]
    );
    assert!(units.partition("bare", "\n\n").is_empty());
    assert!(units.partition("session", "\n\n").is_empty());
    assert_eq!(
        units.partition("session", "next"),
        vec![(1, "next".to_owned())]
    );
    units.break_after_interrupt("session");
    units.clear("session");
}

async fn drain(receiver: &ThreadEvents) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await
    {
        out.push(event);
    }
    out
}

#[tokio::test]
async fn forwards_thought_as_reasoning_and_tools_as_provider_cards() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new("thinking".into())),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
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
    assert_eq!(thought["params"]["itemId"], "session:reasoning");
    assert_eq!(thought["params"]["summaryIndex"], 0);
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
        &thoughts(),
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
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "output-progress",
                acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::Pending)
                    .raw_output(json!("output")),
            )),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "content-progress",
                acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::Pending)
                    .content(vec![text_content("content")]),
            )),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "anonymous-progress",
                acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Pending),
            )),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
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
    let messages = drain(&receiver).await;
    assert!(messages.iter().any(|event| {
        event["method"] == "item/providerTool/update"
            && event["params"]["status"] == "completed"
            && event["params"]["output"] == "file contents here"
    }));
    assert!(messages.iter().any(|event| {
        event["method"] == "item/providerTool/update" && event["params"]["status"] == "failed"
    }));
}

#[tokio::test]
async fn covers_tool_update_progress_and_empty_plan_paths() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "start",
                acp::ToolCallUpdateFields::new()
                    .title("Read")
                    .kind(acp::ToolKind::Read)
                    .status(acp::ToolCallStatus::InProgress)
                    .raw_input(json!({"path":"file"}))
                    .content(vec![text_content("chunk")])
                    .locations(vec![acp::ToolCallLocation::new("file")]),
            )),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "title-only",
                acp::ToolCallUpdateFields::new()
                    .title("Read")
                    .status(acp::ToolCallStatus::Pending),
            )),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                "content-only",
                acp::ToolCallUpdateFields::new().content(vec![text_content("ignored")]),
            )),
        ),
    );
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new(
                " ",
                acp::PlanEntryPriority::Low,
                acp::PlanEntryStatus::InProgress,
            )])),
        ),
    );

    let messages = drain(&receiver).await;
    assert!(messages.iter().any(|event| {
        event["method"] == "item/providerTool/call"
            && event["params"]["arguments"]["path"] == "file"
    }));
    assert!(messages.iter().any(|event| {
        event["method"] == "item/providerTool/update" && event["params"]["callId"] == "title-only"
    }));
    assert!(!messages.iter().any(|event| {
        event["method"] == "item/providerTool/update" && event["params"]["callId"] == "content-only"
    }));
    assert!(messages.iter().any(|event| {
        event["method"] == "item/agentMessage/delta" && event["params"]["delta"] == "\nPlan 0/1\n"
    }));
}

fn text_content(value: &str) -> acp::ToolCallContent {
    acp::ContentBlock::Text(acp::TextContent::new(value)).into()
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
            &thoughts(),
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

#[tokio::test]
async fn forwards_plan_as_checklist_text() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    dispatch_notification(
        &events,
        &thoughts(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::Plan(acp::Plan::new(vec![
                acp::PlanEntry::new(
                    "Investigate",
                    acp::PlanEntryPriority::High,
                    acp::PlanEntryStatus::Completed,
                ),
                acp::PlanEntry::new(
                    "Implement",
                    acp::PlanEntryPriority::Medium,
                    acp::PlanEntryStatus::InProgress,
                ),
            ])),
        ),
    );
    let plan = receiver.recv().await.unwrap();
    assert_eq!(plan["method"], "item/agentMessage/delta");
    let text = plan["params"]["delta"].as_str().unwrap();
    // Compact plan status (not a full checklist dump).
    assert!(text.contains("Plan 1/2"), "text={text}");
    assert!(text.contains("Implement"), "text={text}");
    assert!(!text.contains("Investigate"), "text={text}");
}

#[tokio::test]
async fn ignores_mode_and_session_title_chatter() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for update in [
        acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new("review")),
        acp::SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title("Work")),
        acp::SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title("")),
        acp::SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new()),
    ] {
        dispatch_notification(
            &events,
            &thoughts(),
            acp::SessionNotification::new("session", update),
        );
    }
    let drained = drain(&receiver).await;
    assert!(
        drained.is_empty(),
        "mode/title updates must not spam the bridge: {drained:?}"
    );
}

#[test]
fn ignores_unrelated_or_unstructured_extensions() {
    let events = ThreadEventDispatcher::default();
    for (method, payload) in [("other/method", "{}"), ("_x.ai/session/update", "\"text\"")] {
        let raw = RawValue::from_string(payload.to_owned()).unwrap();
        dispatch_extension(
            &events,
            &thoughts(),
            acp::ExtNotification::new(method, Arc::from(raw)),
        );
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
        &thoughts(),
        acp::ExtNotification::new("_x.ai/session/update", Arc::from(raw)),
    );
}
