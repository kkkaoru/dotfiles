use std::sync::{Arc, atomic::AtomicBool};

use agent_client_protocol::{self as acp, Client as _};
use serde_json::{json, value::RawValue};

use super::{
    COMMAND_QUEUE_CAPACITY, DriverCommand, GrokAcp, PreparedTurn, TURN_QUEUE_CAPACITY,
    client::AcpClient, prompt, turns::drive_turn_tasks, updates,
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
        turn_permits: Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY)),
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
                permit: Arc::new(tokio::sync::Semaphore::new(1))
                    .acquire_owned()
                    .await
                    .unwrap(),
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

    let permits = Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY));
    let (turns, mut turn_receiver) = tokio::sync::mpsc::channel(TURN_QUEUE_CAPACITY);
    for index in 0..TURN_QUEUE_CAPACITY {
        turns
            .send(PreparedTurn {
                session_id: format!("session-{index}"),
                prompt: "prompt".to_owned(),
                effort: None,
                _permit: Arc::clone(&permits).acquire_owned().await.unwrap(),
            })
            .await
            .unwrap();
    }
    assert_eq!(permits.available_permits(), 0);
    assert_eq!(turns.capacity(), 0);
    assert_eq!(turn_receiver.len(), TURN_QUEUE_CAPACITY);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            Arc::clone(&permits).acquire_owned(),
        )
        .await
        .is_err()
    );
    turn_receiver.recv().await.unwrap();
    assert!(Arc::clone(&permits).acquire_owned().await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn turn_worker_runs_local_tasks_concurrently_and_cleans_them_up() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let permits = Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY));
            let (turns, receiver) = tokio::sync::mpsc::channel(TURN_QUEUE_CAPACITY);
            let active = std::rc::Rc::new(std::cell::Cell::new(0));
            let peak = std::rc::Rc::new(std::cell::Cell::new(0));
            let both_started = std::rc::Rc::new(tokio::sync::Notify::new());
            let release = std::rc::Rc::new(tokio::sync::Notify::new());
            let worker = tokio::task::spawn_local(drive_turn_tasks(receiver, {
                let active = std::rc::Rc::clone(&active);
                let peak = std::rc::Rc::clone(&peak);
                let both_started = std::rc::Rc::clone(&both_started);
                let release = std::rc::Rc::clone(&release);
                move |_turn| {
                    let active = std::rc::Rc::clone(&active);
                    let peak = std::rc::Rc::clone(&peak);
                    let both_started = std::rc::Rc::clone(&both_started);
                    let release = std::rc::Rc::clone(&release);
                    async move {
                        let count = active.get() + 1;
                        active.set(count);
                        peak.set(peak.get().max(count));
                        if count == 2 {
                            both_started.notify_one();
                        }
                        release.notified().await;
                        active.set(active.get() - 1);
                    }
                }
            }));
            for session_id in ["one", "two"] {
                turns
                    .send(PreparedTurn {
                        session_id: session_id.to_owned(),
                        prompt: String::new(),
                        effort: None,
                        _permit: Arc::clone(&permits).acquire_owned().await.unwrap(),
                    })
                    .await
                    .unwrap();
            }
            tokio::time::timeout(std::time::Duration::from_secs(1), both_started.notified())
                .await
                .expect("turn tasks did not overlap");
            assert_eq!(peak.get(), 2);
            drop(turns);
            tokio::time::timeout(std::time::Duration::from_secs(1), worker)
                .await
                .expect("turn worker did not abort active tasks")
                .unwrap();
            assert_eq!(permits.available_permits(), TURN_QUEUE_CAPACITY);
        })
        .await;
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
