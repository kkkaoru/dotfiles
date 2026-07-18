use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use claudex_agent_adapter::{
    agent_backend::{AgentBackend, BackendKind, BackendRoute},
    anthropic::Bridge,
    http_router,
};
use serde_json::json;

#[tokio::test]
async fn lazy_routes_cover_provider_entry_points_and_failed_startup_state() {
    let (home, codex_spawns) = provider_home();

    let backend = AgentBackend::spawn_routes(&[
        route("gpt-model", BackendKind::CodexAppServer),
        route("gpt-secondary", BackendKind::CodexAppServer),
        route("gpt-unused", BackendKind::CodexAppServer),
        route("grok-model", BackendKind::GrokAcp),
    ]);
    exercise_initial_provider_routes(&backend, &codex_spawns).await;

    assert!(backend.supports_model("gpt-5.6-sol"));
    assert!(backend.supports_model("grok-4.5"));
    assert!(!backend.supports_model("claude-opus-5"));
    exercise_explicit_subagent_routes(Arc::clone(&backend)).await;
    assert_eq!(
        backend.started_models(),
        [
            "gpt-model",
            "gpt-secondary",
            "grok-model",
            "gpt-5.6-sol",
            "grok-4.5"
        ]
    );
    assert_single_codex_spawn(&codex_spawns);

    exercise_dynamic_route().await;
    exercise_failed_route_health().await;
    drop(home);
}

fn provider_home() -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().expect("create provider entry-point home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(home.path().join(".codex/auth.json"), "{}").expect("write Codex auth");
    let codex_spawns = home.path().join("codex-spawns");
    let codex_wrapper = home.path().join("codex-wrapper");
    fs::write(
        &codex_wrapper,
        format!(
            "#!/bin/sh\nprintf 'spawn\\n' >> \"$HOME/codex-spawns\"\nexec \"{}\" \"$@\"\n",
            env!("CARGO_BIN_EXE_codex-mock")
        ),
    )
    .expect("write Codex spawn-counting wrapper");
    fs::set_permissions(&codex_wrapper, fs::Permissions::from_mode(0o755))
        .expect("make Codex wrapper executable");
    // This binary contains one current-thread test, so provider overrides cannot race.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("CLAUDEX_CODEX_PROGRAM", &codex_wrapper);
        std::env::set_var("CLAUDEX_GROK_PROGRAM", env!("CARGO_BIN_EXE_grok-acp-mock"));
    }
    std::env::set_current_dir(home.path()).expect("isolate Grok ACP trace output");
    (home, codex_spawns)
}

async fn exercise_initial_provider_routes(backend: &Arc<AgentBackend>, codex_spawns: &Path) {
    assert!(backend.started_models().is_empty());
    let first_codex = backend.request("thread/start", json!({"model":"gpt-model"}));
    let second_codex = backend.request("thread/start", json!({"model":"gpt-secondary"}));
    let (codex, secondary_codex) = tokio::join!(first_codex, second_codex);
    let codex = codex.expect("start first lazy Codex route");
    let secondary_codex = secondary_codex.expect("start second lazy Codex route");
    let grok = backend
        .request("thread/start", json!({"model":"grok-model"}))
        .await
        .expect("start lazy Grok route");
    assert!(codex.pointer("/thread/id").is_some());
    assert!(secondary_codex.pointer("/thread/id").is_some());
    assert!(grok.pointer("/thread/id").is_some());
    assert_eq!(
        fs::read_to_string(codex_spawns).expect("read Codex spawn count"),
        "spawn\n"
    );
    assert_eq!(
        backend.started_models(),
        ["gpt-model", "gpt-secondary", "grok-model"]
    );
    assert!(backend.is_alive());
}

fn assert_single_codex_spawn(codex_spawns: &Path) {
    assert_eq!(
        fs::read_to_string(codex_spawns).expect("re-read Codex spawn count"),
        "spawn\n",
        "dynamic GPT routes must reuse the initialized Codex provider"
    );
}

