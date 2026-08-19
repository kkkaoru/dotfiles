use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use serde_json::{Value, json};

use super::super::{ActiveTurn, PiGateway};
use crate::app_server::{ThreadEventDispatcher, ThreadEvents};

fn event(mut fields: Value) -> Value {
    let object = fields.as_object_mut().expect("event fields object");
    object.insert("version".to_owned(), json!(1));
    object.insert("id".to_owned(), json!("request"));
    fields
}

fn gateway() -> PiGateway {
    PiGateway {
        provider: "provider".to_owned(),
        model_id: "model".to_owned(),
        process: tokio::sync::Mutex::new(None),
        directory: PathBuf::new(),
        socket: PathBuf::new(),
        token: "token".to_owned(),
        events: Arc::new(ThreadEventDispatcher::default()),
        active: Arc::new(Mutex::new(HashMap::<String, ActiveTurn>::new())),
        pending_request_ids: Arc::new(Mutex::new(HashMap::new())),
        alive: AtomicBool::new(true),
    }
}

#[tokio::test]
async fn translates_text_thinking_tool_usage_and_terminal_events() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    feed_text_thinking_tool_turn(&gateway);
    assert_translated_tool_stream(&receiver).await;
}

fn feed_text_thinking_tool_turn(gateway: &PiGateway) {
    let mut state = super::EventTranslateState::default();
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({"type":"text_delta","index":0,"delta":"hello"})),
                &mut state
            )
            .expect("text delta")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({"type":"thinking_delta","index":1,"delta":"reason"})),
                &mut state
            )
            .expect("thinking delta")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_start","index":2,"toolCallId":"call-1","name":"Read"
                })),
                &mut state
            )
            .expect("tool start")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_delta","index":2,"delta":"{\"path\":\"a\"}"
                })),
                &mut state
            )
            .expect("tool delta")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_end","index":2,"toolCallId":"call-1","name":"Read",
                    "arguments":{"path":"a"}
                })),
                &mut state
            )
            .expect("tool end")
    );
    assert!(
        gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"done","reason":"toolUse","message":{"usage":{
                        "input":11,"output":7,"reasoning":4,
                        "cacheRead":5,"cacheWrite":3,"cacheWrite1h":2,
                        "totalTokens":26,
                        "cost":{"input":1.0,"output":2.0,"cacheRead":0.1,
                            "cacheWrite":0.2,"total":3.3}
                    }}
                })),
                &mut state
            )
            .expect("done")
    );
}

async fn assert_translated_tool_stream(receiver: &ThreadEvents) {
    let text = receiver.recv().await.expect("text event");
    let thinking = receiver.recv().await.expect("thinking event");
    let start = receiver.recv().await.expect("tool start");
    let delta = receiver.recv().await.expect("tool delta");
    let tool = receiver.recv().await.expect("tool event");
    let usage = receiver.recv().await.expect("usage event");
    let done = receiver.recv().await.expect("done event");
    assert_eq!(text["method"], "item/agentMessage/delta");
    assert_eq!(thinking["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thinking["params"]["summaryIndex"], 0);
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["tool"], "Read");
    assert_eq!(start["params"]["callId"], "call-1");
    assert_eq!(delta["method"], "item/tool/delta");
    assert_eq!(delta["params"]["delta"], "{\"path\":\"a\"}");
    assert_eq!(tool["params"]["arguments"], json!({"path":"a"}));
    assert_translated_usage_and_done(&usage, &done);
}

fn assert_translated_usage_and_done(usage: &Value, done: &Value) {
    let usage = &usage["params"]["tokenUsage"]["last"];
    assert_eq!(usage["inputTokens"], 11);
    assert_eq!(usage["outputTokens"], 3);
    assert_eq!(usage["reasoningOutputTokens"], 4);
    assert_eq!(
        usage["outputTokens"].as_u64().expect("output")
            + usage["reasoningOutputTokens"].as_u64().expect("reasoning"),
        7
    );
    assert_eq!(usage["cacheReadInputTokens"], 5);
    assert_eq!(usage["cacheCreationInputTokens"], 3);
    assert_eq!(usage["cacheCreation1hInputTokens"], 2);
    assert_eq!(usage["totalTokens"], 26);
    assert_eq!(usage["cost"]["total"], 3.3);
    assert_eq!(done["params"]["turn"]["status"], "completed");
    assert_eq!(done["params"]["turn"]["providerStopReason"], "tool_use");
}

#[tokio::test]
async fn toolcall_start_emits_live_progress_before_arguments_complete() {
    // Pi streamSimple emits toolcall_start as soon as grok begins a tool.
    // SubAgent TUI paints native tool_use cards from item/tool/start, not
    // ACP item/providerTool ▶ thinking. Buffering until toolcall_end drops
    // mid-run progress.
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_start","index":0,"toolCallId":"call-1","name":"Read"
                })),
                &mut state
            )
            .expect("tool start")
    );

    let start = tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv())
        .await
        .expect("toolcall_start must dispatch live tool start, not wait for toolcall_end")
        .expect("start event");
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["callId"], "call-1");
    assert_eq!(start["params"]["tool"], "Read");
    assert_ne!(start["method"], "item/providerTool/call");
}

