use std::{path::Path, time::Duration};

use claudex_agent_adapter::{agent_backend::AgentBackend, copilot_acp::CopilotAcp};
use serde_json::{Value, json};

#[tokio::test]
async fn routes_a_selected_model_through_copilot_cli_acp() {
    let root = tempfile::tempdir().expect("Copilot ACP fixture");
    let agent = CopilotAcp::spawn_with_program(
        "gpt-copilot-test",
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        root.path().to_owned(),
    )
    .await
    .expect("start Copilot ACP mock");
    let copilot = AgentBackend::copilot(agent);
    assert!(copilot.is_alive());
    assert_eq!(copilot.kind().to_string(), "copilot-acp");
    let backend = AgentBackend::routed(vec![("gpt-copilot-test".to_owned(), copilot)]);
    assert_eq!(
        backend.route_descriptions(),
        ["gpt-copilot-test=copilot-acp"]
    );
    let response = backend
        .request(
            "thread/start",
            json!({
                "model":"gpt-copilot-test",
                "baseInstructions":"project policy\n\nbridge-only",
                "developerInstructions":"bridge-only"
            }),
        )
        .await
        .expect("create Copilot ACP session");
    let thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .expect("Copilot ACP thread ID");
    let events = backend.subscribe_thread(thread_id);
    backend
        .request_detached(
            "turn/start",
            json!({"threadId":thread_id,"effort":"max","input":"user prompt"}),
        )
        .await
        .expect("start Copilot ACP turn");
    assert_eq!(
        receive(&events)
            .await
            .pointer("/params/delta")
            .and_then(Value::as_str),
        Some("GROK_ACP_STREAM_OK")
    );
    assert_eq!(receive(&events).await["method"], "turn/completed");

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert!(trace.iter().any(|event| {
        event["arguments"] == json!(["--acp", "--stdio", "--model", "gpt-copilot-test"])
    }));
    assert!(
        trace.iter().any(|event| {
            event.pointer("/set_model/_meta/reasoningEffort") == Some(&json!("max"))
        })
    );
    let prompt = trace
        .iter()
        .find_map(|event| {
            event
                .pointer("/prompt/prompt/0/text")
                .and_then(Value::as_str)
        })
        .expect("Copilot ACP prompt");
    assert_eq!(prompt, "project policy\n\nuser prompt");
    assert!(!prompt.contains("Grok SubAgent effort routing"));
    assert!(backend.respond(json!(1), json!({})).await.is_err());
}

async fn receive(events: &claudex_agent_adapter::app_server::ThreadEvents) -> Value {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("Copilot ACP event timeout")
        .expect("Copilot ACP event stream closed")
}

fn read_trace(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("read Copilot ACP trace")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse Copilot ACP trace"))
        .collect()
}
