#![allow(clippy::excessive_nesting)]

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    convert::Infallible,
    ops::ControlFlow,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore, mpsc};

use super::{
    SegmentBuilder, StreamWaitInput, builder::parse_tool_call, context_window, error_flow,
    message_start, sanitize, send_stream_completion, send_stream_error, send_stream_frame,
    thinking::ThinkingState, tool_use_frames, turn_flow,
};
use crate::{
    agent_backend::AgentBackend,
    anthropic::{ActiveTurn, Bridge, ContextRetry, MessagesRequest, Session},
    app_server::{AppServer, events::ThreadEventDispatcher},
    grok_acp::GrokAcp,
};

#[tokio::test]
async fn ignores_missing_and_empty_text_deltas() {
    let mut builder = SegmentBuilder::new(7);
    builder
        .text_delta(&json!({"params":{}}), None)
        .await
        .expect("missing delta");
    builder
        .text_delta(&json!({"params":{"delta":""}}), None)
        .await
        .expect("empty delta");
    let segment = builder.finish(None).await.expect("empty segment");
    assert!(segment.blocks.is_empty());
    assert_eq!(segment.usage.input_tokens, 7);
    assert_eq!(segment.usage.output_tokens, 0);
}

#[test]
fn recognizes_context_markers_in_every_provider_error_field() {
    let events = [
        json!({"params":{"error":{"message":"context window"}}}),
        json!({"params":{"message":"context window"}}),
        json!({"params":{"error":{"codexErrorInfo":"context window"}}}),
        json!({"params":{"error":{"code":"context window"}}}),
        json!({"params":{"error":{"type":"context window"}}}),
        json!({"params":{"error":{"name":"context window"}}}),
        json!({"params":{"error":{"additionalDetails":"context window"}}}),
    ];
    for event in events {
        assert!(context_window::is_context_window_event(&event));
    }
    assert!(context_window::is_context_window_event(
        &json!({"event":"context window"})
    ));
    assert!(!context_window::is_context_window_event(
        &json!({"params":{}})
    ));
}

#[test]
fn sanitizes_text_thinking_and_provider_status_variants() {
    let mut blocks = vec![
        json!({"type":"text","text":"hello\u{200b}"}),
        json!({"type":"text"}),
        json!({"type":"thinking"}),
        json!({"type":"thinking","thinking":"useful\u{200b}"}),
        json!({"type":"thinking","thinking":"▶ running"}),
        json!({"type":"unknown"}),
    ];
    sanitize::sanitize_committed_blocks(&mut blocks);
    assert_eq!(blocks[0]["text"], "hello");
    assert_eq!(blocks[1]["type"], "text");
    assert_eq!(blocks[2]["type"], "thinking");
    assert_eq!(blocks[3]["thinking"], "useful");
    assert_eq!(blocks[4]["type"], "unknown");
    assert_eq!(blocks.len(), 5);

    for status in [
        "✓ done",
        "✗ failed",
        "Plan next",
        "Plan:",
        "● step",
        "◎ step",
        "○ step",
        "SubAgent started: worker",
        "Retrying provider request",
        "Session mode: worker",
        "Session: worker",
        "🔎 WebSearch: Example Robotics",
    ] {
        let mut status_block = vec![json!({"type":"thinking","thinking":status})];
        sanitize::sanitize_committed_blocks(&mut status_block);
        assert!(
            status_block.is_empty(),
            "status should be removed: {status}"
        );
    }
}

#[tokio::test]
async fn thinking_state_handles_reuse_keepalive_and_unit_transitions() {
    let mut state = ThinkingState::default();
    let mut blocks = Vec::new();
    state
        .delta(
            &json!({"params":{"itemId":"reasoning","summaryIndex":0,"delta":"one"}}),
            &mut blocks,
            None,
        )
        .await
        .expect("first thought");
    state
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("model reasoning heartbeat");
    state
        .delta(
            &json!({"params":{"itemId":"reasoning","summaryIndex":0,"delta":" two"}}),
            &mut blocks,
            None,
        )
        .await
        .expect("continued thought");
    state.close(&mut blocks, None).await.expect("close thought");
    state
        .close(&mut blocks, None)
        .await
        .expect("close empty state");

    state
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("first keepalive");
    state
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("heartbeat keepalive");
    let mut visible = vec![json!({"type":"text","text":"answer"})];
    state
        .activity_keepalive(&mut visible, None)
        .await
        .expect("visible output keepalive");
    state
        .delta(
            &json!({"params":{"itemId":"model:status","summaryIndex":0,"delta":"ignored"}}),
            &mut blocks,
            None,
        )
        .await
        .expect("status-like thought");
}

#[tokio::test]
async fn joins_text_deltas_and_estimates_usage() {
    let mut builder = SegmentBuilder::new(2);
    assert!(!builder.has_external_tool_calls());
    for delta in ["hello ", "world"] {
        builder
            .text_delta(&json!({"params":{"delta":delta}}), None)
            .await
            .expect("text delta");
    }
    builder.update_usage(&json!({
        "params":{"tokenUsage":{"last":{"inputTokens":9}}}
    }));
    let segment = builder.finish(None).await.expect("text segment");
    assert_eq!(segment.blocks[0]["text"], "hello world");
    assert_eq!(segment.stop_reason, "end_turn");
    assert_eq!(segment.usage.input_tokens, 9);
    assert!(segment.usage.output_tokens > 0);
}

#[tokio::test]
async fn defaults_missing_reasoning_usage_to_zero() {
    let mut builder = SegmentBuilder::new(2);
    builder.update_usage(&json!({
        "params":{"tokenUsage":{"last":{"outputTokens":5}}}
    }));
    let segment = builder.finish(None).await.expect("usage segment");
    assert_eq!(segment.usage.output_tokens, 5);
}

