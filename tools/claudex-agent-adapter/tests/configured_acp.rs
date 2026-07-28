use std::{sync::Arc, time::Duration};

use claudex_agent_adapter::{
    agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute},
    anthropic::Bridge,
    http_router,
};
use reqwest::Client;
use serde_json::{Value, json};

const ACP_EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIGURED_PARALLEL_LIMIT: usize = 7;
const EXPECTED_QUEUED_REQUESTS: usize = 1;
const TRACE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PARALLEL_RELEASE_FILE: &str = "grok-acp-parallel-release";
static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allows_session_creations_up_to_the_configured_concurrency_limit() {
    let _cwd_guard = CWD_LOCK.lock().await;
    let root = tempfile::tempdir().expect("configured session concurrency fixture");
    std::env::set_current_dir(root.path()).expect("isolate configured session trace");
    let model = "opencode-go/deepseek-v4-flash";
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: model.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency: Some(CONFIGURED_PARALLEL_LIMIT),
        model_prefixes: vec!["opencode-go/".to_owned()],
        acp: Some(AcpLaunch {
            program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
            arguments: vec![
                "--mode".to_owned(),
                "concurrent-sessions-at-limit".to_owned(),
            ],
        }),
    }]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, model.to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind session concurrency adapter");
    let address = listener
        .local_addr()
        .expect("session concurrency adapter address");
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, model.to_owned(), None))
            .await
            .expect("serve session concurrency adapter");
    });

    let client = Client::new();
    let url = format!("http://{address}/v1/messages");
    let health_url = format!("http://{address}/health");
    let mut requests = Vec::new();
    for index in 0..=CONFIGURED_PARALLEL_LIMIT {
        let client = client.clone();
        let url = url.clone();
        requests.push(tokio::spawn(async move {
            client
                .post(url)
                .json(&json!({
                    "model":model,
                    "max_tokens":128,
                    "messages":[{"role":"user","content":format!("session {index}")}]
                }))
                .send()
                .await
                .expect("send session concurrency turn")
                .error_for_status()
                .expect("session concurrency turn status")
                .json::<Value>()
                .await
                .expect("decode session concurrency turn")
        }));
    }

    let saturated = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let health = client
                .get(&health_url)
                .send()
                .await
                .expect("session concurrency health")
                .json::<Value>()
                .await
                .expect("decode session concurrency health");
            let status = &health["model_concurrency"][model];
            if status["active"] == CONFIGURED_PARALLEL_LIMIT
                && status["queued"] == EXPECTED_QUEUED_REQUESTS
                && session_count(root.path()) == CONFIGURED_PARALLEL_LIMIT
            {
                break health;
            }
            tokio::time::sleep(TRACE_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("session/new calls should reach the configured limit");
    assert_eq!(
        saturated["model_concurrency"][model],
        json!({
            "active":CONFIGURED_PARALLEL_LIMIT,
            "limit":CONFIGURED_PARALLEL_LIMIT,
            "available":false,
            "queued":EXPECTED_QUEUED_REQUESTS
        })
    );
    assert_eq!(session_count(root.path()), CONFIGURED_PARALLEL_LIMIT);

    std::fs::write(root.path().join(PARALLEL_RELEASE_FILE), b"release")
        .expect("release parallel sessions");
    for request in requests {
        let response = tokio::time::timeout(ACP_EVENT_TIMEOUT, request)
            .await
            .expect("session concurrency turn timed out")
            .expect("session concurrency task failed");
        assert_eq!(response["content"][0]["text"], "GROK_ACP_STREAM_OK");
    }
    // Once the queued request acquires a permit, it may either create a new session or reuse one
    // of the seven sessions released above. Both are valid; every request must still be prompted.
    assert_eq!(
        prompt_count(root.path()),
        CONFIGURED_PARALLEL_LIMIT + EXPECTED_QUEUED_REQUESTS,
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enforces_seven_exact_model_turns_and_queues_the_eighth() {
    let _cwd_guard = CWD_LOCK.lock().await;
    let root = tempfile::tempdir().expect("configured concurrency fixture");
    std::env::set_current_dir(root.path()).expect("isolate configured concurrency trace");
    let model = "opencode-go/deepseek-v4-flash";
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: model.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency: Some(CONFIGURED_PARALLEL_LIMIT),
        model_prefixes: vec!["opencode-go/".to_owned()],
        acp: Some(AcpLaunch {
            program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
            arguments: vec!["--mode".to_owned(), "concurrent-turns-seven".to_owned()],
        }),
    }]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, model.to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind concurrency adapter");
    let address = listener.local_addr().expect("concurrency adapter address");
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, model.to_owned(), None))
            .await
            .expect("serve concurrency adapter");
    });
    // Two independent HTTP clients model two Claude Code main sessions sharing one daemon.
    let client_a = Client::new();
    let client_b = Client::new();
    let health_url = format!("http://{address}/health");
    let initial = client_a
        .get(&health_url)
        .send()
        .await
        .expect("initial health")
        .json::<Value>()
        .await
        .expect("decode initial health");
    assert_eq!(
        initial["model_concurrency"][model],
        json!({
            "active":0,
            "limit":CONFIGURED_PARALLEL_LIMIT,
            "available":true,
            "queued":0
        })
    );

    let url = format!("http://{address}/v1/messages");
    let mut turns = Vec::new();
    for index in 0..=CONFIGURED_PARALLEL_LIMIT {
        let client = if index % 2 == 0 {
            client_a.clone()
        } else {
            client_b.clone()
        };
        let url = url.clone();
        turns.push(tokio::spawn(async move {
            client
                .post(url)
                .json(&json!({
                    "model":model,
                    "max_tokens":128,
                    "messages":[{"role":"user","content":format!("parallel {index}")}]
                }))
                .send()
                .await
                .expect("send parallel turn")
                .error_for_status()
                .expect("parallel turn status")
                .json::<Value>()
                .await
                .expect("decode parallel turn")
        }));
    }

    let saturated = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let health = client_a
                .get(&health_url)
                .send()
                .await
                .expect("saturated health")
                .json::<Value>()
                .await
                .expect("decode saturated health");
            let status = &health["model_concurrency"][model];
            if status["active"] == CONFIGURED_PARALLEL_LIMIT
                && status["queued"] == EXPECTED_QUEUED_REQUESTS
                && prompt_count(root.path()) == CONFIGURED_PARALLEL_LIMIT
            {
                break health;
            }
            tokio::time::sleep(TRACE_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("seven prompts should run while the eighth waits");
    assert_eq!(
        saturated["model_concurrency"][model],
        json!({
            "active":CONFIGURED_PARALLEL_LIMIT,
            "limit":CONFIGURED_PARALLEL_LIMIT,
            "available":false,
            "queued":EXPECTED_QUEUED_REQUESTS
        })
    );

    std::fs::write(root.path().join(PARALLEL_RELEASE_FILE), b"release")
        .expect("release parallel prompts");
    for turn in turns {
        let response = tokio::time::timeout(ACP_EVENT_TIMEOUT, turn)
            .await
            .expect("parallel turn timed out")
            .expect("parallel turn task failed");
        assert_eq!(response["content"][0]["text"], "GROK_ACP_STREAM_OK");
    }
    assert_eq!(prompt_count(root.path()), CONFIGURED_PARALLEL_LIMIT + 1);
    server.abort();
}

