use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    sync::Arc,
    time::Instant,
};

use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore, mpsc};

use super::*;
use crate::agent_backend::AgentBackend;
use crate::anthropic::{
    Bridge, Session,
    bridge_types::{LaunchAvailabilityState, LaunchCapabilitySummary},
};

fn test_session(claude_session_id: Option<&str>) -> Session {
    test_session_with_tools(claude_session_id, [("Agent", "Agent")], false)
}

fn test_session_with_tools<const N: usize>(
    claude_session_id: Option<&str>,
    external_tools: [(&str, &str); N],
    launch_unavailable: bool,
) -> Session {
    let slots = Arc::new(Semaphore::new(1));
    Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: external_tools
            .into_iter()
            .map(|(dynamic, native)| (dynamic.to_owned(), native.to_owned()))
            .collect(),
        launch_availability: LaunchAvailabilityState::from_summary(LaunchCapabilitySummary::new(
            launch_unavailable,
        )),
        client_user_id: None,
        claude_session_id: claude_session_id.map(str::to_owned),
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    }
}

const LAUNCH_UNAVAILABLE_NOTICE: &str =
    "Agent/Task is unavailable in this session, so this SubAgent was not started";

fn native_launch_event(method: &str, call_id: &str, tool: &str, arguments: Value) -> Value {
    json!({
        "method": method,
        "params": {
            "callId": call_id,
            "tool": tool,
            "title": tool,
            "status": "pending",
            "arguments": arguments
        }
    })
}

fn drain_stream(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> String {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .map(|frame| String::from_utf8_lossy(&frame.expect("stream frame")).into_owned())
        .collect()
}

fn assert_notice_uses_text_delta(frames: &str, notice: &str) {
    let notice_frame = frames
        .split("\n\n")
        .find(|frame| frame.contains(notice))
        .expect("notice SSE frame");
    assert!(
        notice_frame.contains(r#""type":"text_delta""#),
        "{notice_frame}"
    );
    assert!(!notice_frame.contains("thinking_delta"), "{notice_frame}");
    assert!(
        !notice_frame.contains("claudex_provider_progress"),
        "{notice_frame}"
    );
}

async fn settled_bridge() -> Bridge {
    let leaf = Arc::new(AgentBackend::Pi(
        crate::pi_gateway::PiGateway::alive_for_test(),
    ));
    let backend = AgentBackend::routed(vec![("main".to_owned(), leaf)]);
    let mut catalog = crate::provider_config::ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "worker", "main", "high",
        )])
        .expect("worker catalog");
    Bridge::new_with_backend(backend, "main".to_owned()).with_model_catalog(catalog)
}

#[tokio::test]
async fn unavailable_notice_is_committed_text_and_one_shot_across_call_and_update() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session_with_tools(Some("scope-unavailable"), [("Bash", "Bash")], true);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1);

    for (method, call_id) in [
        ("item/providerTool/call", "native-agent-call"),
        ("item/providerTool/update", "native-agent-update"),
        ("item/providerTool/call", "native-task-call"),
    ] {
        builder
            .provider_launch_event(
                &bridge,
                &session,
                &[],
                &json!(null),
                &native_launch_event(method, call_id, "Agent", json!({})),
                Some(&sender),
            )
            .await
            .expect("native launch attempt");
    }

    let live = drain_stream(&mut receiver);
    assert_eq!(live.matches(LAUNCH_UNAVAILABLE_NOTICE).count(), 1, "{live}");
    assert!(live.contains("Try another model or session that offers Agent/Task"));
    assert_notice_uses_text_delta(&live, LAUNCH_UNAVAILABLE_NOTICE);
    let segment = builder.finish(Some(&sender)).await.expect("finish");
    let finish = drain_stream(&mut receiver);
    assert!(!finish.contains(LAUNCH_UNAVAILABLE_NOTICE), "{finish}");
    assert_incomplete_launch_committed_text(&segment.blocks, &[LAUNCH_UNAVAILABLE_NOTICE]);
    assert!(
        !serde_json::to_string(&segment.blocks)
            .expect("serialize segment")
            .contains("awaiting its prompt")
    );
    assert!(
        !serde_json::to_string(&segment.blocks)
            .expect("serialize segment")
            .contains("never received a prompt")
    );
}