#[tokio::test]
async fn streams_summarized_thinking_as_separate_units_before_text() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(2);
    for (summary_index, delta) in [(0, "Plan"), (1, "Act")] {
        assert!(
            builder
                .model_output_event(
                    &json!({
                        "method":"item/reasoning/summaryTextDelta",
                        "params":{"itemId":"reasoning-1","summaryIndex":summary_index,"delta":delta}
                    }),
                    Some(&sender),
                )
                .await
                .expect("reasoning delta")
        );
    }
    assert!(
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/textDelta",
                    "params":{"itemId":"reasoning-1","contentIndex":0,"delta":"raw secret"}
                }),
                Some(&sender),
            )
            .await
            .expect("raw reasoning is ignored")
    );
    builder
        .text_delta(&json!({"params":{"delta":"Answer"}}), Some(&sender))
        .await
        .expect("text delta");
    builder.update_usage(&json!({
        "params":{"tokenUsage":{"last":{
            "inputTokens":9,"outputTokens":5,"reasoningOutputTokens":7
        }}}
    }));
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    // Each summaryIndex is its own thinking block (Claude-like units).
    assert_eq!(segment.blocks[0]["type"], "thinking");
    assert_eq!(segment.blocks[0]["thinking"], "Plan");
    assert_eq!(segment.blocks[1]["type"], "thinking");
    assert_eq!(segment.blocks[1]["thinking"], "Act");
    assert_ne!(
        segment.blocks[0]["signature"],
        segment.blocks[1]["signature"]
    );
    assert_eq!(segment.blocks[2], json!({"type":"text","text":"Answer"}));
    assert_eq!(segment.usage.input_tokens, 9);
    assert_eq!(segment.usage.output_tokens, 12);

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        frames.push(serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame"));
    }
    // start Plan, delta, sig, stop, start Act, delta, sig, stop, start text, delta, stop
    assert_eq!(frames.len(), 11);
    assert_eq!(frames[0]["content_block"]["type"], "thinking");
    assert_eq!(
        frames[1]["delta"],
        json!({"type":"thinking_delta","thinking":"Plan"})
    );
    assert_eq!(frames[2]["delta"]["type"], "signature_delta");
    assert_eq!(frames[3], json!({"type":"content_block_stop","index":0}));
    assert_eq!(frames[4]["content_block"]["type"], "thinking");
    assert_eq!(
        frames[5]["delta"],
        json!({"type":"thinking_delta","thinking":"Act"})
    );
    assert_eq!(frames[6]["delta"]["type"], "signature_delta");
    assert_eq!(frames[7], json!({"type":"content_block_stop","index":1}));
    assert_eq!(frames[8]["content_block"]["type"], "text");
    assert_eq!(
        frames[9]["delta"],
        json!({"type":"text_delta","text":"Answer"})
    );
    assert_eq!(frames[10], json!({"type":"content_block_stop","index":2}));
}

#[tokio::test]
async fn activity_keepalive_emits_visible_status_then_zero_width_heartbeat() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("open keepalive thinking");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("second heartbeat");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    // Pure keepalive thinking is stripped from the committed segment.
    assert!(segment.blocks.is_empty());

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        frames.push(serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame"));
    }
    assert_eq!(frames[0]["content_block"]["type"], "thinking");
    assert_eq!(
        frames[1]["delta"],
        json!({
            "type":"thinking_delta",
            "thinking":"Claudex is still working; waiting for provider output\u{2026}"
        })
    );
    assert_eq!(
        frames[2]["delta"],
        json!({"type":"thinking_delta","thinking":"\u{200b}"})
    );
}

#[tokio::test]
async fn activity_keepalive_uses_open_text_when_visible_output_started() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .text_delta(&json!({"params":{"delta":"hi"}}), Some(&sender))
        .await
        .expect("text");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("text heartbeat");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    // Stream-only heartbeat: final answer text stays clean.
    assert_eq!(segment.blocks[0], json!({"type":"text","text":"hi"}));
    let saw_zwsp = stream_contains_zwsp(&mut receiver).await;
    assert!(saw_zwsp, "expected a zero-width text_delta keepalive frame");
}

#[tokio::test]
async fn refreshes_activity_deadlines_and_detects_closed_streams() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    let mut deadline = Box::pin(tokio::time::sleep(std::time::Duration::from_secs(1)));
    super::refresh_activity_keepalive(
        &mut builder,
        &sender,
        deadline.as_mut(),
        Duration::from_secs(1),
    )
    .await
    .expect("activity keepalive");
    assert!(!deadline.is_elapsed());

    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    assert!(
        !bridge
            .finish_if_stream_closed(&sender, &session, &events, true)
            .await
    );
    drop(receiver);
    assert!(
        bridge
            .finish_if_stream_closed(&sender, &session, &events, true)
            .await
    );
    super::disconnect::warn_disconnect_failure(
        &anyhow!("test drain failure"),
        "thread",
        "tested disconnect warning",
    );
    super::disconnect::warn_cancel_failure(&anyhow!("test cancel failure"), "thread");
}

#[tokio::test]
async fn hidden_provider_events_do_not_postpone_visible_activity() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let wait = bridge.wait_for_stream_segment_with_interval(StreamWaitInput {
        session: &session,
        events: Arc::new(events),
        current_messages: &[],
        system: &json!(null),
        sender: &sender,
        builder: SegmentBuilder::new(1),
        activity_interval: Duration::from_millis(10),
    });
    let dispatch = dispatch_hidden_events(&dispatcher);
    let (result, ()) = tokio::join!(wait, dispatch);
    result.expect("stream segment");
    drop(sender);

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("waiting for provider output"));
}

async fn stream_contains_zwsp(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> bool {
    let mut saw_zwsp = false;
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        saw_zwsp |= frame.contains("\\u200b") || frame.contains('\u{200b}');
    }
    saw_zwsp
}

async fn dispatch_hidden_events(dispatcher: &crate::app_server::events::ThreadEventDispatcher) {
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(4)).await;
        dispatcher.dispatch(json!({
            "method":"thread/tokenUsage/updated",
            "params":{
                "threadId":"thread",
                "tokenUsage":{"last":{"inputTokens":1}}
            }
        }));
    }
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
}

#[tokio::test]
async fn reports_a_closed_provider_event_stream() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
        })
        .await;
    let Err(error) = result else {
        panic!("closed provider event stream must fail");
    };

    assert!(error.to_string().contains("event stream closed"));
}

#[tokio::test]
async fn classifies_a_dead_provider_stream_closure_for_one_retry() {
    let (bridge, session, dispatcher) = grok_disconnect_fixture();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
        })
        .await;
    assert!(matches!(
        result,
        Ok(super::StreamTurn::ProviderFailure { .. })
    ));
}