fn prompt_count(root: &std::path::Path) -> usize {
    trace_event_count(root, "prompt")
}

fn session_count(root: &std::path::Path) -> usize {
    trace_event_count(root, "new_session")
}

fn trace_event_count(root: &std::path::Path, key: &str) -> usize {
    std::fs::read_to_string(root.join("grok-acp-mock.jsonl"))
        .map(|trace| {
            trace
                .lines()
                .filter(|line| {
                    serde_json::from_str::<Value>(line)
                        .ok()
                        .is_some_and(|event| event.get(key).is_some())
                })
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn configured_acp_routes_dynamic_models_and_expands_arguments() {
    let _cwd_guard = CWD_LOCK.lock().await;
    assert!(
        AgentBackend::spawn(BackendKind::ConfiguredAcp, "missing-launch")
            .await
            .is_err()
    );
    let root = tempfile::tempdir().expect("configured ACP fixture");
    std::env::set_current_dir(root.path()).expect("isolate ACP trace");
    let request_cwd = root.path().join("request-project");
    std::fs::create_dir(&request_cwd).expect("create request project");
    let request_cwd = request_cwd
        .canonicalize()
        .expect("canonicalize request project");
    let route = BackendRoute {
        model: "vendor-default".to_owned(),
        backend: BackendKind::ConfiguredAcp,
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        model_prefixes: vec!["vendor-".to_owned()],
        max_concurrency: None,
        acp: Some(AcpLaunch {
            program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
            arguments: vec!["--model".to_owned(), "{model}".to_owned()],
        }),
    };
    let backend = AgentBackend::spawn_routes(&[route]);
    let response = backend
        .request(
            "thread/start",
            json!({
                "model":"vendor-next",
                "cwd":"/adapter/launch/directory/must-not-win",
                "baseInstructions":format!(
                    "Project policy\n- Primary working directory: {}\nBridge policy",
                    request_cwd.display()
                )
            }),
        )
        .await
        .expect("start configured ACP session");
    assert!(response.pointer("/thread/id").is_some());
    assert_eq!(backend.started_models(), ["vendor-next"]);
    assert!(backend.route_descriptions()[0].contains("configured-acp"));
    let thread_id = response["thread"]["id"].as_str().unwrap();
    let receiver = backend.subscribe_thread(thread_id);
    backend
        .request_detached(
            "turn/start",
            json!({"threadId":thread_id,"input":"configured prompt","effort":"xhigh"}),
        )
        .await
        .expect("start configured ACP turn");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
        .await
        .expect("configured ACP event");
    assert!(
        backend
            .respond_for_model("vendor-next", json!(1), json!({}))
            .await
            .is_err()
    );

    assert_configured_trace(root.path(), &request_cwd);

    assert_params_cwd(&backend, root.path()).await;

    let agent = claudex_agent_adapter::grok_acp::GrokAcp::spawn_configured(
        "vendor-leaf",
        &AcpLaunch {
            program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
            arguments: vec!["--model".to_owned(), "{model}".to_owned()],
        },
    )
    .await
    .expect("start configured ACP leaf");
    let leaf = AgentBackend::configured_acp(agent);
    assert_eq!(leaf.kind(), BackendKind::ConfiguredAcp);
    assert!(leaf.is_alive());
    assert!(leaf.request("unsupported", json!({})).await.is_err());
    assert!(
        leaf.request_detached("unsupported", json!({}))
            .await
            .is_err()
    );
    assert!(leaf.respond(json!(1), json!({})).await.is_err());

    session_scoped_configured_acp_recycles_after_one_failed_stream().await;
}

async fn session_scoped_configured_acp_recycles_after_one_failed_stream() {
    let root = tempfile::tempdir().expect("session-scoped ACP fixture");
    std::env::set_current_dir(root.path()).expect("isolate ACP trace");
    let model = "opencode-go/deepseek-v4-flash";
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: model.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        model_prefixes: Vec::new(),
        max_concurrency: None,
        acp: Some(AcpLaunch {
            program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
            arguments: vec!["--mode".to_owned(), "fail-prompt-once".to_owned()],
        }),
    }]);
    let response = backend
        .request("thread/start", json!({"model":model,"cwd":root.path()}))
        .await
        .expect("start session");
    let thread_id = response["thread"]["id"].as_str().unwrap();
    let receiver = backend.subscribe_thread(thread_id);
    backend
        .request_detached(
            "turn/start",
            json!({"threadId":thread_id,"input":"do work","effort":"high"}),
        )
        .await
        .expect("start failing turn");
    let failed = tokio::time::timeout(ACP_EVENT_TIMEOUT, receiver.recv())
        .await
        .expect("failed turn event")
        .expect("failed turn event dispatcher");
    assert_eq!(failed["method"], "error");
    assert!(
        failed["params"]["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("recycling provider"))
    );
    assert!(backend.started_models().is_empty());

    let restarted = backend
        .request("thread/start", json!({"model":model,"cwd":root.path()}))
        .await
        .expect("restart configured ACP after failed stream");
    let restarted_thread = restarted["thread"]["id"].as_str().unwrap();
    let restarted_receiver = backend.subscribe_thread(restarted_thread);
    backend
        .request_detached(
            "turn/start",
            json!({"threadId":restarted_thread,"input":"finish work","effort":"high"}),
        )
        .await
        .expect("start turn on recycled provider");
    tokio::time::timeout(ACP_EVENT_TIMEOUT, async {
        loop {
            let event = restarted_receiver
                .recv()
                .await
                .expect("recycled provider event dispatcher");
            if event["method"] == "turn/completed" {
                break;
            }
            assert_ne!(
                event["method"], "error",
                "recycled provider failed: {event}"
            );
        }
    })
    .await
    .expect("recycled provider completed turn");

    let trace = std::fs::read_to_string(root.path().join("grok-acp-mock.jsonl"))
        .expect("configured ACP trace")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("trace event"))
        .collect::<Vec<_>>();
    assert!(trace.iter().any(|event| {
        event
            .pointer("/set_model/modelId")
            .is_some_and(|configured| configured == model)
    }));
    let prompts = trace
        .iter()
        .filter_map(|event| event.get("prompt"))
        .collect::<Vec<_>>();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].to_string().contains("do work"));
    assert!(prompts[1].to_string().contains("finish work"));
}

