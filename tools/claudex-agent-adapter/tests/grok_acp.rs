#[path = "support/project_fixture.rs"]
mod project_fixture;

use std::{path::Path, sync::Arc, time::Duration};

use claudex_agent_adapter::{
    agent_backend::AgentBackend, anthropic::Bridge, grok_acp::GrokAcp, http_router,
};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt as _, net::UnixStream};

use project_fixture::ProjectFixture;

const SETUP_RELEASE_SOCKET: &str = "grok-acp-setup-release.sock";

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
    assert!(!trace.iter().any(|event| event.get("cancel").is_some()));
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
    let prompt = trace
        .iter()
        .find_map(|event| {
            event
                .pointer("/prompt/prompt/0/text")
                .and_then(Value::as_str)
        })
        .expect("prompt trace");
    assert!(prompt.starts_with("project policy\n\nGrok SubAgent effort routing:"));
    assert!(prompt.ends_with("\n\nuser prompt"));
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

    for model in ["fail-initialize", "fail-auth"] {
        let root = tempfile::tempdir().expect("startup error fixture");
        let failed = GrokAcp::spawn_with_program(
            model,
            env!("CARGO_BIN_EXE_grok-acp-mock"),
            root.path().to_owned(),
        )
        .await;
        assert!(failed.is_err());
    }

    let root = tempfile::tempdir().expect("session error fixture");
    let agent = spawn_mock("fail-session", root.path()).await;
    assert!(agent.create_session(json!({})).await.is_err());

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

#[tokio::test]
async fn forwards_grok_tool_subagent_retry_and_usage_updates() {
    let root = tempfile::tempdir().expect("coverage update fixture");
    let agent = spawn_mock("coverage-updates", root.path()).await;
    let response = agent.create_session(json!({})).await.unwrap();
    let thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .unwrap();
    let events = agent.subscribe_thread(thread_id);
    agent
        .start_turn(json!({"threadId":thread_id,"input":"coverage"}))
        .await
        .unwrap();

    let mut received = Vec::new();
    loop {
        let event = recv(&events).await;
        let completed = event["method"] == "turn/completed";
        received.push(event);
        if completed {
            break;
        }
    }
    let combined = received.iter().map(Value::to_string).collect::<String>();
    assert!(combined.contains("Completed search"));
    assert!(combined.contains("SubAgent started"));
    assert!(combined.contains("Retrying provider"));
    assert!(combined.contains("tokenUsage"));
}

#[tokio::test]
async fn streams_two_grok_acp_sessions_concurrently() {
    let root = tempfile::tempdir().expect("concurrent fixture");
    let agent = spawn_mock("concurrent-turns", root.path()).await;
    let first = agent.create_session(json!({})).await.unwrap();
    let first_id = first.pointer("/thread/id").and_then(Value::as_str).unwrap();
    let first_events = agent.subscribe_thread(first_id);
    agent
        .start_turn(json!({"threadId":first_id,"input":"first"}))
        .await
        .unwrap();

    let second = tokio::time::timeout(Duration::from_secs(1), agent.create_session(json!({})))
        .await
        .expect("session creation blocked behind an active turn")
        .unwrap();
    let second_id = second
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .unwrap();
    let second_events = agent.subscribe_thread(second_id);
    agent
        .start_turn(json!({"threadId":second_id,"input":"second"}))
        .await
        .unwrap();

    let (first_stream, second_stream) = tokio::join!(recv(&first_events), recv(&second_events));
    for event in [first_stream, second_stream] {
        assert_eq!(
            event.pointer("/params/delta").and_then(Value::as_str),
            Some("GROK_ACP_STREAM_OK"),
            "unexpected concurrent event: {event}"
        );
    }
    let (first_done, second_done) = tokio::join!(recv(&first_events), recv(&second_events));
    assert_eq!(first_done["method"], "turn/completed");
    assert_eq!(second_done["method"], "turn/completed");
}