#[tokio::test]
async fn retries_context_window_errors_only_before_committed_output() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
        })
        .await
        .expect("context error should request retry");
    assert!(matches!(result, super::StreamTurn::ContextWindow { .. }));

    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"visible"}
    }));
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
        })
        .await;
    let Err(error) = result else {
        panic!("context error after visible output must be fatal");
    };
    assert!(error.to_string().contains("context window"));
}

#[tokio::test]
async fn reports_slow_stream_preparation_before_the_provider_is_ready() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let prepare = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok::<_, anyhow::Error>("ready")
    };
    let (result, mut builder) = super::prepare_with_activity(
        prepare,
        3,
        &sender,
        Some("SubAgent starting: worker-model (effort=high)"),
        Duration::from_millis(5),
        Duration::from_millis(50),
    )
    .await;
    assert_eq!(result.expect("prepare result"), Some("ready"));
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    assert_eq!(segment.usage.input_tokens, 3);
    // Keepalive thinking is live-only; committed segment stays clean.
    assert!(segment.blocks.is_empty());
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        frames.push(String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE"));
    }
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("SubAgent starting: worker-model"))
    );
    assert!(frames.iter().any(|frame| frame.contains("thinking_delta")));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("content_block_stop"))
    );
}

#[tokio::test]
async fn finishes_fast_or_disconnected_stream_preparation_without_activity_status() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let (result, builder) = super::prepare_with_activity(
        std::future::ready(Ok::<_, anyhow::Error>("ready")),
        1,
        &sender,
        None,
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(result.expect("fast prepare"), Some("ready"));
    assert!(builder.blocks.is_empty());

    drop(receiver);
    let (result, builder) = super::prepare_with_activity(
        std::future::pending::<anyhow::Result<()>>(),
        1,
        &sender,
        None,
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .await;
    assert!(result.expect("disconnected prepare").is_none());
    assert!(builder.blocks.is_empty());

    let (error_sender, _error_receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let (result, builder) = super::prepare_with_activity(
        std::future::ready(Err::<(), _>(anyhow!("provider setup failed"))),
        1,
        &error_sender,
        None,
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .await;
    assert!(
        result
            .expect_err("failed prepare")
            .to_string()
            .contains("failed")
    );
    assert!(builder.blocks.is_empty());
}

#[tokio::test]
async fn ignores_malformed_empty_raw_and_late_reasoning() {
    let mut builder = SegmentBuilder::new(1);
    for event in [
        json!({"method":"item/reasoning/summaryTextDelta","params":{}}),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning"}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":7,"summaryIndex":0,"delta":"wrong item type"}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":"zero","delta":"wrong index type"}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0,"delta":7}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0,"delta":""}
        }),
        json!({
            "method":"item/reasoning/textDelta",
            "params":{"itemId":"reasoning","contentIndex":0,"delta":"raw"}
        }),
    ] {
        assert!(
            builder
                .model_output_event(&event, None)
                .await
                .expect("ignored reasoning event")
        );
    }
    assert!(builder.blocks.is_empty());

    builder
        .text_delta(&json!({"params":{"delta":"visible"}}), None)
        .await
        .expect("visible text");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"late","summaryIndex":0,"delta":"late"}
            }),
            None,
        )
        .await
        .expect("late reasoning");
    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.blocks, [json!({"type":"text","text":"visible"})]);
}

#[tokio::test]
async fn streams_native_web_search_status_without_committing_progress_text() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    for event in [
        json!({
            "method":"item/started",
            "params":{"item":{"type":"message","query":"ignored"}}
        }),
        json!({
            "method":"item/started",
            "params":{"item":{"type":"webSearch","query":"Example Robotics"}}
        }),
        json!({
            "method":"item/started",
            "params":{"item":{"type":"webSearch","query":""}}
        }),
    ] {
        assert!(
            builder
                .handle_event(&bridge, &session, &[], &json!({}), &event, Some(&sender),)
                .await
                .expect("native search event")
                .is_continue()
        );
    }
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    assert_eq!(segment.blocks.len(), 1);
    let text = segment.blocks[0]["text"].as_str().expect("progress text");
    assert!(text.trim().is_empty());
    assert_eq!(segment.usage.web_search_requests, 0);

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        frames.push(frame);
    }
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Example Robotics"))
    );
    assert!(frames.iter().any(|frame| frame.contains("WebSearch")));
    app.shutdown().await;
}

#[tokio::test]
async fn closes_each_reasoning_item_with_its_own_signature() {
    let mut builder = SegmentBuilder::new(1);
    for (item_id, delta) in [("first", "one"), ("second", "two")] {
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/summaryTextDelta",
                    "params":{"itemId":item_id,"summaryIndex":0,"delta":delta}
                }),
                None,
            )
            .await
            .expect("reasoning item");
    }
    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.blocks.len(), 2);
    assert_eq!(segment.blocks[0]["thinking"], "one");
    assert_eq!(segment.blocks[1]["thinking"], "two");
    assert_ne!(
        segment.blocks[0]["signature"],
        segment.blocks[1]["signature"]
    );
    assert!(segment.usage.output_tokens > 0);
}

