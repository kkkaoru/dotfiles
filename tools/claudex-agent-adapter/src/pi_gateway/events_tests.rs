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
        alive: AtomicBool::new(true),
    }
}

#[tokio::test]
async fn translates_text_thinking_tool_usage_and_terminal_events() {
    let gateway = gateway();
    let receiver = gateway.subscribe_thread("thread");
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
                    "type":"done","reason":"toolUse","message":{"usage":{"input":3,"output":5}}
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
    assert_eq!(tool["params"]["arguments"], json!({"path":"a"}));
    assert_eq!(usage["params"]["tokenUsage"]["last"]["outputTokens"], 5);
    assert_eq!(done["params"]["turn"]["status"], "completed");
}
