use std::sync::Arc;

use claudex_agent_adapter::{
    agent_backend::AgentBackend,
    anthropic::Bridge,
    app_server::AppServer,
    http_router,
    provider_config::{ModelCatalog, WorkerRoute},
};
use reqwest::Client;
use serde_json::json;

#[tokio::test]
async fn ccr_search_returns_results_from_a_configured_worker_and_filters_domains() {
    let root = tempfile::tempdir().expect("search fixture");
    let source = root.path().join("codex-source");
    std::fs::create_dir(&source).expect("create search source");
    std::fs::write(source.join("auth.json"), "{}").expect("write search auth");
    let app = AppServer::spawn_with_program(
        "search-model",
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &root.path().join("codex-home"),
    )
    .await
    .expect("start search worker");
    let backend = AgentBackend::codex(app);
    let catalog = ModelCatalog::default()
        .with_search_worker_routes(vec![WorkerRoute::new(
            "search-worker".to_owned(),
            "search-model".to_owned(),
            "high".to_owned(),
        )])
        .expect("configure search worker");
    let bridge = Arc::new(
        Bridge::new_with_backend(backend, "search-model".to_owned()).with_model_catalog(catalog),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind search listener");
    let address = listener.local_addr().expect("search listener address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "search-model".to_owned(), None),
        )
        .await
        .expect("serve search adapter");
    });

    let client = Client::new();
    let response = client
        .post(format!(
            "http://{address}/v1/code/sessions/session_search/worker/web-search"
        ))
        .json(&json!({
            "query":"WEBSEARCH_QUERY",
            "allowed_domains":["example.com"],
            "blocked_domains":["blocked.example.com"]
        }))
        .send()
        .await
        .expect("request search");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("decode search response");
    assert_eq!(body["error"], serde_json::Value::Null);
    assert_eq!(body["results"].as_array().unwrap().len(), 1);
    assert_eq!(body["results"][0]["url"], "https://example.com/source");

    let empty = client
        .post(format!(
            "http://{address}/v1/code/sessions/session_search/worker/web-search"
        ))
        .json(&json!({"query":"WEBSEARCH_EMPTY"}))
        .send()
        .await
        .expect("request empty search");
    assert_eq!(empty.status(), reqwest::StatusCode::OK);
    let empty_body: serde_json::Value = empty.json().await.expect("decode empty search");
    assert!(empty_body["results"].as_array().unwrap().is_empty());
    assert!(
        empty_body["error"].as_str().is_some_and(|error| !error.is_empty()),
        "{empty_body}"
    );

    let blocked = client
        .post(format!(
            "http://{address}/v1/code/sessions/session_search/worker/web-search"
        ))
        .json(&json!({
            "query":"WEBSEARCH_QUERY",
            "blocked_domains":["example.com"]
        }))
        .send()
        .await
        .expect("request blocked search");
    let blocked_body: serde_json::Value = blocked.json().await.expect("decode blocked search");
    assert!(blocked_body["results"].as_array().unwrap().is_empty());
    server.abort();
}
