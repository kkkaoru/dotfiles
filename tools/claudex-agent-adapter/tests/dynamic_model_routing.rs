use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use claudex_agent_adapter::{
    agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute, WebSearchMode},
    anthropic::Bridge,
    http_router,
};
use reqwest::{Client, Response};
use serde_json::{Value, json};

#[path = "support/coverage_profile.rs"]
mod coverage_profile;

const DEFAULT_MODEL: &str = "opencode-go/deepseek-v4-flash";
const DYNAMIC_MODEL: &str = "opencode-go/deepseek-v4-runtime-test";
const MODEL_LIMIT: usize = 2;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const POLL_TIMEOUT: Duration = Duration::from_secs(2);
static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn resolves_dynamic_opencode_model_and_reports_exact_model_capacity() {
    let _cwd_guard = CWD_LOCK.lock().await;
    let root = tempfile::tempdir().expect("dynamic model fixture");
    std::env::set_current_dir(root.path()).expect("isolate dynamic ACP trace");

    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: DEFAULT_MODEL.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        effort: None,
        model_provider: None,
        model_catalog_json: None,
        pi_provider: None,
        pi_model: None,
        max_context_tokens: None,
        max_concurrency: Some(MODEL_LIMIT),
        model_prefixes: vec!["opencode-go/".to_owned()],
        acp: Some(AcpLaunch {
            program: coverage_profile::wrapped_program_string(
                root.path(),
                env!("CARGO_BIN_EXE_grok-acp-mock"),
            ),
            arguments: vec!["--mode".to_owned(), "cancellable-turns".to_owned()],
        }),
        web_search_mode: WebSearchMode::default(),
    }]);
    assert!(backend.supports_model(DYNAMIC_MODEL));
    assert!(!backend.models().contains(&DYNAMIC_MODEL.to_owned()));

    let bridge = Arc::new(Bridge::new_with_backend(
        Arc::clone(&backend),
        DEFAULT_MODEL.to_owned(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind dynamic model adapter");
    let address = listener
        .local_addr()
        .expect("dynamic model adapter address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, DEFAULT_MODEL.to_owned(), None),
        )
        .await
        .expect("serve dynamic model adapter");
    });

    let client = Client::new();
    let health_url = format!("http://{address}/health");
    let messages_url = format!("http://{address}/v1/messages");
    let initial = health(&client, &health_url).await;
    assert_eq!(
        initial["model_concurrency"][DEFAULT_MODEL],
        json!({"active":0,"limit":MODEL_LIMIT,"available":true,"queued":0})
    );
    assert!(initial["model_concurrency"].get(DYNAMIC_MODEL).is_none());

    let first = start_blocked_stream(&client, &messages_url, "first").await;
    let second = start_blocked_stream(&client, &messages_url, "second").await;
    wait_for_trace_count(root.path(), "prompt_submitted", 2).await;
    let saturated = wait_for_health(&client, &health_url, |value| {
        value["model_concurrency"][DYNAMIC_MODEL]["active"] == MODEL_LIMIT
            && value["model_concurrency"][DYNAMIC_MODEL]["available"] == false
    })
    .await;
    assert_eq!(
        saturated["model_concurrency"][DYNAMIC_MODEL],
        json!({"active":MODEL_LIMIT,"limit":MODEL_LIMIT,"available":false,"queued":0})
    );
    assert!(
        saturated["started_models"]
            .as_array()
            .is_some_and(|models| models.iter().any(|model| model == DYNAMIC_MODEL))
    );

    drop(first);
    drop(second);
    wait_for_trace_count(root.path(), "cancel", 2).await;
    let released = wait_for_health(&client, &health_url, |value| {
        value["model_concurrency"][DYNAMIC_MODEL]["active"] == 0
            && value["model_concurrency"][DYNAMIC_MODEL]["queued"] == 0
    })
    .await;
    assert_eq!(
        released["model_concurrency"][DYNAMIC_MODEL],
        json!({"active":0,"limit":MODEL_LIMIT,"available":true,"queued":0})
    );

    let completed = client
        .post(&messages_url)
        .json(&json!({
            "model":DYNAMIC_MODEL,
            "messages":[{"role":"user","content":"COMPLETE dynamic model"}]
        }))
        .send()
        .await
        .expect("send released dynamic request")
        .error_for_status()
        .expect("released dynamic request status")
        .json::<Value>()
        .await
        .expect("decode released dynamic response");
    assert_eq!(completed["model"], DYNAMIC_MODEL);
    assert_eq!(completed["content"][0]["text"], "GROK_ACP_STREAM_OK");
    let trace = std::fs::read_to_string(root.path().join("grok-acp-mock.jsonl"))
        .expect("dynamic ACP trace");
    assert!(trace.lines().any(|line| {
        serde_json::from_str::<Value>(line)
            .ok()
            .is_some_and(|event| event["arguments"] == json!(["--mode", "cancellable-turns"]))
    }));

    backend.shutdown().await;
    server.abort();
}

async fn start_blocked_stream(client: &Client, url: &str, label: &str) -> Response {
    let mut response = client
        .post(url)
        .json(&json!({
            "model":DYNAMIC_MODEL,
            "stream":true,
            "messages":[{"role":"user","content":format!("BLOCK {label}")}]
        }))
        .send()
        .await
        .expect("send blocked dynamic stream")
        .error_for_status()
        .expect("blocked dynamic stream status");
    let frame = tokio::time::timeout(Duration::from_millis(500), response.chunk())
        .await
        .expect("dynamic stream message_start timeout")
        .expect("read dynamic stream frame")
        .expect("dynamic stream ended before message_start");
    assert!(String::from_utf8_lossy(&frame).contains("event: message_start"));
    response
}

async fn health(client: &Client, url: &str) -> Value {
    client
        .get(url)
        .send()
        .await
        .expect("request dynamic model health")
        .error_for_status()
        .expect("dynamic model health status")
        .json()
        .await
        .expect("decode dynamic model health")
}

async fn wait_for_health(client: &Client, url: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let value = health(client, url).await;
        if predicate(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "dynamic model health condition timed out"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_trace_count(root: &std::path::Path, key: &str, expected: usize) {
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let count = trace_event_count(root, key);
        if count >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "dynamic ACP trace `{key}` timed out"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn trace_event_count(root: &std::path::Path, key: &str) -> usize {
    std::fs::read_to_string(root.join("grok-acp-mock.jsonl"))
        .map(|trace| {
            trace
                .lines()
                .filter(|line| trace_has_key(line, key))
                .count()
        })
        .unwrap_or_default()
}

fn trace_has_key(line: &str, key: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .is_some_and(|event| event.get(key).is_some())
}