#[test]
fn parses_tool_calls_and_reports_each_missing_field() {
    let valid = json!({
        "id":8,
        "params":{"callId":"call","tool":"lookup"}
    });
    let call = parse_tool_call(&valid).expect("valid tool call");
    assert_eq!(call.call_id, "call");
    assert_eq!(call.name, "lookup");
    assert_eq!(call.arguments, Value::Null);
    assert_eq!(call.request_id, json!(8));

    for (event, message) in [
        (json!({}), "params missing"),
        (json!({"params":{"tool":"x"},"id":1}), "callId missing"),
        (json!({"params":{"callId":"x"},"id":1}), "name missing"),
        (
            json!({"params":{"callId":"x","tool":"y"}}),
            "request id missing",
        ),
    ] {
        let error = match parse_tool_call(&event) {
            Ok(_) => panic!("invalid tool call was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(message));
    }
}

#[tokio::test]
async fn rejects_a_malformed_tool_event_before_dispatch() {
    let root = tempfile::tempdir().expect("tool event fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("mock-app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
    )
    .expect("mock app-server");
    let mut permissions = std::fs::metadata(&program)
        .expect("mock metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("mock permissions");
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let bridge = Bridge::new(app, "main".to_owned());
    let slots = Arc::new(Semaphore::new(1));
    let session = Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    };
    let error = SegmentBuilder::new(1)
        .handle_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &json!({"method":"item/tool/call","params":{}}),
            None,
        )
        .await
        .expect_err("malformed tool event");
    assert!(error.to_string().contains("callId missing"));
}

#[tokio::test]
async fn bridges_acp_agent_provider_tools_to_tool_use_but_keeps_native_tools_as_wip() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    // disconnect_fixture already maps cc_Agent_0 → Agent; add Bash for native WIP.
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Bash_0".to_owned(), "Bash".to_owned());
    // Also accept the plain "Agent" provider label used by some ACP agents.
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("Agent".to_owned(), "Agent".to_owned());
    let messages = [json!({"role":"user","content":"delegate work"})];
    let routing = json!(
        r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model","effort":"high"}]}"#
    );
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &messages,
            &routing,
            &json!({
                "method":"item/providerTool/call",
                "params":{
                    "callId":"acp-agent-1",
                    "tool":"Agent",
                    "status":"pending",
                    "arguments":{"prompt":"implement feature","subagent_type":"general-purpose"}
                }
            }),
            None,
        )
        .await
        .expect("bridge Agent to tool_use");
    assert!(builder.has_external_tool_calls());
    assert_eq!(builder.blocks[0]["type"], "tool_use");
    assert_eq!(builder.blocks[0]["name"], "Agent");
    let prompt = builder.blocks[0]["input"]["prompt"]
        .as_str()
        .expect("bridged Agent prompt");
    assert!(prompt.starts_with("implement feature"));
    assert_eq!(
        builder.blocks[0]["input"]["subagent_type"],
        "general-purpose"
    );

    let mut native = SegmentBuilder::new(1);
    let _ = native
        .handle_event(
            &bridge,
            &session,
            &messages,
            &routing,
            &json!({
                "method":"item/providerTool/call",
                "params":{
                    "callId":"acp-bash-1",
                    "tool":"Bash",
                    "status":"pending",
                    "arguments":{"command":"ls"}
                }
            }),
            None,
        )
        .await
        .expect("native Bash stays WIP");
    assert!(!native.has_external_tool_calls());
    assert!(
        native
            .blocks
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) != Some("tool_use"))
    );
    let progress = native
        .open_text_block
        .as_ref()
        .map(|(_, text)| text.as_str())
        .expect("native progress text");
    assert!(progress.contains("▶ Bash"));
    assert!(progress.contains("ls"));
}

#[tokio::test]
async fn expands_valid_parallel_agent_batches_and_rejects_short_batches() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let routing = json!(
        r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model"}]}"#
    );
    let messages = [json!({"role":"user","content":"delegate"})];
    let mut builder = SegmentBuilder::new(1);
    let event = agent_batch_event(
        "batch-call",
        [
            worker_task("first", None),
            worker_task("second", Some(true)),
            worker_task("third", Some(true)),
        ],
    );
    let _ = builder
        .handle_event(&bridge, &session, &messages, &routing, &event, None)
        .await
        .expect("parallel batch");
    assert!(builder.has_external_tool_calls());
    assert_eq!(builder.blocks.len(), 3);
    assert_background_batch(&builder, 0, 3);

    let mixed = agent_batch_event(
        "mixed-call",
        [
            worker_task("background", None),
            worker_task("foreground", Some(false)),
            worker_task("third", Some(true)),
        ],
    );
    let _ = builder
        .handle_event(&bridge, &session, &messages, &routing, &mixed, None)
        .await
        .expect("mixed batch modes are normalized to background");
    assert_eq!(builder.blocks.len(), 6);
    assert_background_batch(&builder, 3, 3);

    let short = agent_batch_event("short-call", [worker_task("only", None)]);
    let error = builder
        .handle_event(&bridge, &session, &messages, &routing, &short, None)
        .await
        .expect_err("short batch");
    assert!(error.to_string().contains("between 3 and 40"));
}

#[tokio::test]
async fn forwards_generic_tools_and_blocks_disabled_subagent_models() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let mut generic = SegmentBuilder::new(1);
    let _ = generic
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":1,
                "method":"item/tool/call",
                "params":{
                    "callId":"read",
                    "tool":"cc_Read_0",
                    "arguments":{
                        "path":"README.md",
                        "claudex_model":"gpt-5.6-luna",
                        "claudex_implicit_model":"gpt-5.6-luna",
                        "claudex_effort":"max"
                    }
                }
            }),
            None,
        )
        .await
        .expect("generic external tool");
    assert!(generic.has_external_tool_calls());
    assert_eq!(generic.blocks[0]["name"], "Read");
    assert_eq!(generic.blocks[0]["input"], json!({"path":"README.md"}));

    let disabled = BTreeSet::from(["blocked-model".to_owned()]);
    let (_root, _app, bridge, session) = disconnect_fixture_with_disabled(disabled).await;
    let mut blocked = SegmentBuilder::new(1);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let _ = blocked
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":2,
                "method":"item/tool/call",
                "params":{
                    "callId":"agent",
                    "tool":"cc_Agent_0",
                    "arguments":{"prompt":"delegate","subagent_type":"worker","claudex_model":"blocked-model"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("disabled subagent is a visible local response");
    assert!(!blocked.has_external_tool_calls());
    assert!(
        blocked.blocks[0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("blocked-model"))
    );
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("blocked-model"));
}

fn worker_task(prompt: &str, run_in_background: Option<bool>) -> Value {
    let mut task = json!({
        "prompt": prompt,
        "subagent_type": "worker",
        "claudex_model": "worker-model"
    });
    if let Some(run_in_background) = run_in_background {
        task["run_in_background"] = json!(run_in_background);
    }
    task
}

fn agent_batch_event(call_id: &str, tasks: impl IntoIterator<Item = Value>) -> Value {
    json!({
        "id": 99,
        "method": "item/tool/call",
        "params": {
            "callId": call_id,
            "tool": "cc_Agent_batch_0",
            "arguments": {"tasks": tasks.into_iter().collect::<Vec<_>>()}
        }
    })
}

fn assert_background_batch(builder: &SegmentBuilder, start: usize, count: usize) {
    for index in start..start + count {
        assert_eq!(
            builder.blocks[index]["input"]["run_in_background"].as_bool(),
            Some(true),
            "batch worker {index} should run in background"
        );
    }
}

