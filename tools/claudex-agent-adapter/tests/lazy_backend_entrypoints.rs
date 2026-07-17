use std::{fs, sync::Arc};

use claudex_agent_adapter::{
    agent_backend::{AgentBackend, BackendKind, BackendRoute},
    anthropic::Bridge,
    http_router,
};
use serde_json::json;

#[tokio::test]
async fn lazy_routes_cover_provider_entry_points_and_failed_startup_state() {
    let home = tempfile::tempdir().expect("create provider entry-point home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(home.path().join(".codex/auth.json"), "{}").expect("write Codex auth");

    // This test binary contains one current-thread test, so no other thread can
    // read the process environment before these provider overrides are set.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var(
            "CLAUDEX_CODEX_PROGRAM",
            env!("CARGO_BIN_EXE_routing-codex-mock"),
        );
        std::env::set_var("CLAUDEX_GROK_PROGRAM", env!("CARGO_BIN_EXE_grok-acp-mock"));
    }
    std::env::set_current_dir(home.path()).expect("isolate Grok ACP trace output");

    let backend = AgentBackend::spawn_routes(&[
        route("gpt-model", BackendKind::CodexAppServer),
        route("grok-model", BackendKind::GrokAcp),
    ]);
    assert!(backend.started_models().is_empty());
    let codex = backend
        .request("thread/start", json!({"model":"gpt-model"}))
        .await
        .expect("start lazy Codex route");
    let grok = backend
        .request("thread/start", json!({"model":"grok-model"}))
        .await
        .expect("start lazy Grok route");
    assert!(codex.pointer("/thread/id").is_some());
    assert!(grok.pointer("/thread/id").is_some());
    assert_eq!(backend.started_models(), ["gpt-model", "grok-model"]);
    assert!(backend.is_alive());

    let failed = AgentBackend::spawn_routes(&[route("bad-version", BackendKind::GrokAcp)]);
    assert!(
        failed
            .request("thread/start", json!({"model":"bad-version"}))
            .await
            .is_err()
    );
    assert!(failed.started_models().is_empty());
    assert!(!failed.is_alive());

    let bridge = Arc::new(Bridge::new_with_backend(failed, "bad-version".to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "bad-version".to_owned(), None),
        )
        .await
        .unwrap();
    });
    let health = reqwest::get(format!("http://{address}/health"))
        .await
        .unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        health.json::<serde_json::Value>().await.unwrap()["status"],
        "unavailable"
    );
    server.abort();
}

fn route(model: &str, backend: BackendKind) -> BackendRoute {
    BackendRoute {
        model: model.to_owned(),
        backend,
    }
}