async fn exercise_dynamic_route() {
    let dynamic_only = AgentBackend::spawn_routes(&[route("grok-only", BackendKind::GrokAcp)]);
    dynamic_only
        .request("thread/start", json!({"model":"gpt-dynamic-only"}))
        .await
        .expect("start an inferred Codex route without a configured Codex route");
    dynamic_only
        .respond(json!(999), json!({}))
        .await
        .expect("find the dynamically started Codex backend");
}

async fn exercise_failed_route_health() {
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

async fn exercise_explicit_subagent_routes(backend: Arc<AgentBackend>) {
    let bridge = Arc::new(Bridge::new_with_backend(backend, "gpt-model".to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "gpt-model".to_owned(), None))
            .await
            .unwrap();
    });
    for (suffix, selected, expected) in [
        ("GPT", "gpt-5.6-sol", "medium"),
        ("GROK", "grok-4.5", "GROK_ACP_STREAM_OK"),
    ] {
        let user_id = format!("explicit-{suffix}");
        let prompt = launch_agent(&url, &user_id, suffix, selected).await;
        let response = post(
            &url,
            json!({
                "model":"claude-sonnet-5", "system":"cc_is_subagent=true",
                "metadata":{"user_id":user_id},
                "messages":[{"role":"user","content":prompt}]
            }),
        )
        .await;
        assert_eq!(response.pointer("/content/0/text"), Some(&json!(expected)));
        assert_eq!(response["model"], selected);
    }
    exercise_model_specific_tool_round_trip(&url).await;
    server.abort();
}

async fn exercise_model_specific_tool_round_trip(url: &str) {
    let user_id = "explicit-gpt-tool";
    let prompt = launch_agent(url, user_id, "GPT_TOOL", "gpt-5.6-sol").await;
    let tools = json!([{
        "name":"lookup", "input_schema":{"type":"object","properties":{
            "key":{"type":"string"}
        }}
    }]);
    let first = post(
        url,
        json!({
            "model":"claude-sonnet-5", "system":"cc_is_subagent=true",
            "metadata":{"user_id":user_id}, "tools":tools,
            "messages":[{"role":"user","content":prompt}]
        }),
    )
    .await;
    assert_eq!(first["model"], "gpt-5.6-sol");
    assert_eq!(first["stop_reason"], "tool_use");
    let second = post(
        url,
        json!({
            "model":"claude-sonnet-5", "system":"cc_is_subagent=true",
            "metadata":{"user_id":user_id}, "tools":tools,
            "messages":[
                {"role":"user","content":prompt},
                {"role":"assistant","content":first["content"]},
                {"role":"user","content":[{
                    "type":"tool_result", "tool_use_id":first["content"][0]["id"],
                    "content":"MODEL_ROUTE_OK"
                }]}
            ]
        }),
    )
    .await;
    assert_eq!(second["model"], "gpt-5.6-sol");
    assert_eq!(second["content"][0]["text"], "MODEL_ROUTE_OK");
}

async fn launch_agent(url: &str, user_id: &str, suffix: &str, selected: &str) -> String {
    let response = post(
        url,
        json!({
            "model":"gpt-model", "system":"provider routing test",
            "metadata":{"user_id":user_id},
            "tools":[{"name":"Agent","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":
                format!("USE_AGENT_MODEL_{suffix} {selected}")}]
        }),
    )
    .await;
    assert!(response.pointer("/content/0/input/model").is_none());
    assert!(response.pointer("/content/0/input/claudex_model").is_none());
    response["content"][0]["input"]["prompt"]
        .as_str()
        .expect("correlated Agent prompt")
        .to_owned()
}

async fn post(url: &str, body: serde_json::Value) -> serde_json::Value {
    reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await
        .expect("send adapter request")
        .error_for_status()
        .expect("successful adapter response")
        .json()
        .await
        .expect("decode adapter response")
}

fn route(model: &str, backend: BackendKind) -> BackendRoute {
    BackendRoute {
        model: model.to_owned(),
        backend,
    }
}