#[tokio::test]
async fn treats_a_closed_sender_after_batch_finish_as_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &[json!({"role":"user","content":"delegate"})],
            &json!(
                r#"{"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model"}]}"#
            ),
            &json!({
                "id":99,
                "method":"item/tool/call",
                "params":{
                    "callId":"batch-call",
                    "tool":"cc_Agent_batch_0",
                    "arguments":{"tasks":[
                        {"prompt":"first","subagent_type":"worker","claudex_model":"worker-model"},
                        {"prompt":"second","subagent_type":"worker","claudex_model":"worker-model"},
                        {"prompt":"third","subagent_type":"worker","claudex_model":"worker-model"}
                    ]}
                }
            }),
            None,
        )
        .await
        .expect("batch tool call");
    let result = bridge
        .external_batch_segment(&session, events, &mut builder, &sender)
        .await
        .expect("closed batch sender");
    assert!(matches!(result, super::StreamTurn::Disconnected));
}

#[tokio::test]
async fn commits_status_deltas_as_progress_text_after_answer_starts() {
    let mut builder = SegmentBuilder::new(1);
    builder.blocks.push(json!({"type":"text","text":"answer"}));
    assert!(builder.has_committed_output());
    builder
        .stream_progress_text("", None)
        .await
        .expect("empty status");
    // Existing closed text block: open a new progress text stream.
    builder
        .stream_progress_text("\n▶ provider\n", None)
        .await
        .expect("visible block status");
    assert!(
        builder
            .open_text_block
            .as_ref()
            .expect("progress text")
            .1
            .contains("▶ provider")
    );

    builder.blocks.clear();
    builder.open_text_block = Some((0, "answer".to_owned()));
    builder.blocks.push(json!({"type":"text","text":""}));
    builder
        .stream_progress_text("\n▶ provider\n", None)
        .await
        .expect("open text status");
    assert_eq!(
        builder
            .open_text_block
            .as_ref()
            .map(|(_, text)| text.as_str()),
        Some("answer\n▶ provider\n")
    );
}

#[tokio::test]
async fn unsupported_disconnect_with_a_visible_tool_aborts_without_a_drain() {
    let (root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    bridge.sessions.lock().await.push(Arc::clone(&session));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(41));
    *session.pending_since.lock().unwrap() = Some(Instant::now());
    app.dispatch_test_event(json!({
        "id":41,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"duplicate","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "id":42,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"new","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "method":"thread/tokenUsage/updated",
        "params":{"threadId":"thread","tokenUsage":{"last":{"inputTokens":1}}}
    }));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","willRetry":true}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    assert!(session.pending_since.lock().unwrap().is_none());
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(!app.is_alive());
    assert_eq!(Arc::strong_count(&events), 1, "no hidden drain owns events");
    tokio::time::timeout(Duration::from_secs(1), async {
        while events.recv().await.is_some() {}
    })
    .await
    .expect("provider abort must close the event channel after queued events");
    assert_eq!(bridge.used_session_slots(), 1);
    drop(session);
    assert_eq!(bridge.used_session_slots(), 0);
    drop(root);
}

#[tokio::test]
async fn async_handoff_with_a_visible_tool_drains_without_closing_shared_provider() {
    let (root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    bridge.sessions.lock().await.push(Arc::clone(&session));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(41));
    *session.pending_since.lock().unwrap() = Some(Instant::now());
    app.dispatch_test_event(json!({
        "id":41,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"duplicate","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    assert!(matches!(
        bridge
            .disconnect_stream_for_async_handoff(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(
        app.is_alive(),
        "async handoff must not stop a shared provider"
    );
    wait_for_disconnected_drain(&events).await;
    drop(session);
    drop(root);
}

#[tokio::test]
async fn disconnected_drain_handles_incremental_events_and_provider_errors() {
    let (root, app, bridge, _session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "id":51,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"first","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "id":51,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"duplicate","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"inProgress"}}
    }));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","willRetry":true}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    super::disconnect::drain_disconnected_turn(&bridge.app, "main", events, HashSet::new())
        .await
        .expect("completed turn drains successfully");
    assert_disconnected_tool_rejections(&root, &[51]).await;
}

#[tokio::test]
async fn disconnected_drain_returns_non_retryable_provider_errors() {
    let (_root, app, bridge, _session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","message":"fatal"}
    }));

    let error =
        super::disconnect::drain_disconnected_turn(&bridge.app, "main", events, HashSet::new())
            .await
            .expect_err("fatal provider event must stop the drain");
    assert!(error.to_string().contains("fatal"));
}

#[tokio::test]
async fn unsupported_disconnect_drains_without_closing_the_provider() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","message":"fatal"}
    }));
    bridge.finish_closed_stream(&session, &events, false).await;
    wait_for_disconnected_drain(&events).await;
    assert!(app.is_alive());
}

#[tokio::test]
async fn tolerates_failed_pending_tool_rejection_after_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(61));
    app.shutdown().await;

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    wait_for_disconnected_drain(&events).await;
}

#[tokio::test]
async fn cancellation_failure_detaches_and_warns_for_pending_tools() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(61));
    app.shutdown().await;

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    wait_for_disconnected_drain(&events).await;
}

#[tokio::test]
async fn grok_cancellation_failure_rejects_pending_tools_and_detaches() {
    let (bridge, session, dispatcher) = grok_disconnect_fixture();
    let events = Arc::new(dispatcher.subscribe("thread"));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(61));
    *session.pending_since.lock().unwrap() = Some(Instant::now());
    bridge.sessions.lock().await.push(Arc::clone(&session));

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(session.pending_tools.lock().await.is_empty());
    dispatcher.close();
    wait_for_disconnected_drain(&events).await;
}

#[tokio::test]
async fn disconnected_drain_reports_closed_and_malformed_event_streams() {
    let (bridge, _session, dispatcher) = grok_disconnect_fixture();
    let events = Arc::new(dispatcher.subscribe("thread"));
    dispatcher.close();
    let error = super::disconnect::drain_disconnected_turn(
        &bridge.app,
        "main",
        Arc::clone(&events),
        HashSet::new(),
    )
    .await
    .expect_err("closed event stream should be reported");
    assert!(error.to_string().contains("event stream closed"));

    let dispatcher = ThreadEventDispatcher::default();
    let events = Arc::new(dispatcher.subscribe("thread"));
    dispatcher.dispatch(json!({
        "method":"item/tool/call",
        "params":{"threadId":"thread"}
    }));
    let error =
        super::disconnect::drain_disconnected_turn(&bridge.app, "main", events, HashSet::new())
            .await
            .expect_err("malformed tool event should be reported");
    assert!(error.to_string().contains("tool"));
}