#[tokio::test]
async fn dropping_http_stream_cancels_the_active_acp_prompt() {
    let root = ProjectFixture::new("disconnect");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("cancellable-turns", root.path()).await;
    let backend = AgentBackend::routed(vec![(
        "cancellable-turns".to_owned(),
        AgentBackend::grok(Arc::clone(&agent)),
    )]);
    let bridge = Arc::new(Bridge::new_with_backend(
        backend,
        "cancellable-turns".to_owned(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "cancellable-turns".to_owned(), None),
        )
        .await
        .unwrap();
    });

    let client = Client::new();
    let response = client
        .post(&url)
        .json(&json!({
            "model":"cancellable-turns",
            "stream":true,
            "messages":[{"role":"user","content":"BLOCK UNTIL DISCONNECT"}]
        }))
        .send()
        .await
        .expect("start cancellable HTTP stream");
    wait_for_trace_count(&trace_path, "prompt_submitted", 1).await;
    drop(response);

    let trace = wait_for_trace_count(&trace_path, "cancel", 1).await;
    let cancelled_session = trace
        .iter()
        .find_map(|event| event.pointer("/cancel/sessionId").and_then(Value::as_str))
        .expect("session/cancel trace");
    assert_eq!(cancelled_session, "grok-session-1");

    let completed: Value = client
        .post(&url)
        .json(&json!({
            "model":"cancellable-turns",
            "messages":[{"role":"user","content":"COMPLETE NORMALLY"}]
        }))
        .send()
        .await
        .expect("start independent session")
        .error_for_status()
        .expect("independent session status")
        .json()
        .await
        .expect("independent session response");
    assert_eq!(completed["content"][0]["text"], "GROK_ACP_STREAM_OK");
    assert_eq!(completed["stop_reason"], "end_turn");
    server.abort();
}

#[tokio::test]
async fn disconnect_during_effort_setup_does_not_submit_or_cancel_a_prompt() {
    let root = ProjectFixture::new("setup");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("blocked-effort", root.path()).await;
    let mut setup_release = UnixStream::connect(root.path().join(SETUP_RELEASE_SOCKET))
        .await
        .expect("connect blocked setup handshake");
    let backend = AgentBackend::routed(vec![(
        "blocked-effort".to_owned(),
        AgentBackend::grok(Arc::clone(&agent)),
    )]);
    let bridge = Arc::new(Bridge::new_with_backend(
        backend,
        "blocked-effort".to_owned(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "blocked-effort".to_owned(), None),
        )
        .await
        .unwrap();
    });

    let client = Client::new();
    let response = client
        .post(&url)
        .json(&json!({
            "model":"blocked-effort",
            "stream":true,
            "output_config":{"effort":"high"},
            "messages":[{"role":"user","content":"DISCONNECT DURING SETUP"}]
        }))
        .send()
        .await
        .expect("start blocked effort stream");
    wait_for_trace_count(&trace_path, "set_model_blocked", 1).await;
    drop(response);
    setup_release
        .write_all(&[1])
        .await
        .expect("release blocked setup after disconnect");
    drop(setup_release);
    wait_for_trace_count(&trace_path, "set_model_settled", 1).await;

    let completed: Value = client
        .post(&url)
        .json(&json!({
            "model":"blocked-effort",
            "messages":[{"role":"user","content":"COMPLETE AFTER SETUP CANCEL"}]
        }))
        .send()
        .await
        .expect("start independent request")
        .error_for_status()
        .expect("independent request status")
        .json()
        .await
        .expect("independent response");
    assert_eq!(completed["stop_reason"], "end_turn");

    let trace = read_trace(&trace_path);
    assert!(!trace.iter().any(|event| event.get("cancel").is_some()));
    assert!(!trace.iter().any(|event| {
        event.pointer("/prompt/sessionId") == Some(&json!("grok-session-1"))
    }));
    server.abort();
}

#[tokio::test]
async fn ignored_setup_invalidates_only_that_session_and_recovers_capacity() {
    let root = ProjectFixture::new("ignored-setup");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("ignored-setup", root.path()).await;
    let capacity = agent.turn_capacity();
    let (setup_id, setup_events) = start_blocked_setup(&agent, &trace_path).await;
    fill_remaining_capacity(&agent, &trace_path, capacity).await;

    let (next_id, next_events) = open_capacity_waiting_session(&agent).await;
    let mut queued = spawn_queued_turn(Arc::clone(&agent), next_id);
    assert_still_queued(&mut queued, "turn started without available capacity").await;

    let error = cancel_blocked_setup_and_expect_timeout(
        Arc::clone(&agent),
        setup_id.clone(),
        &mut queued,
    )
    .await;
    assert!(
        error
            .to_string()
            .contains("setup cancellation did not settle within")
    );
    assert_setup_timeout_event(&setup_events).await;
    await_queued_turn_completion(&mut queued, &next_events).await;
    assert_session_invalidated(&agent, &setup_id).await;
    assert!(!read_trace(&trace_path).iter().any(|event| event.get("cancel").is_some()));
}

