use claudex_agent_adapter::agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute};
use serde_json::{Value, json};

#[tokio::test]
async fn configured_acp_routes_dynamic_models_and_expands_arguments() {
    assert!(
        AgentBackend::spawn(BackendKind::ConfiguredAcp, "missing-launch")
            .await
            .is_err()
    );
    let root = tempfile::tempdir().expect("configured ACP fixture");
    std::env::set_current_dir(root.path()).expect("isolate ACP trace");
    let route = BackendRoute {
        model: "vendor-default".to_owned(),
        backend: BackendKind::ConfiguredAcp,
        model_prefixes: vec!["vendor-".to_owned()],
        acp: Some(AcpLaunch {
            program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
            arguments: vec!["--model".to_owned(), "{model}".to_owned()],
        }),
    };
    let backend = AgentBackend::spawn_routes(&[route]);
    let response = backend
        .request("thread/start", json!({"model":"vendor-next"}))
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

    let first = std::fs::read_to_string(root.path().join("grok-acp-mock.jsonl"))
        .expect("configured ACP trace")
        .lines()
        .next()
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .expect("argument trace");
    assert_eq!(first["arguments"], json!(["--model", "vendor-next"]));

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
}