#[tokio::test]
async fn unavailable_notice_uses_the_same_committed_path_without_streaming() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session_with_tools(Some("scope-nonstream"), [("Bash", "Bash")], true);
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "nonstream-agent",
                "Agent",
                json!({}),
            ),
            None,
        )
        .await
        .expect("nonstream native launch attempt");
    let segment = builder.finish(None).await.expect("finish");
    assert_incomplete_launch_committed_text(&segment.blocks, &[LAUNCH_UNAVAILABLE_NOTICE]);
}

#[tokio::test]
async fn unavailable_notice_requires_an_exact_native_launch_attempt() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session_with_tools(Some("scope-nonlaunch"), [("Bash", "Bash")], true);
    let mut builder = SegmentBuilder::new(1);

    for event in [
        native_launch_event("item/providerTool/call", "shell", "shell", json!({})),
        native_launch_event("item/providerTool/call", "lower-agent", "agent", json!({})),
        json!({
            "method":"item/providerTool/call",
            "params": {
                "callId":"agent-in-description",
                "tool":"shell",
                "title":"run Agent helper",
                "arguments": {"_toolName":"helper"}
            }
        }),
    ] {
        builder
            .provider_launch_event(&bridge, &session, &[], &json!(null), &event, None)
            .await
            .expect("nonlaunch provider event");
    }

    let segment = builder.finish(None).await.expect("finish");
    assert!(
        !serde_json::to_string(&segment.blocks)
            .expect("serialize segment")
            .contains(LAUNCH_UNAVAILABLE_NOTICE),
        "nonlaunch activity must not consume or emit the unavailable notice"
    );
    assert!(
        session.take_launch_unavailable_notice(),
        "nonlaunch activity must leave the one-shot notice available"
    );
}

#[tokio::test]
async fn supplied_agent_with_prompt_bridges_without_unavailable_notice() {
    let bridge = settled_bridge().await;
    let session =
        test_session_with_tools(Some("scope-supplied-agent"), [("Agent", "Agent")], false);
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "supplied-agent",
                "Agent",
                json!({
                    "prompt":"perform the bounded check",
                    "subagent_type":"worker",
                    "claudex_model":"main",
                    "claudex_effort":"high"
                }),
            ),
            None,
        )
        .await
        .expect("bridge supplied Agent");
    assert_eq!(builder.bridged_provider_launch_ids, ["supplied-agent"]);
    let segment = builder.finish(None).await.expect("finish");
    assert!(
        !serde_json::to_string(&segment.blocks)
            .expect("serialize segment")
            .contains(LAUNCH_UNAVAILABLE_NOTICE)
    );
}

#[tokio::test]
async fn supplied_task_keeps_existing_incomplete_notice_without_unavailable_notice() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let mut session =
        test_session_with_tools(Some("scope-supplied-task"), [("Task", "Task")], false);
    session
        .external_tool_names
        .insert("Task".to_owned(), "Task".to_owned());
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "supplied-task",
                "Task",
                json!({"_toolName":"Task"}),
            ),
            None,
        )
        .await
        .expect("unconfirmed supplied Task");
    let segment = builder.finish(None).await.expect("finish");
    let committed = serde_json::to_string(&segment.blocks).expect("serialize segment");
    assert!(committed.contains("awaiting its prompt"), "{committed}");
    assert!(committed.contains("never received a prompt"), "{committed}");
    assert!(
        !committed.contains(LAUNCH_UNAVAILABLE_NOTICE),
        "{committed}"
    );
}

#[tokio::test]
async fn inconclusive_mcp_task_without_mapping_keeps_existing_incomplete_notice() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session =
        test_session_with_tools(Some("scope-inconclusive-task"), [("Bash", "Bash")], false);
    let mut builder = SegmentBuilder::new(1);
    let event = json!({
        "method":"item/providerTool/call",
        "params": {
            "callId":"inconclusive-mcp-task",
            "tool":"mcp",
            "title":"Task",
            "status":"pending",
            "arguments":{"_toolName":"Task"}
        }
    });
    builder
        .provider_launch_event(&bridge, &session, &[], &json!(null), &event, None)
        .await
        .expect("inconclusive MCP Task");
    let segment = builder.finish(None).await.expect("finish");
    let committed = serde_json::to_string(&segment.blocks).expect("serialize segment");

    assert!(committed.contains("awaiting its prompt"), "{committed}");
    assert!(committed.contains("never received a prompt"), "{committed}");
    assert!(committed.contains("inconclusive-mcp-task"), "{committed}");
    assert!(
        !committed.contains(LAUNCH_UNAVAILABLE_NOTICE),
        "{committed}"
    );
}