#[tokio::test]
async fn drive_stream_reports_unretryable_context_window_errors() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let error = receiver
        .recv()
        .await
        .expect("context window error frame")
        .expect("infallible frame");
    assert!(String::from_utf8_lossy(&error).contains("context window"));
}

#[tokio::test]
async fn drive_stream_retries_context_window_then_completes() {
    let (_root, _app, bridge, session) = retryable_drive_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let request = drive_request();

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let retried_session = {
        let sessions = bridge.sessions.lock().await;
        assert_eq!(sessions.len(), 1, "retry should retain its fresh session");
        Arc::clone(&sessions[0])
    };
    let transcript = retried_session.transcript.lock().await.clone();
    assert_eq!(transcript[0], json!({"role":"user","content":"retry me"}));
    assert_eq!(transcript[1], json!({"role":"assistant","content":[]}));

    let mut output = String::new();
    while let Ok(frame) = receiver.try_recv() {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("event: message_delta"));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn drive_stream_keeps_content_indices_monotonic_across_context_retry() {
    let (_root, _app, bridge, session) = retryable_drive_fixture_with_output().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive thinking block");

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            builder,
            None,
        )
        .await;

    let mut open_index = None;
    let mut next_index = 0;
    let mut started_types = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        let payload = serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame");
        match payload.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                let index = payload["index"].as_u64().expect("start index") as usize;
                assert_eq!(index, next_index, "content indices must not be reused");
                assert!(open_index.replace(index).is_none(), "nested content block");
                next_index += 1;
                started_types.push(payload["content_block"]["type"].clone());
            }
            Some("content_block_delta") => {
                let index = payload["index"].as_u64().expect("delta index") as usize;
                assert_eq!(open_index, Some(index), "delta must target the open block");
            }
            Some("content_block_stop") => {
                let index = payload["index"].as_u64().expect("stop index") as usize;
                assert_eq!(open_index.take(), Some(index), "stop must close its start");
            }
            _ => {}
        }
    }

    assert_eq!(started_types, vec![json!("thinking"), json!("text")]);
    assert_eq!(next_index, 2);
    assert!(open_index.is_none());
}

#[tokio::test]
async fn drive_stream_retries_context_window_with_explicit_effort() {
    let (root, _app, bridge, session) = retryable_drive_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let request = drive_request();
    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: Some("high".to_owned()),
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let trace = request_trace(&root.path().join("requests.log"), 2).await;
    let turn_starts = trace
        .into_iter()
        .filter(|request| request.get("method").and_then(Value::as_str) == Some("turn/start"))
        .collect::<Vec<_>>();
    assert!(
        !turn_starts.is_empty(),
        "retry should trigger a logged turn/start request"
    );
    for request in &turn_starts {
        assert_eq!(request["params"]["effort"], "high");
    }

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("event: message_delta"));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn drive_stream_reports_context_retry_setup_errors() {
    let (_root, _app, bridge, session) = retry_failure_drive_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let error = receiver
        .recv()
        .await
        .expect("context retry setup error frame")
        .expect("infallible frame");
    assert!(String::from_utf8_lossy(&error).contains("retry setup failed"));
    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn drive_stream_finishes_quietly_after_client_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = app.subscribe_thread("thread");
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;
}

#[tokio::test]
async fn drive_stream_reports_closed_provider_event_streams() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let error = receiver
        .recv()
        .await
        .expect("closed event stream error frame")
        .expect("infallible frame");
    assert!(String::from_utf8_lossy(&error).contains("event stream closed"));
}

#[tokio::test]
async fn drive_stream_stops_before_commit_when_client_closes_after_segment() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"answer"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    let driver = tokio::spawn(Arc::clone(&bridge).drive_stream(
        drive_turn(session.clone(), events, Vec::new(), None).await,
        sender,
        SegmentBuilder::new(1),
        None,
    ));

    tokio::time::timeout(Duration::from_secs(1), async {
        while receiver.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stream should fill before the completion frame");
    receiver.close();
    driver.await.expect("stream driver task");

    assert!(session.transcript.lock().await.is_empty());
}

#[tokio::test]
async fn subagent_stream_without_hard_timeout_stays_attached_beyond_300_seconds() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    tokio::time::pause();
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);

    let driver = tokio::spawn(Arc::clone(&bridge).drive_subagent_stream_with_timeout(
        drive_turn(session, events, Vec::new(), None).await,
        sender,
        SegmentBuilder::new(1),
        super::drive::StreamDriveOptions {
            model_permit: None,
            is_subagent: true,
            run_in_background: true,
            timeout: None,
        },
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(301)).await;
    tokio::task::yield_now().await;

    assert!(
        !driver.is_finished(),
        "unset timeout ended the native Agent stream"
    );
    assert!(bridge.detached_sessions.lock().await.is_empty());

    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"stream completed after 301 seconds"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    driver.await.expect("stream driver task");

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("stream completed after 301 seconds"));
}

