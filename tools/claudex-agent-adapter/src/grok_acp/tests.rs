use std::sync::{Arc, atomic::AtomicBool};

use agent_client_protocol::{self as acp, Client as _};
use serde_json::{json, value::RawValue};

use super::{
    COMMAND_QUEUE_CAPACITY, DriverCommand, GrokAcp, PreparedTurn, TURN_QUEUE_CAPACITY,
    client::AcpClient, prompt, updates,
};
use crate::app_server::events::ThreadEventDispatcher;

#[test]
fn converts_backend_prompts_and_effort() {
    assert_eq!(prompt::input_text(&json!("hello")), "hello");
    assert_eq!(
        prompt::input_text(&json!([{"type":"text","text":"one"},{"content":"two"}])),
        "one\ntwo"
    );
    assert_eq!(prompt::grok_effort("low"), Some("low"));
    assert_eq!(prompt::grok_effort("mid"), Some("medium"));
    assert_eq!(prompt::grok_effort("xhigh"), Some("high"));
    assert_eq!(prompt::grok_effort("invalid"), None);
    assert_eq!(prompt::copilot_effort("mid"), Some("medium"));
    assert_eq!(prompt::copilot_effort("xhigh"), Some("xhigh"));
    assert_eq!(prompt::copilot_effort("max"), Some("max"));
    assert_eq!(prompt::copilot_effort("invalid"), None);
    assert_eq!(prompt::input_text(&serde_json::Value::Null), "");
    assert_eq!(
        prompt::input_text(&json!({"key":"value"})),
        r#"{"key":"value"}"#
    );
}

#[test]
fn removes_codex_only_bridge_instructions() {
    let params = json!({
        "baseInstructions":"project rules\n\nbackend-only",
        "developerInstructions":"backend-only"
    });
    assert!(prompt::provider_instructions(&params, true).starts_with("project rules\n\n"));
    assert!(prompt::provider_instructions(&params, true).contains("claudex-medium"));
    assert!(prompt::provider_instructions(&json!({}), true).contains("claudex-xhigh"));
    assert_eq!(
        prompt::provider_instructions(&params, false),
        "project rules"
    );
}

#[tokio::test]
async fn falls_back_to_the_first_permission_or_cancels() {
    let client = AcpClient::new(Arc::new(ThreadEventDispatcher::default()));
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

#[tokio::test]
async fn client_accepts_extension_notifications() {
    let client = AcpClient::new(Arc::new(ThreadEventDispatcher::default()));
    let raw = RawValue::from_string("{}".to_owned()).unwrap();
    client
        .ext_notification(acp::ExtNotification::new("unrelated", Arc::from(raw)))
        .await
        .unwrap();
}

#[tokio::test]
async fn reports_a_closed_driver_for_each_command_response_type() {
    let (commands, receiver) = tokio::sync::mpsc::channel(1);
    drop(receiver);
    let agent = GrokAcp {
        commands,
        events: Arc::new(ThreadEventDispatcher::default()),
        alive: Arc::new(AtomicBool::new(false)),
    };

    assert!(agent.create_session(json!({})).await.is_err());
    assert!(agent.start_turn(json!({})).await.is_err());
}

#[tokio::test]
async fn bounded_queues_apply_backpressure_at_fixed_capacities() {
    let (commands, mut command_receiver) = tokio::sync::mpsc::channel(COMMAND_QUEUE_CAPACITY);
    for _ in 0..COMMAND_QUEUE_CAPACITY {
        let (response, _) = tokio::sync::oneshot::channel();
        commands
            .send(DriverCommand::StartTurn {
                params: json!({}),
                response,
            })
            .await
            .unwrap();
    }
    assert_eq!(commands.capacity(), 0);
    assert_eq!(command_receiver.len(), COMMAND_QUEUE_CAPACITY);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), commands.reserve())
            .await
            .is_err()
    );
    command_receiver.recv().await.unwrap();
    assert!(commands.reserve().await.is_ok());

    let (turns, mut turn_receiver) = tokio::sync::mpsc::channel(TURN_QUEUE_CAPACITY);
    for index in 0..TURN_QUEUE_CAPACITY {
        turns
            .send(PreparedTurn {
                session_id: format!("session-{index}"),
                prompt: "prompt".to_owned(),
                effort: None,
            })
            .await
            .unwrap();
    }
    assert_eq!(turns.capacity(), 0);
    assert_eq!(turn_receiver.len(), TURN_QUEUE_CAPACITY);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), turns.reserve())
            .await
            .is_err()
    );
    turn_receiver.recv().await.unwrap();
    assert!(turns.reserve().await.is_ok());
}

#[tokio::test]
async fn public_spawn_entry_points_report_a_missing_program() {
    let previous = std::env::var_os("CLAUDEX_GROK_PROGRAM");
    // No other unit test reads this provider-specific override.
    unsafe { std::env::set_var("CLAUDEX_GROK_PROGRAM", "/definitely/missing/grok") };
    let spawned = GrokAcp::spawn("model").await;
    if let Some(value) = previous {
        unsafe { std::env::set_var("CLAUDEX_GROK_PROGRAM", value) };
    } else {
        unsafe { std::env::remove_var("CLAUDEX_GROK_PROGRAM") };
    }
    assert!(spawned.is_err());

    assert!(
        GrokAcp::spawn_with_program(
            "model",
            "/definitely/missing/grok",
            std::env::current_dir().unwrap()
        )
        .await
        .is_err()
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
