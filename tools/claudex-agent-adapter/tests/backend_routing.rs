use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

use claudex_agent_adapter::{
    agent_backend::AgentBackend, anthropic::Bridge, app_server::AppServer, grok_acp::GrokAcp,
    http_router, provider_config,
};
use reqwest::Client;
use serde_json::{Value, json};

#[tokio::test]
async fn routes_main_and_subagent_models_to_coexisting_backends() {
    let root = tempfile::tempdir().expect("routing fixture");
    let source = root.path().join("codex-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("auth.json"), "{}").unwrap();
    let codex = AppServer::spawn_with_program(
        "gpt-model",
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &root.path().join("codex-home"),
    )
    .await
    .expect("start Codex backend");
    let grok = GrokAcp::spawn_with_program(
        "grok-model",
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        root.path().to_owned(),
    )
    .await
    .expect("start Grok backend");
    let backend = AgentBackend::routed(vec![
        ("gpt-model".to_owned(), AgentBackend::codex(codex)),
        ("grok-model".to_owned(), AgentBackend::grok(grok)),
    ]);
    assert!(backend.is_alive());
    assert!(backend.supports_model("gpt-model"));
    assert_eq!(backend.models(), ["gpt-model", "grok-model"]);
    assert_eq!(
        backend.route_descriptions(),
        ["gpt-model=codex-app-server", "grok-model=grok-acp"]
    );
    assert!(
        backend
            .request("thread/start", json!({"model":"unknown"}))
            .await
            .is_err()
    );
    assert!(backend.request("unsupported", json!({})).await.is_err());
    assert!(
        backend
            .request_detached("unsupported", json!({}))
            .await
            .is_err()
    );
    backend.respond(json!(999), json!({})).await.unwrap();
    backend
        .respond_for_model("gpt-model", json!(998), json!({}))
        .await
        .unwrap();
    assert!(
        backend
            .respond_for_model("unknown", json!(997), json!({}))
            .await
            .is_err()
    );
    assert!(
        backend
            .respond_for_model("grok-model", json!(996), json!({}))
            .await
            .is_err()
    );
    let bridge = Arc::new(Bridge::new_with_backend(
        Arc::clone(&backend),
        "gpt-model".to_owned(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "gpt-model".to_owned(), None))
            .await
            .unwrap();
    });

    let client = Client::new();
    let url = format!("http://{address}/v1/messages");
    let codex_response = request(&client, &url, "gpt-model").await;
    let grok_response = request(&client, &url, "grok-model").await;
    assert_eq!(response_text(&codex_response), "OK");
    assert_eq!(response_text(&grok_response), "GROK_ACP_STREAM_OK");
    server.abort();
}

#[tokio::test]
async fn health_reports_unavailable_after_a_leaf_backend_stops() {
    let root = tempfile::tempdir().expect("health fixture");
    let source = root.path().join("codex-source");
    std::fs::create_dir(&source).expect("create Codex source home");
    std::fs::write(source.join("auth.json"), "{}").expect("write Codex auth");
    let app_server = AppServer::spawn_with_program(
        "health-model",
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &root.path().join("codex-home"),
    )
    .await
    .expect("start Codex backend");
    let backend = AgentBackend::codex(app_server);
    backend.shutdown().await;

    let bridge = Arc::new(Bridge::new_with_backend(backend, "health-model".to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind health listener");
    let address = listener.local_addr().expect("read health listener address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "health-model".to_owned(), None),
        )
        .await
        .expect("serve health endpoint");
    });

    let response = Client::new()
        .get(format!("http://{address}/health"))
        .send()
        .await
        .expect("request unavailable health");
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<Value>().await.expect("decode health")["status"],
        "unavailable"
    );
    server.abort();
}