async fn start_blocked_setup(
    agent: &GrokAcp,
    trace_path: &Path,
) -> (String, claudex_agent_adapter::app_server::ThreadEvents) {
    let setup = agent.create_session(json!({})).await.unwrap();
    let setup_id = setup["thread"]["id"].as_str().unwrap().to_owned();
    let setup_events = agent.subscribe_thread(&setup_id);
    agent
        .start_turn(json!({
            "threadId":setup_id,
            "effort":"high",
            "input":"SETUP NEVER SETTLES"
        }))
        .await
        .unwrap();
    wait_for_trace_count(trace_path, "set_model_blocked", 1).await;
    (setup_id, setup_events)
}

async fn fill_remaining_capacity(agent: &GrokAcp, trace_path: &Path, capacity: usize) {
    for index in 1..capacity {
        let response = agent.create_session(json!({})).await.unwrap();
        let session_id = response["thread"]["id"].as_str().unwrap();
        agent
            .start_turn(json!({"threadId":session_id,"input":format!("BLOCK {index}")}))
            .await
            .unwrap();
    }
    wait_for_trace_count(trace_path, "prompt_submitted", capacity - 1).await;
}

async fn open_capacity_waiting_session(
    agent: &GrokAcp,
) -> (String, claudex_agent_adapter::app_server::ThreadEvents) {
    let next = tokio::time::timeout(Duration::from_secs(1), agent.create_session(json!({})))
        .await
        .expect("setup settlement blocked independent session creation")
        .unwrap();
    let next_id = next["thread"]["id"].as_str().unwrap().to_owned();
    let next_events = agent.subscribe_thread(&next_id);
    (next_id, next_events)
}

fn spawn_queued_turn(
    agent: Arc<GrokAcp>,
    session_id: String,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        agent
            .start_turn(json!({
                "threadId":session_id,
                "input":"COMPLETE AFTER SETUP TIMEOUT"
            }))
            .await
    })
}

async fn assert_still_queued(
    queued: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
    message: &str,
) {
    assert!(
        tokio::time::timeout(Duration::from_millis(50), queued)
            .await
            .is_err(),
        "{message}"
    );
}

async fn cancel_blocked_setup_and_expect_timeout(
    agent: Arc<GrokAcp>,
    setup_id: String,
    queued: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
) -> anyhow::Error {
    let mut cancellation = tokio::spawn(async move { agent.cancel_turn(&setup_id).await });
    assert_still_queued(queued, "turn started before setup settlement timed out").await;
    tokio::time::timeout(Duration::from_secs(3), &mut cancellation)
        .await
        .expect("ignored setup did not reach its settlement timeout")
        .expect("cancellation task failed")
        .expect_err("ignored setup unexpectedly settled")
}

async fn assert_setup_timeout_event(
    setup_events: &claudex_agent_adapter::app_server::ThreadEvents,
) {
    let setup_error = recv(setup_events).await;
    assert_eq!(setup_error["method"], "error");
    assert!(
        setup_error["params"]["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("setup cancellation did not settle within"))
    );
}

async fn await_queued_turn_completion(
    queued: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
    next_events: &claudex_agent_adapter::app_server::ThreadEvents,
) {
    tokio::time::timeout(Duration::from_secs(1), queued)
        .await
        .expect("queued turn did not recover released capacity")
        .expect("queued turn task failed")
        .expect("queued turn start failed");
    assert_eq!(recv(next_events).await["params"]["delta"], "GROK_ACP_STREAM_OK");
    assert_eq!(
        recv(next_events).await["params"]["turn"]["status"],
        "completed"
    );
}

async fn assert_session_invalidated(agent: &GrokAcp, setup_id: &str) {
    let invalidated = agent
        .start_turn(json!({"threadId":setup_id,"input":"MUST NOT REUSE"}))
        .await
        .expect_err("timed-out setup session was reused");
    assert!(invalidated.to_string().contains("was invalidated"));
}

#[tokio::test]
async fn cancelled_turn_releases_capacity_for_another_session() {
    let root = ProjectFixture::new("cancel");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("cancellable-turns", root.path()).await;
    let capacity = agent.turn_capacity();
    let mut blocked = Vec::with_capacity(capacity);
    for index in 0..capacity {
        let response = agent.create_session(json!({})).await.unwrap();
        let session_id = response["thread"]["id"].as_str().unwrap().to_owned();
        let events = agent.subscribe_thread(&session_id);
        agent
            .start_turn(json!({
                "threadId":session_id,
                "input":format!("BLOCK {index}")
            }))
            .await
            .unwrap();
        blocked.push((session_id, events));
    }
    wait_for_trace_count(&trace_path, "prompt", capacity).await;

    let next = agent.create_session(json!({})).await.unwrap();
    let next_id = next["thread"]["id"].as_str().unwrap().to_owned();
    let next_events = agent.subscribe_thread(&next_id);
    let queued_agent = Arc::clone(&agent);
    let queued_id = next_id.clone();
    let mut queued = tokio::spawn(async move {
        queued_agent
            .start_turn(json!({"threadId":queued_id,"input":"COMPLETE AFTER CANCEL"}))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut queued)
            .await
            .is_err(),
        "turn started without available capacity"
    );

    agent.cancel_turn(&blocked[0].0).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut queued)
        .await
        .expect("queued turn did not acquire released capacity")
        .expect("queued turn task failed")
        .expect("queued turn start failed");
    let cancelled = recv(&blocked[0].1).await;
    assert_eq!(cancelled["params"]["turn"]["status"], "cancelled");

    let output = recv(&next_events).await;
    assert_eq!(output["params"]["delta"], "GROK_ACP_STREAM_OK");
    let completed = recv(&next_events).await;
    assert_eq!(completed["params"]["turn"]["status"], "completed");
}