#[tokio::test]
async fn subagent_stream_hard_timeout_cancels_and_reports_a_visible_error() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = app.subscribe_thread("thread");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let observed_session = Arc::clone(&session);

    Arc::clone(&bridge)
        .drive_subagent_stream_with_timeout(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            super::drive::StreamDriveOptions {
                model_permit: None,
                is_subagent: true,
                run_in_background: true,
                timeout: Some(Duration::ZERO),
            },
        )
        .await;

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        output.contains("configured hard timeout"),
        "unexpected stream: {output}"
    );
    assert!(!output.contains("dynamic progress"));
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert_eq!(
        bridge
            .subagent_hard_timeout_cancel_attempts
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    app.dispatch_test_event(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"late stream result"}
    }));
    app.dispatch_test_event(json!({
        "method":"item/tool/call",
        "params":{
            "threadId":"thread",
            "item":{"id":"late-stream-tool","name":"Read","arguments":{"path":"ignored"}}
        }
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    tokio::task::yield_now().await;
    assert!(observed_session.transcript.lock().await.is_empty());
    assert!(observed_session.pending_tools.lock().await.is_empty());
}

#[tokio::test]
async fn subagent_stream_timeout_tolerates_a_disconnected_client() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = app.subscribe_thread("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);

    Arc::clone(&bridge)
        .drive_subagent_stream_with_timeout(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            super::drive::StreamDriveOptions {
                model_permit: None,
                is_subagent: true,
                run_in_background: true,
                timeout: Some(Duration::ZERO),
            },
        )
        .await;
    assert!(bridge.detached_sessions.lock().await.is_empty());
}

async fn drive_turn(
    session: Arc<Session>,
    events: crate::app_server::ThreadEvents,
    extras: Vec<Value>,
    retry: Option<ContextRetry>,
) -> ActiveTurn {
    let gate = Arc::clone(&session.gate).lock_owned().await;
    ActiveTurn {
        session,
        events: Arc::new(events),
        response_model: "main".to_owned(),
        extras,
        routing_system: Value::Null,
        input_tokens: 1,
        retry,
        gate,
        detached: false,
    }
}

fn drive_request() -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"user","content":"retry me"})],
        tools: Vec::new(),
        stream: true,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

async fn disconnect_fixture() -> (tempfile::TempDir, Arc<AppServer>, Bridge, Arc<Session>) {
    disconnect_fixture_with_disabled(Default::default()).await
}

fn grok_disconnect_fixture() -> (Bridge, Arc<Session>, Arc<ThreadEventDispatcher>) {
    let backend = Arc::new(AgentBackend::Grok(GrokAcp::stopped_for_test()));
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let slot = Arc::clone(&bridge.session_slots)
        .try_acquire_owned()
        .expect("session slot");
    let dispatcher = Arc::new(ThreadEventDispatcher::default());
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: BTreeSet::new(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slot,
    });
    (bridge, session, dispatcher)
}

async fn disconnect_fixture_with_disabled(
    disabled_subagent_models: BTreeSet<String>,
) -> (tempfile::TempDir, Arc<AppServer>, Bridge, Arc<Session>) {
    let root = tempfile::tempdir().expect("disconnect fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("mock-app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nlog=\"${0%/*}/responses.log\"\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do printf '%s\\n' \"$line\" >> \"$log\"; done\n",
    )
    .expect("mock app-server");
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let progress_program = root.path().join("mock-claude");
    std::fs::write(
        &progress_program,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"dynamic progress from progress subagent\"}'\n",
    )
    .expect("write progress model mock");
    let mut progress_permissions = std::fs::metadata(&progress_program)
        .expect("progress model metadata")
        .permissions();
    progress_permissions.set_mode(0o755);
    std::fs::set_permissions(&progress_program, progress_permissions)
        .expect("make progress model mock executable");
    let settings = root.path().join("settings.json");
    std::fs::write(&settings, r#"{"model":"mock-progress"}"#)
        .expect("write progress model settings");
    let bridge = Bridge::new_with_subscription_program(
        Arc::clone(&app),
        "main".to_owned(),
        progress_program,
    )
    .with_settings_path(settings);
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models,
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::from([
            (
                "cc_Agent_batch_0".to_owned(),
                "__claudex_agent_batch__:Agent".to_owned(),
            ),
            ("cc_Agent_0".to_owned(), "Agent".to_owned()),
            ("cc_Read_0".to_owned(), "Read".to_owned()),
        ]),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: Arc::clone(&bridge.session_slots)
            .try_acquire_owned()
            .expect("session slot"),
    });
    (root, app, bridge, session)
}

async fn assert_disconnected_tool_rejections(root: &tempfile::TempDir, expected: &[u64]) {
    let log = root.path().join("responses.log");
    let actual = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let responses = std::fs::read_to_string(&log).unwrap_or_default();
            let ids = responses
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|response| response.get("id").and_then(Value::as_u64))
                .collect::<Vec<_>>();
            if ids.as_slice() == expected {
                return ids;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnected tool responses should be written promptly");
    assert_eq!(actual, expected);
}

async fn request_trace(path: &std::path::Path, expected: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let trace = std::fs::read_to_string(path)
                .ok()
                .map(|trace| {
                    trace
                        .lines()
                        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                        .collect::<Vec<Value>>()
                })
                .unwrap_or_default();
            if trace.len() >= expected {
                return trace;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("mock trace timeout")
}

async fn wait_for_disconnected_drain(events: &Arc<crate::app_server::ThreadEvents>) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while Arc::strong_count(events) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background disconnected drain should finish promptly");
}

async fn retryable_drive_fixture() -> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>)
{
    retryable_drive_fixture_with_retried_output(false).await
}

async fn retryable_drive_fixture_with_output()
-> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>) {
    retryable_drive_fixture_with_retried_output(true).await
}

async fn retryable_drive_fixture_with_retried_output(
    emit_output: bool,
) -> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>) {
    let root = tempfile::tempdir().expect("retry stream fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let requests_log = root.path().join("requests.log");
    let program = root.path().join("retry-app-server");
    let mut program_script = r#"#!/bin/sh
log="__REQUESTS_LOG__"
read initialize
printf '%s\n' '{"id":1,"result":{}}'
read initialized
read start
printf '%s\n' "$start" >> "$log"
printf '%s\n' '{"id":2,"result":{"thread":{"id":"retried"}}}'
read turn
printf '%s\n' "$turn" >> "$log"
sleep 0.05
__RETRIED_OUTPUT__
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"retried","turn":{"status":"completed"}}}'
while read line; do :; done
"#.to_owned();
    program_script = program_script.replace("__REQUESTS_LOG__", &requests_log.to_string_lossy());
    let retried_output = emit_output.then_some(
        "printf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"retried\",\"delta\":\"retried answer\"}}'",
    );
    program_script = program_script.replace("__RETRIED_OUTPUT__", retried_output.unwrap_or(""));
    std::fs::write(&program, &program_script).expect("mock app-server");
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let bridge = Arc::new(Bridge::new(Arc::clone(&app), "main".to_owned()));
    let slots = Arc::new(Semaphore::new(1));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    (root, app, bridge, session)
}