#[tokio::test]
async fn context_window_recovery_is_model_agnostic_for_fugu_routes() {
    let root = tempfile::tempdir().expect("Fugu routing fixture");
    let source = root.path().join("codex-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("auth.json"), "{}").unwrap();
    let codex = AppServer::spawn_with_program(
        "bootstrap-model",
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &root.path().join("codex-home"),
    )
    .await
    .expect("start Codex backend");
    let backend = AgentBackend::routed(vec![
        ("fugu".to_owned(), AgentBackend::codex(Arc::clone(&codex))),
        (
            "fugu-ultra-v1.1".to_owned(),
            AgentBackend::codex(Arc::clone(&codex)),
        ),
    ]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, "fugu".to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "fugu".to_owned(), None))
            .await
            .unwrap();
    });

    for model in ["fugu", "fugu-ultra-v1.1"] {
        let response = Client::new()
            .post(&url)
            .json(&json!({
                "model":model,
                "messages":[{"role":"user","content":"CONTEXT_WINDOW_ONCE"}]
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(response["model"], model);
        assert_eq!(response_text(&response), "OK_AFTER_CONTEXT_RESTART");
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolates_parallel_sessions_across_worker_threads_and_backends() {
    let root = tempfile::tempdir().expect("parallel routing fixture");
    let source = root.path().join("parallel-codex-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("auth.json"), "{}").unwrap();
    let codex = AppServer::spawn_with_program(
        "gpt-model",
        env!("CARGO_BIN_EXE_routing-codex-mock"),
        &source,
        &root.path().join("parallel-codex-home"),
    )
    .await
    .unwrap();
    let grok = GrokAcp::spawn_with_program(
        "grok-model",
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        root.path().to_owned(),
    )
    .await
    .unwrap();
    let backend = AgentBackend::routed(vec![
        ("gpt-model".to_owned(), AgentBackend::codex(codex)),
        ("grok-model".to_owned(), AgentBackend::grok(grok)),
    ]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, "gpt-model".to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "gpt-model".to_owned(), None))
            .await
            .unwrap();
    });

    // Prove Codex and Grok routes progress concurrently without sharing session
    // state. Keep each wave to one pair so the single-threaded ACP LocalSet is
    // not overwhelmed by mock permission fan-out.
    for pair in 0..10 {
        let codex_url = url.clone();
        let grok_url = url.clone();
        let codex_index = pair * 2;
        let grok_index = pair * 2 + 1;
        let codex_task = tokio::spawn(assert_parallel_response(
            codex_url,
            "gpt-model",
            codex_index,
            "CODEX_ROUTED_OK",
        ));
        let grok_task = tokio::spawn(assert_parallel_response(
            grok_url,
            "grok-model",
            grok_index,
            "GROK_ACP_STREAM_OK",
        ));
        tokio::try_join!(codex_task, grok_task).expect("mixed Codex/Grok pair must complete");
    }
    server.abort();
}

async fn assert_parallel_response(
    url: String,
    model: &'static str,
    index: usize,
    expected: &'static str,
) {
    let response = parallel_request(&url, model, index).await;
    assert_eq!(response_text(&response), expected);
}

async fn request(client: &Client, url: &str, model: &str) -> Value {
    client
        .post(url)
        .json(&json!({
            "model":model,
            "max_tokens":128,
            "messages":[{"role":"user","content":"Say OK"}]
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn response_text(response: &Value) -> &str {
    response
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .expect("response text")
}

async fn spawn_subscription_failover_bridge(
    root: &std::path::Path,
    subscription_result: &str,
) -> (String, tokio::task::JoinHandle<()>) {
    let source = root.join("codex-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("auth.json"), "{}").unwrap();
    let subscription_program = root.join("claude-mock");
    let script = format!(
        "#!/bin/sh\nset -eu\ncat >/dev/null\nprintf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"{result}\"}}'\n",
        result = subscription_result,
    );
    fs::write(&subscription_program, script).unwrap();
    let mut permissions = fs::metadata(&subscription_program).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&subscription_program, permissions).unwrap();
    let codex = AppServer::spawn_with_program(
        "fugu",
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &root.join("codex-home"),
    )
    .await
    .expect("start Codex backend");
    let provider_config = provider_config::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.config/claudex/providers.json")
            .as_path(),
    )
    .expect("load configured subscription fallback");
    let bridge = Arc::new(
        Bridge::new_with_subscription_program(
            Arc::clone(&codex),
            "fugu".to_owned(),
            &subscription_program,
        )
        .with_model_catalog(provider_config.model_catalog)
        .with_usage_limit_cache_home(root),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "fugu".to_owned(), None))
            .await
            .unwrap();
    });
    (url, server)
}

async fn post_fugu_message(url: &str, content: &str) -> Value {
    Client::new()
        .post(url)
        .json(&json!({
            "model":"fugu",
            "messages":[{"role":"user","content":content}]
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

#[tokio::test]
async fn usage_limit_failsover_from_codex_app_server_to_subscription_route() {
    let root = tempfile::tempdir().expect("usage-limit routing fixture");
    let (url, server) =
        spawn_subscription_failover_bridge(root.path(), "SUBSCRIPTION_FAILOVER_OK").await;

    let response = post_fugu_message(&url, "USAGE_LIMIT_ALWAYS").await;
    assert_eq!(response["model"], "claude-sonnet-5");
    assert_eq!(response_text(&response), "SUBSCRIPTION_FAILOVER_OK");

    let cooldown = root
        .path()
        .join(".cache/claudex/codex-app-server-usage-limit.json");
    assert!(
        cooldown.is_file(),
        "usage-limit cooldown should be persisted for later preflight routing"
    );

    let preflight = post_fugu_message(&url, "hello after cooldown").await;
    assert_eq!(preflight["model"], "claude-sonnet-5");

    server.abort();
}

#[tokio::test]
async fn sakana_auth_failure_failsover_to_subscription_and_cools_down_fugu() {
    let root = tempfile::tempdir().expect("auth failover routing fixture");
    let (url, server) = spawn_subscription_failover_bridge(root.path(), "AUTH_FAILOVER_OK").await;

    let response = post_fugu_message(&url, "AUTH_FAIL_ALWAYS").await;
    assert_eq!(response["model"], "claude-sonnet-5");
    assert_eq!(response_text(&response), "AUTH_FAILOVER_OK");

    let cooldown = root
        .path()
        .join(".cache/claudex/provider-auth-cooldown.json");
    assert!(
        cooldown.is_file(),
        "provider auth cooldown should be persisted for later preflight routing"
    );

    let preflight = post_fugu_message(&url, "hello after auth cooldown").await;
    assert_eq!(preflight["model"], "claude-sonnet-5");

    server.abort();
}

async fn parallel_request(url: &str, model: &str, index: usize) -> Value {
    Client::new()
        .post(url)
        .json(&json!({
            "model":model,
            "max_tokens":128,
            "metadata":{"user_id":format!("parallel-{index}")},
            "messages":[{"role":"user","content":format!("request {index}")}]
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
