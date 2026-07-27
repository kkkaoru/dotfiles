use std::time::Duration;

use claudex_agent_adapter::agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute};
use serde_json::{Value, json};

const ACP_EVENT_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn configured_acp_routes_dynamic_models_and_expands_arguments() {
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