#[tokio::test]
async fn ignored_cancellation_invalidates_only_that_session_and_recovers_capacity() {
    let root = ProjectFixture::new("ignored");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("ignored-cancellation", root.path()).await;
    let capacity = agent.turn_capacity();
    let mut blocked = Vec::with_capacity(capacity);
    for index in 0..capacity {
        let response = agent.create_session(json!({})).await.unwrap();
        let session_id = response["thread"]["id"].as_str().unwrap().to_owned();
        agent
            .start_turn(json!({"threadId":session_id,"input":format!("BLOCK {index}")}))
            .await
            .unwrap();
        blocked.push(session_id);
    }
    wait_for_trace_count(&trace_path, "prompt_submitted", capacity).await;

    let cancelled_id = blocked[0].clone();
    let cancelling_agent = Arc::clone(&agent);
    let mut cancellation =
        tokio::spawn(async move { cancelling_agent.cancel_turn(&cancelled_id).await });
    wait_for_trace_count(&trace_path, "cancel", 1).await;

    let next = tokio::time::timeout(Duration::from_secs(1), agent.create_session(json!({})))
        .await
        .expect("cancellation settlement blocked independent session creation")
        .unwrap();
    let next_id = next["thread"]["id"].as_str().unwrap().to_owned();
    let next_events = agent.subscribe_thread(&next_id);
    let queued_agent = Arc::clone(&agent);
    let queued_id = next_id.clone();
    let mut queued = tokio::spawn(async move {
        queued_agent
            .start_turn(json!({"threadId":queued_id,"input":"COMPLETE AFTER TIMEOUT"}))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut queued)
            .await
            .is_err(),
        "turn started before cancellation settlement timed out"
    );

    let error = tokio::time::timeout(Duration::from_secs(3), &mut cancellation)
        .await
        .expect("ignored cancellation did not reach its settlement timeout")
        .expect("cancellation task failed")
        .expect_err("ignored cancellation unexpectedly settled");
    assert!(error.to_string().contains("did not settle within"));
    tokio::time::timeout(Duration::from_secs(1), &mut queued)
        .await
        .expect("queued turn did not recover released capacity")
        .expect("queued turn task failed")
        .expect("queued turn start failed");
    assert_eq!(recv(&next_events).await["params"]["delta"], "GROK_ACP_STREAM_OK");
    assert_eq!(
        recv(&next_events).await["params"]["turn"]["status"],
        "completed"
    );

    let invalidated = agent
        .start_turn(json!({"threadId":blocked[0],"input":"MUST NOT REUSE"}))
        .await
        .expect_err("timed-out session was reused");
    assert!(invalidated.to_string().contains("was invalidated"));
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

async fn wait_for_trace_count(path: &Path, key: &str, expected: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let trace = try_read_trace(path).unwrap_or_default();
            if trace
                .iter()
                .filter(|event| event.get(key).is_some())
                .count()
                >= expected
            {
                return trace;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ACP trace event timeout")
}

fn try_read_trace(path: &Path) -> Option<Vec<Value>> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents
        .lines()
        .map(|line| serde_json::from_str(line).ok())
        .collect()
}