async fn retry_failure_drive_fixture()
-> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>) {
    let root = tempfile::tempdir().expect("retry failure stream fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("retry-failure-app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread start\nprintf '%s\\n' '{\"id\":2,\"error\":{\"message\":\"retry setup failed\"}}'\nwhile read line; do :; done\n",
    )
    .expect("mock app-server");
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let bridge = Arc::new(Bridge::new(Arc::clone(&app), "main".to_owned()));
    let slots = Arc::new(Semaphore::new(1));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    (root, app, bridge, session)
}

#[test]
fn handles_all_turn_and_error_states() {
    assert_eq!(
        turn_flow(&json!({})).expect("missing status"),
        ControlFlow::Break(())
    );
    assert_eq!(
        turn_flow(&json!({"params":{"turn":{"status":"inProgress"}}})).expect("in progress"),
        ControlFlow::Continue(())
    );
    assert!(
        turn_flow(&json!({"params":{"turn":{"status":"cancelled"}}}))
            .expect_err("failed status")
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(
        error_flow(&json!({"params":{"willRetry":true}})).expect("retry"),
        ControlFlow::Continue(())
    );
    assert!(error_flow(&json!({"params":{"message":"fatal"}})).is_err());
    assert!(error_flow(&json!({"message":"fatal"})).is_err());
}

#[tokio::test]
async fn emits_completion_error_and_optional_frames() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let segment = super::super::Segment {
        blocks: Vec::new(),
        stop_reason: "end_turn",
        usage: super::super::Usage {
            input_tokens: 1,
            output_tokens: 4,
            web_search_requests: 0,
        },
        web_evidence: super::super::WebEvidenceSummary::default(),
    };
    send_stream_completion(&sender, &segment).await;
    send_stream_error(&sender, anyhow!("boom")).await;
    send_stream_frame(None, "ignored", || json!({}))
        .await
        .expect("optional stream");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("event: message_delta"));
    assert!(output.contains("\"output_tokens\":4"));
    assert!(output.contains("event: message_stop"));
    assert!(output.contains("event: error"));
    assert!(output.contains("boom"));
}

#[tokio::test]
async fn completion_frame_exposes_verified_web_evidence_metadata() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    let segment = super::super::Segment {
        blocks: Vec::new(),
        stop_reason: "end_turn",
        usage: super::super::Usage {
            input_tokens: 1,
            output_tokens: 4,
            web_search_requests: 2,
        },
        web_evidence: super::super::WebEvidenceSummary::from_verified_count(2),
    };

    send_stream_completion(&sender, &segment).await;

    let frame = receiver
        .recv()
        .await
        .expect("message delta")
        .expect("frame");
    let frame = String::from_utf8(frame.to_vec()).expect("UTF-8 SSE");
    let payload = frame
        .strip_prefix("event: message_delta\ndata: ")
        .expect("message delta payload");
    let payload: Value = serde_json::from_str(payload.trim()).expect("message delta JSON");
    assert_eq!(
        payload["usage"]["server_tool_use"]["web_search_requests"],
        2
    );
    assert_eq!(
        payload["metadata"]["claudex"]["web_evidence"]["verified_count"],
        2
    );
}

#[test]
fn creates_start_and_tool_frames() {
    let start = message_start("test-model", 12);
    assert!(start.contains("\"model\":\"test-model\""));
    assert!(start.contains("\"input_tokens\":12"));
    let block = json!({
        "id":"toolu_test", "name":"lookup", "input":{"key":"value"}
    });
    let frames = tool_use_frames(3, &block);
    assert_eq!(frames[0].0, "content_block_start");
    assert_eq!(frames[1].1["index"], 3);
    assert!(
        frames[1].1["delta"]["partial_json"]
            .as_str()
            .expect("partial JSON")
            .contains("value")
    );
    assert_eq!(frames[2].0, "content_block_stop");
}

#[test]
fn committed_output_ignores_empty_or_disposable_blocks() {
    let mut builder = SegmentBuilder::new(1);
    assert!(!builder.has_committed_output());

    builder
        .blocks
        .push(json!({"type":"thinking","thinking":"▶ running"}));
    assert!(!builder.has_committed_output());

    builder.open_text_block = Some((0, String::new()));
    assert!(!builder.has_committed_output());
    builder.open_text_block = Some((0, "answer".to_owned()));
    assert!(builder.has_committed_output());
}

#[tokio::test]
async fn prepared_stream_releases_its_concurrency_ticket_after_a_prepare_error() {
    let (_root, _app, bridge, _session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let limits = super::super::model_concurrency::ModelConcurrency::new(Vec::new());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let mut request = drive_request();
    request.messages = vec![json!({
        "role":"user",
        "content":[{"type":"tool_result","tool_use_id":"orphan","content":"result"}]
    })];

    Arc::clone(&bridge)
        .drive_prepared_subagent_stream(super::PreparedStream {
            request,
            input_tokens: 1,
            effort: None,
            concurrency_ticket: limits.ticket("main", Some(1)),
            is_subagent: false,
            run_in_background: false,
            sender,
        })
        .await;

    let frame = receiver
        .recv()
        .await
        .expect("stream preparation error frame")
        .expect("infallible frame");
    assert!(String::from_utf8_lossy(&frame).contains("no active claudex session"));
    assert_eq!(
        serde_json::to_value(limits.snapshot()).unwrap()["main"]["active"],
        0
    );
}

#[tokio::test]
async fn prepared_stream_stops_when_the_client_disconnects_during_setup() {
    let (_root, _app, bridge, _session) = disconnect_fixture().await;
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);

    Arc::new(bridge)
        .drive_prepared_subagent_stream(super::PreparedStream {
            request: drive_request(),
            input_tokens: 1,
            effort: None,
            concurrency_ticket: None,
            is_subagent: false,
            run_in_background: false,
            sender,
        })
        .await;
}

#[tokio::test]
async fn external_batch_segment_returns_an_unsettled_segment_while_stream_is_open() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let mut builder = SegmentBuilder::new(1);
    builder
        .text_delta(&json!({"params":{"delta":"answer"}}), Some(&sender))
        .await
        .expect("text segment");

    let result = bridge
        .external_batch_segment(&session, events, &mut builder, &sender)
        .await
        .expect("open stream segment");
    let super::StreamTurn::Segment {
        segment,
        provider_settled,
    } = result
    else {
        panic!("open sender must keep the batch segment");
    };
    assert!(!provider_settled);
    assert_eq!(segment.blocks[0]["text"], "answer");
}
//x