#[tokio::test]
async fn unavailable_native_task_has_no_misleading_incomplete_notice_or_secret_echo() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session_with_tools(Some("scope-task-unavailable"), [("Bash", "Bash")], true);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "mcp-secret-call-id",
                "Task",
                json!({"_toolName":"Task", "prompt":"mcp-secret-prompt"}),
            ),
            Some(&sender),
        )
        .await
        .expect("unavailable Task attempt");
    let live = drain_stream(&mut receiver);
    let segment = builder.finish(Some(&sender)).await.expect("finish");
    let finish = drain_stream(&mut receiver);
    let committed = serde_json::to_string(&segment.blocks).expect("serialize segment");

    assert_eq!(live.matches(LAUNCH_UNAVAILABLE_NOTICE).count(), 1, "{live}");
    assert!(!finish.contains("never received a prompt"), "{finish}");
    assert!(!committed.contains("awaiting its prompt"), "{committed}");
    assert!(
        !committed.contains("never received a prompt"),
        "{committed}"
    );
    assert!(!committed.contains("mcp-secret"), "{committed}");
    assert_incomplete_launch_committed_text(&segment.blocks, &[LAUNCH_UNAVAILABLE_NOTICE]);
}

#[tokio::test]
async fn incomplete_task_launch_stays_actionable_text_live_and_after_sanitize() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let mut session = test_session(Some("scope-task"));
    session
        .external_tool_names
        .insert("Task".to_owned(), "Task".to_owned());
    let event = json!({
        "method": "item/providerTool/call",
        "params": {
            "callId": "task-awaiting-prompt",
            "tool": "Task",
            "title": "Task",
            "status": "pending",
            "arguments": {"_toolName": "Task"}
        }
    });
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(&bridge, &session, &[], &json!(null), &event, Some(&sender))
        .await
        .expect("unconfirmed Task open");
    builder
        .provider_launch_event(&bridge, &session, &[], &json!(null), &event, Some(&sender))
        .await
        .expect("replayed unconfirmed Task open");

    let mut before_finish = String::new();
    while let Ok(frame) = receiver.try_recv() {
        before_finish.push_str(&String::from_utf8_lossy(&frame.expect("live frame")));
    }
    assert_eq!(
        before_finish
            .matches("This SubAgent launch is awaiting its prompt")
            .count(),
        1
    );
    assert!(before_finish.contains("Send Agent/Task again with a non-empty prompt"));
    assert_text_notice_stream(&before_finish);
    assert!(!before_finish.contains("were not started"));

    let segment = builder.finish(Some(&sender)).await.expect("finish");
    let mut finish_frames = String::new();
    while let Ok(frame) = receiver.try_recv() {
        finish_frames.push_str(&String::from_utf8_lossy(&frame.expect("finish frame")));
    }
    assert!(
        finish_frames.contains("task-awaiting-prompt"),
        "{finish_frames}"
    );
    assert!(
        finish_frames.contains("were not started"),
        "{finish_frames}"
    );
    assert_text_notice_stream(&finish_frames);
    assert_incomplete_launch_committed_text(
        &segment.blocks,
        &[
            "This SubAgent launch is awaiting its prompt",
            "never received a prompt and were not started",
        ],
    );
    assert!(
        builder.incomplete_launch_call_ids.is_empty(),
        "reported incomplete launches must clear"
    );
    assert_eq!(
        builder.last_turn_progress.len(),
        1,
        "{:?}",
        builder.last_turn_progress
    );
    assert_eq!(builder.last_turn_progress[0].id, "task-awaiting-prompt");
    assert_eq!(builder.last_turn_progress[0].status, "dropped");
    builder.publish_turn_progress(&session);
    let progress = session.turn_progress.lock().expect("turn progress");
    assert_eq!(progress.len(), 1, "{progress:?}");
    assert_eq!(progress[0].id, "task-awaiting-prompt");
    assert_eq!(progress[0].status, "dropped");
}