#[tokio::test]
async fn toolcall_progress_waits_for_name_then_emits_once() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({"type":"toolcall_start","index":0,"toolCallId":"call-late"})),
            &mut state,
        )
        .expect("start without name");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "progress must wait until the tool name arrives"
    );

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"name":"Bash","delta":"{"
            })),
            &mut state,
        )
        .expect("name arrives on first delta");
    let start = receiver.recv().await.expect("late name start");
    let first_delta = receiver.recv().await.expect("first argument delta");
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["callId"], "call-late");
    assert_eq!(start["params"]["tool"], "Bash");
    assert_eq!(first_delta["method"], "item/tool/delta");
    assert_eq!(first_delta["params"]["delta"], "{");

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"name":"Bash","delta":"}"
            })),
            &mut state,
        )
        .expect("later delta");
    let second_delta = receiver.recv().await.expect("later argument delta");
    assert_eq!(second_delta["method"], "item/tool/delta");
    assert_eq!(second_delta["params"]["delta"], "}");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "the same tool must not emit a second start card"
    );
}

#[tokio::test]
async fn delta_less_thinking_end_and_text_end_still_stream() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_end","index":0,"content":"plan the edit"
            })),
            &mut state,
        )
        .expect("thinking_end");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"text_end","index":1,"content":"done"
            })),
            &mut state,
        )
        .expect("text_end");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_end","index":0,"content":"plan the edit"
            })),
            &mut state,
        )
        .expect("duplicate thinking_end");

    let thinking = receiver.recv().await.expect("thinking from end");
    let complete = receiver.recv().await.expect("thinking complete");
    let text = receiver.recv().await.expect("text from end");
    let duplicate_complete = receiver.recv().await.expect("duplicate thinking complete");
    assert_eq!(thinking["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thinking["params"]["delta"], "plan the edit");
    assert_eq!(complete["method"], "item/reasoning/complete");
    assert_eq!(text["method"], "item/agentMessage/delta");
    assert_eq!(text["params"]["delta"], "done");
    assert_eq!(duplicate_complete["method"], "item/reasoning/complete");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "a later thinking_end must not replay already streamed content"
    );
}

#[tokio::test]
async fn thinking_delta_prevents_thinking_end_replay() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_delta","index":3,"delta":"why"
            })),
            &mut state,
        )
        .expect("thinking_delta");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_end","index":3,"content":"why extra"
            })),
            &mut state,
        )
        .expect("thinking_end after delta");

    let thinking = receiver.recv().await.expect("delta thinking");
    let complete = receiver.recv().await.expect("thinking complete");
    assert_eq!(thinking["params"]["delta"], "why");
    assert_eq!(complete["method"], "item/reasoning/complete");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "thinking_end must not append after a streamed delta"
    );
}

#[tokio::test]
async fn omits_unreported_one_hour_cache_usage() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"done","reason":"stop",
                "message":{"usage":{"input":1,"output":2,"cacheWrite":3}}
            })),
            &mut super::EventTranslateState::default(),
        )
        .expect("done");

    let usage = receiver.recv().await.expect("usage event");
    let usage = &usage["params"]["tokenUsage"]["last"];
    assert_eq!(usage["cacheCreationInputTokens"], 3);
    assert!(usage.get("cacheCreation1hInputTokens").is_none());
}

#[tokio::test]
async fn clamps_invalid_usage_subsets_without_underflow() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"done","reason":"stop",
                "message":{"usage":{
                    "output":3,"reasoning":5,"cacheWrite":2,"cacheWrite1h":4
                }}
            })),
            &mut super::EventTranslateState::default(),
        )
        .expect("done");

    let usage = receiver.recv().await.expect("usage event");
    let usage = &usage["params"]["tokenUsage"]["last"];
    assert_eq!(usage["outputTokens"], 0);
    assert_eq!(usage["reasoningOutputTokens"], 3);
    assert_eq!(usage["cacheCreationInputTokens"], 2);
    assert_eq!(usage["cacheCreation1hInputTokens"], 2);
}

#[test]
fn maps_only_supported_pi_stop_reasons() {
    for (reason, expected) in [
        ("stop", "end_turn"),
        ("length", "max_tokens"),
        ("toolUse", "tool_use"),
        ("deferred", "pause_turn"),
    ] {
        assert_eq!(
            super::anthropic_stop_reason(&json!({"reason":reason})).expect("supported reason"),
            expected
        );
    }
    assert!(super::anthropic_stop_reason(&json!({"reason":"pending"})).is_err());
    assert!(super::anthropic_stop_reason(&json!({})).is_err());
}