async fn assert_params_cwd(backend: &AgentBackend, root: &std::path::Path) {
    let params_cwd = root.canonicalize().expect("canonical params cwd");
    let response = backend
        .request(
            "thread/start",
            json!({"model":"vendor-next","cwd":params_cwd,"baseInstructions":"no cwd marker"}),
        )
        .await
        .expect("start configured ACP session from request cwd");
    assert!(response.pointer("/thread/id").is_some());
    assert_configured_session_cwd(root, &params_cwd);
}

fn assert_configured_session_cwd(root: &std::path::Path, expected: &std::path::Path) {
    let trace =
        std::fs::read_to_string(root.join("grok-acp-mock.jsonl")).expect("configured ACP trace");
    assert!(trace.lines().any(|line| {
        serde_json::from_str::<Value>(line)
            .ok()
            .is_some_and(|event| event["new_session"]["cwd"] == json!(expected))
    }));
}

fn assert_configured_trace(root: &std::path::Path, request_cwd: &std::path::Path) {
    let trace = std::fs::read_to_string(root.join("grok-acp-mock.jsonl"))
        .expect("configured ACP trace")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("trace event"))
        .collect::<Vec<_>>();
    assert_eq!(trace[0]["arguments"], json!(["--model", "vendor-next"]));
    assert!(
        trace
            .iter()
            .any(|event| event["new_session"]["cwd"] == json!(request_cwd))
    );
}
