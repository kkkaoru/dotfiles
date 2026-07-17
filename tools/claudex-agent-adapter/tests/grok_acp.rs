use std::{path::Path, time::Duration};

use claudex_agent_adapter::{agent_backend::AgentBackend, grok_acp::GrokAcp};
use serde_json::{Value, json};

#[tokio::test]
async fn streams_grok_acp_and_forwards_model_effort_and_instructions() {
    let root = tempfile::tempdir().expect("Grok ACP fixture");
    let agent = GrokAcp::spawn_with_program(
        "grok-4.5",
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        root.path().to_owned(),
    )
    .await
    .expect("start Grok ACP mock");
    let backend = AgentBackend::grok(agent);
    assert!(backend.is_alive());
    assert_eq!(backend.kind().to_string(), "grok-acp");
    let response = backend
        .request(
            "thread/start",
            json!({
                "baseInstructions":"project policy\n\nCodex bridge policy",
                "developerInstructions":"Codex bridge policy"
            }),
        )
        .await
        .expect("create ACP session");
    let thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .unwrap();
    let events = backend.subscribe_thread(thread_id);
    for effort in ["low", "mid", "xhigh"] {
        backend
            .request_detached(
                "turn/start",
                json!({
                    "threadId":thread_id,
                    "effort":effort,
                    "input":[{"type":"text","text":"user prompt"}]
                }),
            )
            .await
            .expect("start ACP turn");
        let first = recv(&events).await;
        let second = recv(&events).await;
        assert_eq!(
            first.pointer("/params/delta").and_then(Value::as_str),
            Some("GROK_ACP_STREAM_OK"),
            "unexpected first event: {first}"
        );
        assert_eq!(
            second.get("method").and_then(Value::as_str),
            Some("turn/completed")
        );
    }

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert_trace(&trace);
    assert!(backend.request("unsupported", json!({})).await.is_err());
    assert!(
        backend
            .request_detached("unsupported", json!({}))
            .await
            .is_err()
    );
    assert!(backend.respond(json!(1), json!({})).await.is_err());
}

fn assert_trace(trace: &[Value]) {
    assert!(
        trace
            .iter()
            .any(|event| event["arguments"] == json!(["--model", "grok-4.5", "agent", "stdio"]))
    );
    assert!(
        trace
            .iter()
            .any(|event| event.pointer("/new_session/_meta/modelId") == Some(&json!("grok-4.5")))
    );
    for effort in ["low", "medium", "high"] {
        assert!(
            trace
                .iter()
                .any(|event| event.pointer("/set_model/_meta/reasoningEffort")
                    == Some(&json!(effort)))
        );
    }
    assert!(trace.iter().any(
        |event| event.pointer("/permission_response/outcome/optionId")
            == Some(&json!("allow-once"))
    ));
    assert!(
        trace
            .iter()
            .any(|event| event.pointer("/prompt/prompt/0/text")
                == Some(&json!("project policy\n\nuser prompt")))
    );
}

#[tokio::test]
async fn reports_acp_startup_effort_and_prompt_failures() {
    let missing = GrokAcp::spawn_with_program(
        "model",
        "/definitely/missing/grok",
        std::env::current_dir().unwrap(),
    )
    .await;
    assert!(missing.is_err());
    let root = tempfile::tempdir().expect("protocol fixture");
    let incompatible = GrokAcp::spawn_with_program(
        "bad-version",
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        root.path().to_owned(),
    )
    .await;
    assert!(incompatible.is_err());

    for (model, effort, expected) in [
        ("fail-effort", Some("high"), "set effort failed"),
        ("fail-prompt", None, "Internal error"),
    ] {
        let root = tempfile::tempdir().expect("error fixture");
        let agent = spawn_mock(model, root.path()).await;
        let response = agent.create_session(json!({})).await.unwrap();
        let thread_id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .unwrap();
        let events = agent.subscribe_thread(thread_id);
        agent
            .start_turn(json!({"threadId":thread_id,"effort":effort,"input":null}))
            .await
            .unwrap();
        let event = recv(&events).await;
        assert_eq!(event.get("method").and_then(Value::as_str), Some("error"));
        let message = event
            .pointer("/params/error/message")
            .and_then(Value::as_str)
            .unwrap();
        assert!(message.contains(expected), "unexpected error: {message}");
    }

    let root = tempfile::tempdir().expect("no-auth fixture");
    let agent = spawn_mock("no-auth", root.path()).await;
    assert!(agent.is_alive());
}

async fn spawn_mock(model: &str, cwd: &Path) -> std::sync::Arc<GrokAcp> {
    GrokAcp::spawn_with_program(model, env!("CARGO_BIN_EXE_grok-acp-mock"), cwd.to_owned())
        .await
        .expect("start Grok ACP mock")
}

async fn recv(events: &claudex_agent_adapter::app_server::ThreadEvents) -> Value {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("ACP event timeout")
        .expect("ACP event stream closed")
}

fn read_trace(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("read ACP trace")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse ACP trace"))
        .collect()
}
