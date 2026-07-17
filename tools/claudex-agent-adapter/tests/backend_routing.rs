use std::sync::Arc;

use claudex_agent_adapter::{
    agent_backend::AgentBackend, anthropic::Bridge, app_server::AppServer, grok_acp::GrokAcp,
    http_router,
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

    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..40 {
        let url = url.clone();
        tasks.spawn(async move {
            let (model, expected) = route_case(index);
            let response = parallel_request(&url, model, index).await;
            assert_eq!(response_text(&response), expected);
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("parallel request task");
    }
    server.abort();
}

fn route_case(index: usize) -> (&'static str, &'static str) {
    if index % 2 == 0 {
        ("gpt-model", "CODEX_ROUTED_OK")
    } else {
        ("grok-model", "GROK_ACP_STREAM_OK")
    }
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
