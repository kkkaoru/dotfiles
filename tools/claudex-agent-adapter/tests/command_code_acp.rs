use std::{sync::Arc, time::Duration};

use claudex_agent_adapter::{
    agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute, WebSearchMode},
    anthropic::Bridge,
    http_router,
};
use reqwest::Client;
use serde_json::{Value, json};

const ACP_TIMEOUT: Duration = Duration::from_secs(10);

fn command_code_acp_program() -> String {
    std::env::var("COMMAND_CODE_ACP_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_command-code-acp").to_owned())
}

#[test]
fn providers_json_registers_command_code_without_auto_selecting_it() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools")
        .parent()
        .expect("repository");
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(repository.join(".config/claudex/providers.json"))
            .expect("providers.json"),
    )
    .expect("valid providers.json");
    let provider = config["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["id"] == "command-code")
        .expect("command-code provider");
    assert_eq!(provider["backend"], "configured-acp");
    assert_eq!(provider["webSearchMode"], "acp-native");
    assert_eq!(provider["agent"], "claudex-command-code");
    assert_eq!(provider["defaultModel"], "meta/muse-spark-1.2-contributor");
    assert_eq!(provider["acp"]["program"], "command-code-acp");
    assert_eq!(
        provider["acp"]["arguments"],
        json!(["--model", "{model}", "--effort", "{effort}"])
    );
    let main = config["mainProviders"]
        .as_array()
        .expect("mainProviders")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        !main.contains(&"command-code"),
        "command-code must stay out of automatic mainProviders so existing workers are unchanged"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_acp_headless_turn_returns_command_code_output() {
    let model = "meta/muse-spark-1.2-contributor";
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: model.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        effort: Some("high".to_owned()),
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency: Some(1),
        model_prefixes: vec!["meta/muse-spark".to_owned()],
        acp: Some(AcpLaunch {
            program: command_code_acp_program(),
            arguments: vec![
                "--model".to_owned(),
                "{model}".to_owned(),
                "--effort".to_owned(),
                "{effort}".to_owned(),
                "--cmd".to_owned(),
                env!("CARGO_BIN_EXE_command-code-cmd-mock").to_owned(),
            ],
        }),
        web_search_mode: WebSearchMode::Disabled,
    }]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, model.to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind command-code adapter");
    let url = format!(
        "http://{}/v1/messages",
        listener.local_addr().expect("adapter address")
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, model.to_owned(), None))
            .await
            .expect("serve command-code adapter");
    });

    let response = tokio::time::timeout(
        ACP_TIMEOUT,
        Client::new()
            .post(&url)
            .json(&json!({
                "model": model,
                "max_tokens": 128,
                "stream": false,
                "system": "cc_is_subagent=true\n<claudex-agent-id>toolu_command_code</claudex-agent-id>",
                "messages":[{"role":"user","content":"COMMAND_CODE_HEADLESS_OK"}]
            }))
            .send(),
    )
    .await
    .expect("command-code turn timed out")
    .expect("send command-code turn")
    .error_for_status()
    .expect("command-code status")
    .json::<Value>()
    .await
    .expect("decode command-code turn");
    let text = response["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    assert!(
        text.contains("COMMAND_CODE_HEADLESS_OK"),
        "unexpected command-code response: {response}"
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_acp_disconnect_kills_slow_cmd_within_two_seconds() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::TempDir::new().expect("slow cmd dir");
    let program = root.path().join("cmd");
    std::fs::write(
        &program,
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t1\",\"toolName\":\"read_file\",\"description\":\"waiting\"}}'\nexec sleep 30\n",
    )
    .expect("write slow cmd");
    let mut permissions = std::fs::metadata(&program)
        .expect("slow cmd metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("chmod slow cmd");

    let model = "meta/muse-spark-1.2-contributor";
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: model.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        effort: Some("high".to_owned()),
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency: Some(1),
        model_prefixes: vec!["meta/muse-spark".to_owned()],
        acp: Some(AcpLaunch {
            program: command_code_acp_program(),
            arguments: vec![
                "--model".to_owned(),
                "{model}".to_owned(),
                "--effort".to_owned(),
                "{effort}".to_owned(),
                "--cmd".to_owned(),
                program.display().to_string(),
            ],
        }),
        web_search_mode: WebSearchMode::Disabled,
    }]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, model.to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind slow command-code adapter");
    let address = listener.local_addr().expect("adapter address");
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, model.to_owned(), None))
            .await
            .expect("serve slow command-code adapter");
    });
    let client = Client::new();
    let url = format!("http://{address}/v1/messages");
    let health_url = format!("http://{address}/health");
    let response = client
        .post(&url)
        .json(&json!({
            "model": model,
            "max_tokens": 128,
            "stream": true,
            "system": "cc_is_subagent=true\n<claudex-agent-id>toolu_command_code_slow</claudex-agent-id>",
            "messages":[{"role":"user","content":"SLOW_TURN"}]
        }))
        .send()
        .await
        .expect("start slow command-code stream");
    tokio::time::timeout(ACP_TIMEOUT, async {
        loop {
            let health = client
                .get(&health_url)
                .send()
                .await
                .expect("slow turn health")
                .json::<Value>()
                .await
                .expect("decode slow turn health");
            if health["active_provider_turns"].as_u64() == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("slow command-code turn should start");
    drop(response);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let health = client
                .get(&health_url)
                .send()
                .await
                .expect("cancel health")
                .json::<Value>()
                .await
                .expect("decode cancel health");
            if health["active_provider_turns"].as_u64() == Some(0) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("disconnect cancel should settle within 2s");
    server.abort();
}
