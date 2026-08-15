use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use serde_json::{Value, json};

use super::super::{ActiveTurn, PiGateway};
use crate::app_server::ThreadEventDispatcher;

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
    let mut tools = HashMap::new();

    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"text_delta","index":0,"delta":"hello"
                })),
                &mut tools
            )
            .expect("text delta")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"thinking_delta","index":1,"delta":"reason"
                })),
                &mut tools
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
                &mut tools
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
                &mut tools
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
                &mut tools
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
                &mut tools
            )
            .expect("done")
    );

    let text = receiver.recv().await.expect("text event");
    let thinking = receiver.recv().await.expect("thinking event");
    let tool = receiver.recv().await.expect("tool event");
    let usage = receiver.recv().await.expect("usage event");
    let done = receiver.recv().await.expect("done event");
    assert_eq!(text["method"], "item/agentMessage/delta");
    assert_eq!(thinking["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thinking["params"]["summaryIndex"], 0);
    assert_eq!(tool["params"]["arguments"], json!({"path":"a"}));
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
            &mut HashMap::new(),
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
            &mut HashMap::new(),
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
