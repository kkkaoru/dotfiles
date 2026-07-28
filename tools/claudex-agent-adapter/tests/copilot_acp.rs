use std::{path::Path, sync::Arc, time::Duration};

use claudex_agent_adapter::{
    agent_backend::AgentBackend, anthropic::Bridge, copilot_acp::CopilotAcp, http_router,
};
use reqwest::Client;
use serde_json::{Value, json};

#[tokio::test]
async fn unmatched_claude_child_does_not_inherit_main_copilot_acp_route() {
    const MAIN_MODEL: &str = "gpt-5.6-sol";

    let root = tempfile::tempdir().expect("Copilot ACP child fixture");
    let agent = CopilotAcp::spawn_with_program(
        MAIN_MODEL,
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        root.path().to_owned(),
    )
    .await
    .expect("start Copilot ACP mock");
    let backend = AgentBackend::routed(vec![(MAIN_MODEL.to_owned(), AgentBackend::copilot(agent))]);
    assert_eq!(backend.route_descriptions(), ["gpt-5.6-sol=copilot-acp"]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, MAIN_MODEL.to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Copilot ACP HTTP router");
    let address = listener.local_addr().expect("Copilot ACP HTTP address");
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, MAIN_MODEL.to_owned(), None))
            .await
            .expect("serve Copilot ACP HTTP router");
    });

    let response = Client::new()
        .post(format!("http://{address}/v1/messages"))
        .json(&json!({
            "model":"claude-sonnet-5",
            "max_tokens":128,
            "system":[{"type":"text","text":"cc_is_subagent=true"}],
            "messages":[{"role":"user","content":"inherited child prompt"}]
        }))
        .send()
        .await
        .expect("send unmatched Claude Code child request");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let response = response
        .json::<Value>()
        .await
        .expect("decode unmatched child error");
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("did not match an explicit Agent/Task launch"))
    );
    server.abort();

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert!(
        trace.iter().any(|event| {
            event["arguments"] == json!(["--acp", "--stdio", "--model", MAIN_MODEL])
        })
    );
    assert!(!trace.iter().any(|event| event.get("new_session").is_some()));
}

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
    agent
        .cancel_turn("missing-session")
        .await
        .expect("cancel an absent Copilot turn");
    let copilot = AgentBackend::copilot(agent);
    assert!(copilot.is_alive());
    assert_eq!(copilot.kind().to_string(), "copilot-acp");
    assert!(copilot.request("unsupported", json!({})).await.is_err());
    assert!(
        copilot
            .request_detached("unsupported", json!({}))
            .await
            .is_err()
    );
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