fn assert_text_notice_stream(frames: &str) {
    assert!(frames.contains(r#""type":"text_delta""#), "{frames}");
    assert!(!frames.contains(r#""type":"thinking_delta""#), "{frames}");
    assert!(!frames.contains("claudex_provider_progress"), "{frames}");
}

fn assert_exact_text_notice_frames(frames: &[String], expected_fragments: &[&str]) {
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.lines().next().expect("SSE event"))
            .collect::<Vec<_>>(),
        vec![
            "event: content_block_start",
            "event: content_block_delta",
            "event: content_block_stop",
        ],
        "finish must emit exactly one text block"
    );
    let payloads = frames
        .iter()
        .map(|frame| {
            serde_json::from_str::<Value>(
                frame
                    .strip_prefix("event: ")
                    .and_then(|frame| frame.split_once("\ndata: ").map(|(_, data)| data.trim()))
                    .expect("SSE payload"),
            )
            .expect("SSE JSON")
        })
        .collect::<Vec<_>>();
    assert_eq!(payloads[0]["content_block"]["type"], "text");
    assert_eq!(payloads[1]["delta"]["type"], "text_delta");
    assert_eq!(payloads[2], json!({"type":"content_block_stop","index":0}));
    let stream = frames.concat();
    for expected in expected_fragments {
        assert!(stream.contains(expected), "{stream}");
    }
    assert!(!stream.contains("thinking_delta"), "{stream}");
    assert!(!stream.contains("tool_use"), "{stream}");
    assert!(!stream.contains("claudex_provider_progress"), "{stream}");
}

fn assert_incomplete_launch_committed_text(blocks: &[Value], expected_fragments: &[&str]) {
    let committed_text = blocks
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in expected_fragments {
        assert!(committed_text.contains(expected), "{blocks:?}");
    }
    assert!(
        blocks.iter().all(|block| block["type"] != "thinking"),
        "incomplete launch notices must not become thinking chrome: {blocks:?}"
    );
    assert!(
        !serde_json::to_string(blocks)
            .expect("serialize committed segment")
            .contains("claudex_provider_progress"),
        "incomplete launch notices must survive without a thinking signature: {blocks:?}"
    );
}

#[tokio::test]
async fn drain_remaining_queued_launches_bridges_session_queue() {
    let root = tempfile::tempdir().expect("launch queue fixture");
    let queue_dir = root.path().join("cache");
    std::fs::create_dir_all(&queue_dir).expect("queue dir");
    let queue = queue_dir.join("launch-queue.jsonl");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs_f64();
    // Session-owned queues live beside the base path as launch-queue.<owner>.jsonl.
    let owned = queue_dir.join("launch-queue.scope-a.jsonl");
    std::fs::write(
        &owned,
        format!(
            "{}\n",
            json!({
                "ts": now,
                "name": "Agent",
                "owner": "scope-a",
                "arguments": {"prompt": "queued worker"}
            })
        ),
    )
    .expect("queue entry");
    let _guard = temporary_env::set_var("CLAUDEX_LAUNCH_QUEUE", &queue);

    // Use a settled Copilot leaf so tool_call rejection is not required to
    // exercise drain / queued_launch_tool_call / record paths.
    let leaf = Arc::new(AgentBackend::Pi(
        crate::pi_gateway::PiGateway::alive_for_test(),
    ));
    let backend = AgentBackend::routed(vec![("main".to_owned(), leaf)]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session(Some("scope-a"));
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .drain_remaining_queued_launches(&bridge, &session, &[], &json!(null), None)
        .await;
    assert!(
        !builder.bridged_provider_launch_ids.is_empty(),
        "queued Agent must bridge into a provider launch id before tool routing"
    );
}

#[tokio::test]
async fn provider_launch_tracks_unbridged_mcp_cards() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session(Some("scope-b"));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &json!({
                "method": "item/providerTool/call",
                "params": {
                    "callId": "mcp-empty",
                    "tool": "mcp",
                    "title": "MCP",
                    "status": "pending",
                    "arguments": {}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("mcp card");
    assert!(
        matches!(receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "unbridged MCP card must wait for finish before emitting a warning"
    );
    assert!(
        builder
            .mcp_provider_call_ids
            .iter()
            .any(|id| id == "mcp-empty")
    );
    assert!(
        builder
            .incomplete_launch_call_ids
            .iter()
            .any(|id| id == "mcp-empty")
    );

    let segment = builder
        .finish(Some(&sender))
        .await
        .expect("finish mcp card");
    let finish_frames = std::iter::from_fn(|| receiver.try_recv().ok())
        .map(|frame| String::from_utf8_lossy(&frame.expect("finish frame")).into_owned())
        .collect::<Vec<_>>();
    assert_exact_text_notice_frames(
        &finish_frames,
        &["mcp-empty", "never received a prompt and were not started"],
    );
    assert_incomplete_launch_committed_text(
        &segment.blocks,
        &["mcp-empty", "never received a prompt and were not started"],
    );
    assert!(
        builder.incomplete_launch_call_ids.is_empty(),
        "reported incomplete launches must clear"
    );
    assert_eq!(builder.last_turn_progress.len(), 1);
    assert_eq!(builder.last_turn_progress[0].id, "mcp-empty");
    assert_eq!(builder.last_turn_progress[0].status, "dropped");
}

#[tokio::test]
async fn provider_launch_forwards_non_mcp_call_and_update_statuses() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session(Some("scope-c"));
    let mut builder = SegmentBuilder::new(1);

    for (method, status) in [
        ("item/providerTool/call", "pending"),
        ("item/providerTool/update", "in_progress"),
        ("item/providerTool/update", "completed"),
        ("item/providerTool/update", "failed"),
        ("item/providerTool/update", "unknown"),
    ] {
        builder
            .provider_launch_event(
                &bridge,
                &session,
                &[],
                &json!(null),
                &json!({
                    "method": method,
                    "params": {
                        "callId": "ordinary-call",
                        "tool": "shell",
                        "title": "ordinary tool",
                        "status": status,
                        "arguments": {}
                    }
                }),
                None,
            )
            .await
            .expect("ordinary provider event");
    }
}

#[test]
fn provider_launch_helpers_deduplicate_mcp_and_incomplete_ids() {
    let mut builder = SegmentBuilder::new(1);
    builder.note_mcp_provider_call(None, true);
    builder.note_mcp_provider_call(Some("ordinary"), false);
    builder.note_mcp_provider_call(Some("mcp"), true);
    builder.note_mcp_provider_call(Some("mcp"), true);
    assert_eq!(builder.mcp_provider_call_ids, vec!["mcp"]);

    assert!(builder.track_incomplete_launch("pending"));
    assert!(!builder.track_incomplete_launch("pending"));
    builder
        .bridged_provider_launch_ids
        .push("bridged".to_owned());
    assert!(!builder.track_incomplete_launch("bridged"));
    assert_eq!(builder.incomplete_launch_call_ids, vec!["pending"]);
}

#[test]
fn unconfirmed_task_launch_recognizes_each_provider_tool_location() {
    let session = test_session(Some("scope-task-detection"));
    let cases = [
        (
            "params absent",
            json!({"method": "item/providerTool/call"}),
            false,
        ),
        (
            "tool mismatch but title confirms",
            json!({
                "params": {"tool": "shell", "title": "Task", "arguments": {}}
            }),
            true,
        ),
        (
            "arguments tool name confirms",
            json!({
                "params": {
                    "tool": "shell",
                    "title": "worker",
                    "arguments": {"_toolName": "task"}
                }
            }),
            true,
        ),
        (
            "all task identifiers mismatch",
            json!({
                "params": {
                    "tool": "shell",
                    "title": "worker",
                    "arguments": {"_toolName": "Agent"}
                }
            }),
            false,
        ),
    ];

    for (name, event, expected) in cases {
        assert_eq!(
            is_unconfirmed_task_launch(&session, &event),
            expected,
            "{name}"
        );
    }
}

#[test]
fn exact_native_launch_attempt_recognizes_only_exact_agent_or_task_locations() {
    let cases = [
        (
            "params absent",
            json!({"method":"item/providerTool/call"}),
            false,
        ),
        ("exact tool", json!({"params":{"tool":"Agent"}}), true),
        (
            "exact title",
            json!({"params":{"tool":"shell", "title":"Task"}}),
            true,
        ),
        (
            "exact argument tool name",
            json!({"params":{"arguments":{"_toolName":"Agent"}}}),
            true,
        ),
        (
            "case drift is not exact",
            json!({
                "params": {
                    "tool":"agent",
                    "title":"task",
                    "arguments":{"_toolName":"TASK"}
                }
            }),
            false,
        ),
        (
            "launch words embedded in other names are not exact",
            json!({
                "params": {
                    "tool":"ProviderAgent",
                    "title":"Task helper",
                    "arguments":{"_toolName":"mcp_Task_0"}
                }
            }),
            false,
        ),
        (
            "non-string fields are ignored",
            json!({"params":{"tool":7, "title":null, "arguments":{"_toolName":false}}}),
            false,
        ),
    ];

    for (name, event, expected) in cases {
        assert_eq!(is_exact_native_launch_attempt(&event), expected, "{name}");
    }
}

#[tokio::test]
async fn unbridged_launch_suppression_handles_missing_call_id_and_unsuppressed_cards() {
    let session = test_session(Some("scope-task-suppression"));
    let cases = [
        (
            "confirmed task without call id",
            json!({
                "params": {"tool": "Task", "title": "Task", "arguments": {}}
            }),
            true,
            None,
            true,
        ),
        (
            "ordinary provider card is not suppressed",
            json!({
                "params": {"tool": "shell", "title": "shell", "arguments": {}}
            }),
            false,
            Some("ordinary-call"),
            false,
        ),
    ];

    for (name, event, mcp_hint, call_id, expected) in cases {
        let mut builder = SegmentBuilder::new(1);
        assert_eq!(
            builder
                .should_suppress_unbridged_launch(&session, &event, mcp_hint, call_id, None)
                .await
                .expect(name),
            expected,
            "{name}"
        );
        assert!(
            builder.incomplete_launch_call_ids.is_empty(),
            "{name}: no call id or no suppression must not track an incomplete launch"
        );
    }
}

#[tokio::test]
async fn nested_subagent_unbridged_agent_launch_emits_english_notice() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session_with_tools(Some("nested-worker"), [("Bash", "Bash")], false);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "nested-agent",
                "Agent",
                json!({"prompt":"launch another worker"}),
            ),
            Some(&sender),
        )
        .await
        .expect("nested Agent attempt");
    let live = drain_stream(&mut receiver);
    assert!(
        live.contains(crate::anthropic::subagent_reuse::NESTED_SUBAGENT_LAUNCH_NOTICE),
        "{live}"
    );
    assert!(!live.contains(r#""type":"tool_use""#), "{live}");
}

#[tokio::test]
async fn nested_subagent_unbridged_task_launch_emits_english_notice() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session_with_tools(Some("nested-task"), [("Bash", "Bash")], false);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "nested-task",
                "Task",
                json!({"prompt":"launch another worker"}),
            ),
            Some(&sender),
        )
        .await
        .expect("nested Task attempt");
    let live = drain_stream(&mut receiver);
    assert!(
        live.contains(crate::anthropic::subagent_reuse::NESTED_SUBAGENT_LAUNCH_NOTICE),
        "{live}"
    );
    assert!(!live.contains(r#""type":"tool_use""#), "{live}");
}

#[tokio::test]
async fn nested_subagent_send_message_is_not_rejected_as_a_launch() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session =
        test_session_with_tools(Some("nested-send"), [("SendMessage", "SendMessage")], false);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &native_launch_event(
                "item/providerTool/call",
                "nested-send",
                "SendMessage",
                json!({"to":"a0123456789abcdef","message":"continue the review"}),
            ),
            Some(&sender),
        )
        .await
        .expect("nested SendMessage");
    let live = drain_stream(&mut receiver);
    assert!(
        !live.contains(crate::anthropic::subagent_reuse::NESTED_SUBAGENT_LAUNCH_NOTICE),
        "{live}"
    );
}

mod temporary_env {
    use std::path::Path;

    pub(super) struct Guard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    pub(super) fn set_var(key: &'static str, value: &Path) -> Guard {
        let lock = super::super::launch_queue_env_lock();
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Guard {
            key,
            previous,
            _lock: lock,
        }
    }
}
