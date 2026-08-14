use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
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
use tokio::task::JoinHandle;

#[path = "support/coverage_profile.rs"]
mod coverage_profile;

const ACP_TIMEOUT: Duration = Duration::from_secs(10);
const MODEL: &str = "meta/muse-spark-1.2-contributor";
const CONCURRENCY_WAIT_TIMEOUT_ENV: &str = "CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS";

/// Hold a longer model-admission wait for queueing integration coverage.
/// Default production wait is 1s (fast SubAgent failover); this scenario
/// intentionally parks a third turn behind two held slots.
struct ConcurrencyWaitTimeoutGuard {
    previous: Option<String>,
}

impl ConcurrencyWaitTimeoutGuard {
    fn set_ms(ms: u64) -> Self {
        let previous = std::env::var(CONCURRENCY_WAIT_TIMEOUT_ENV).ok();
        // SAFETY: integration binary; restored on Drop. Other tests here do not
        // assert the default 1s admission timeout.
        unsafe {
            std::env::set_var(CONCURRENCY_WAIT_TIMEOUT_ENV, ms.to_string());
        }
        Self { previous }
    }
}

impl Drop for ConcurrencyWaitTimeoutGuard {
    fn drop(&mut self) {
        // SAFETY: paired with set_ms; restores prior process env.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(CONCURRENCY_WAIT_TIMEOUT_ENV, value),
                None => std::env::remove_var(CONCURRENCY_WAIT_TIMEOUT_ENV),
            }
        }
    }
}

fn command_code_acp_program() -> String {
    std::env::var("COMMAND_CODE_ACP_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_command-code-acp").to_owned())
}

fn mock_cmd_program() -> String {
    env!("CARGO_BIN_EXE_command-code-cmd-mock").to_owned()
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools")
        .parent()
        .expect("repository")
        .to_owned()
}

fn wrap_mock(root: &Path, trace: &Path, extra_env: &[(&str, &str)]) -> PathBuf {
    let wrapper = root.join("cmd");
    let mut script = format!(
        "#!/bin/sh\nexport COMMAND_CODE_CMD_MOCK_TRACE='{}'\n",
        trace.display()
    );
    for (key, value) in extra_env {
        script.push_str(&format!("export {key}='{value}'\n"));
    }
    script.push_str(&format!(
        "exec '{}' \"$@\"\n",
        coverage_profile::wrapped_program_string(root, env!("CARGO_BIN_EXE_command-code-cmd-mock"))
    ));
    fs::write(&wrapper, script).expect("write command-code mock wrapper");
    let mut permissions = fs::metadata(&wrapper)
        .expect("wrapper metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).expect("chmod wrapper");
    wrapper
}

struct CommandCodeAdapter {
    url: String,
    health_url: String,
    server: JoinHandle<()>,
}

impl Drop for CommandCodeAdapter {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn spawn_command_code_adapter(
    cmd_program: String,
    max_concurrency: Option<usize>,
) -> CommandCodeAdapter {
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: MODEL.to_owned(),
        backend: BackendKind::ConfiguredAcp,
        effort: Some("high".to_owned()),
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency,
        model_prefixes: vec!["meta/muse-spark".to_owned()],
        acp: Some(AcpLaunch {
            program: command_code_acp_program(),
            arguments: vec![
                "--model".to_owned(),
                "{model}".to_owned(),
                "--effort".to_owned(),
                "{effort}".to_owned(),
                "--cmd".to_owned(),
                cmd_program,
            ],
        }),
        web_search_mode: WebSearchMode::AcpNative,
    }]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, MODEL.to_owned()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind command-code adapter");
    let address = listener.local_addr().expect("adapter address");
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, MODEL.to_owned(), None))
            .await
            .expect("serve command-code adapter");
    });
    CommandCodeAdapter {
        url: format!("http://{address}/v1/messages"),
        health_url: format!("http://{address}/health"),
        server,
    }
}

fn subagent_request(content: &str, stream: bool) -> Value {
    json!({
        "model": MODEL,
        "max_tokens": 256,
        "stream": stream,
        "system": "cc_is_subagent=true\n<claudex-agent-id>toolu_command_code</claudex-agent-id>",
        "messages":[{"role":"user","content": content}]
    })
}

fn response_text(response: &Value) -> String {
    response["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

async fn read_until_contains(
    response: &mut Response,
    stream: &mut String,
    expected: &str,
    early_end_message: &str,
) {
    while !stream.contains(expected) {
        let chunk = response
            .chunk()
            .await
            .expect("read command-code stream")
            .expect(early_end_message);
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
}

fn traced_args(trace: &Path) -> String {
    fs::read_to_string(trace).unwrap_or_default()
}

#[test]
fn providers_json_registers_command_code_for_automatic_selection() {
    let config: Value = serde_json::from_str(
        &fs::read_to_string(repository_root().join(".config/claudex/providers.json"))
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
    assert_eq!(provider["maxConcurrency"], 2);
    assert_eq!(
        provider["agent"],
        "claudex-command-code-muse-spark-1-2-contributor"
    );
    assert!(
        provider["agent"]
            .as_str()
            .is_some_and(|agent| agent.contains("muse-spark-1-2") && agent.contains("contributor")),
        "slug must carry Muse Spark 1.2 contributor: {}",
        provider["agent"]
    );
    assert_eq!(provider["defaultModel"], MODEL);
    assert_eq!(provider["subagentModel"], MODEL);
    assert_eq!(provider["usageProvider"], "commandcode");
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
        main.contains(&"command-code"),
        "command-code must be an automatic mainProviders candidate: {main:?}"
    );
    let definition = fs::read_to_string(
        repository_root().join(".claude/agents/claudex-command-code-muse-spark-1-2-contributor.md"),
    )
    .expect("command-code agent definition");
    assert!(definition.contains("name: claudex-command-code-muse-spark-1-2-contributor"));
    assert!(definition.contains(&format!("model: {MODEL}")));
    assert!(definition.contains("complete tool set and permission context"));
    assert!(!definition.contains("\ntools:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_acp_headless_turn_returns_command_code_output() {
    let adapter = spawn_command_code_adapter(mock_cmd_program(), Some(1)).await;
    let response = tokio::time::timeout(
        ACP_TIMEOUT,
        Client::new()
            .post(&adapter.url)
            .json(&subagent_request("COMMAND_CODE_HEADLESS_OK", false))
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
    assert_eq!(response["stop_reason"], "end_turn");
    assert!(
        response["content"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|block| block["type"] != "tool_use"),
        "native cmd tools must stay display-only: {response}"
    );
    let text = response_text(&response);
    assert!(
        text.contains("COMMAND_CODE_HEADLESS_OK"),
        "unexpected command-code response: {response}"
    );
    assert!(
        !text.contains('▶'),
        "tool chrome must not remain in committed text: {response}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamed_turn_paints_shared_read_chrome_before_end_turn() {
    let adapter = spawn_command_code_adapter(mock_cmd_program(), Some(1)).await;
    let started = Instant::now();
    let mut response = Client::new()
        .post(&adapter.url)
        .json(&subagent_request("STREAM_DELAY", true))
        .send()
        .await
        .expect("start streamed command-code turn");
    let mut stream = String::new();
    read_until_contains(
        &mut response,
        &mut stream,
        "LIVE_DELTA_BEFORE_SLEEP",
        "stream ended before live chrome",
    )
    .await;
    let first_text_at = started.elapsed();
    assert!(
        !stream.contains("STREAM_DELAY_OK") && !stream.contains("event: message_stop"),
        "text_delta was buffered until cmd exit ({first_text_at:?}): {stream}"
    );
    assert!(
        stream.contains("▶") || stream.contains("read_file") || stream.contains("README.md"),
        "shared ACP tool chrome missing from live stream: {stream}"
    );
    assert!(
        !stream.contains("\"type\":\"tool_use\""),
        "live ▶ must not become executable tool_use: {stream}"
    );
    while let Some(chunk) = response.chunk().await.expect("drain stream") {
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(stream.contains("STREAM_DELAY_OK") || stream.contains("MORE_AFTER_SLEEP"));
    assert!(stream.contains("event: message_stop"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_search_uses_shared_query_argument_chrome() {
    let adapter = spawn_command_code_adapter(mock_cmd_program(), Some(1)).await;
    let mut response = Client::new()
        .post(&adapter.url)
        .json(&subagent_request("WEB_SEARCH_AVITA", true))
        .send()
        .await
        .expect("start web_search turn");
    let mut stream = String::new();
    read_until_contains(
        &mut response,
        &mut stream,
        "WEB_SEARCH_OK",
        "stream ended before web_search answer",
    )
    .await;
    while let Some(chunk) = response.chunk().await.expect("drain web_search stream") {
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(
        stream.contains("web_search") && stream.contains("AVITA株式会社"),
        "web_search query must reach the SubAgent stream for TUI cards: {stream}"
    );
    assert!(!stream.contains("ツール結果待ち"));
    assert!(!stream.contains("続きの調査または回答"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canned_japanese_status_is_dropped_from_the_http_bridge() {
    let adapter = spawn_command_code_adapter(mock_cmd_program(), Some(1)).await;
    let response = Client::new()
        .post(&adapter.url)
        .json(&subagent_request("CANNED_STATUS", false))
        .send()
        .await
        .expect("send canned status turn")
        .error_for_status()
        .expect("canned status status")
        .json::<Value>()
        .await
        .expect("decode canned status turn");
    let text = response_text(&response);
    assert!(
        text.contains("CANNED_STATUS_OK") || text.contains("AVITA findings"),
        "{text}"
    );
    assert!(!text.contains("ツール結果待ち"));
    assert!(!text.contains("続きの調査または回答"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_argv_omits_effort_and_always_disables_skills() {
    let root = tempfile::TempDir::new().expect("argv fixture");
    let trace = root.path().join("trace.jsonl");
    let wrapper = wrap_mock(root.path(), &trace, &[]);
    let adapter = spawn_command_code_adapter(wrapper.display().to_string(), Some(1)).await;
    Client::new()
        .post(&adapter.url)
        .json(&subagent_request("COMMAND_CODE_HEADLESS_OK", false))
        .send()
        .await
        .expect("send argv turn")
        .error_for_status()
        .expect("argv turn status");
    let recorded = traced_args(&trace);
    assert!(
        recorded.contains("--no-skills"),
        "skills dump must stay off Muse Spark: {recorded}"
    );
    assert!(
        recorded.contains("--no-session"),
        "one-shot SubAgent turns must not resume project sessions: {recorded}"
    );
    assert!(
        !recorded.contains("--effort"),
        "Muse Spark rejects --effort; keep it ACP-side only: {recorded}"
    );
    assert!(recorded.contains(MODEL), "{recorded}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slim_prompt_drops_routing_dump_before_cmd() {
    let root = tempfile::TempDir::new().expect("slim prompt fixture");
    let trace = root.path().join("trace.jsonl");
    let wrapper = wrap_mock(root.path(), &trace, &[]);
    let adapter = spawn_command_code_adapter(wrapper.display().to_string(), Some(1)).await;
    Client::new()
        .post(&adapter.url)
        .json(&json!({
            "model": MODEL,
            "max_tokens": 128,
            "stream": false,
            "system": concat!(
                "cc_is_subagent=true\n",
                "<system-reminder>\n",
                "Claudex routing data (runtime metadata; values only):\n",
                "{\"selected_workers\":[{\"agent\":\"claudex-gpt\"}]}\n",
                "</system-reminder>\n",
                "claudex_effort: high\n",
                "You are the model inside Claudex on a provider-native ACP backend."
            ),
            "messages":[{"role":"user","content":"Read CLAUDE.md and return the first heading."}]
        }))
        .send()
        .await
        .expect("send slim prompt turn")
        .error_for_status()
        .expect("slim prompt status");
    let recorded = traced_args(&trace);
    assert!(
        recorded.contains("Read CLAUDE.md"),
        "delegated task missing from cmd argv: {recorded}"
    );
    assert!(
        !recorded.contains("selected_workers"),
        "routing dump must not reach cmd: {recorded}"
    );
    assert!(!recorded.contains("claudex_effort"));
    assert!(
        recorded.contains("native thinking/? elapsed and web cards")
            || recorded.contains("Do not greet")
    );
    assert!(!recorded.contains("▶ name: query/path/url"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_failure_is_visible_on_the_http_bridge() {
    let root = tempfile::TempDir::new().expect("auth fixture");
    let trace = root.path().join("trace.jsonl");
    let wrapper = wrap_mock(
        root.path(),
        &trace,
        &[("COMMAND_CODE_CMD_MOCK_MODE", "auth-fail")],
    );
    let adapter = spawn_command_code_adapter(wrapper.display().to_string(), Some(1)).await;
    let response = Client::new()
        .post(&adapter.url)
        .json(&subagent_request("AUTH_PROBE", false))
        .send()
        .await
        .expect("send auth failure turn");
    let status = response.status();
    let body = response.text().await.expect("auth failure body");
    assert!(
        !status.is_success() || body.contains("not authenticated") || body.contains("Command Code"),
        "auth failure should surface: status={status} body={body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines, clippy::excessive_nesting)]
async fn max_concurrency_two_queues_the_third_command_code_turn() {
    let _wait =
        ConcurrencyWaitTimeoutGuard::set_ms(ACP_TIMEOUT.as_millis().try_into().unwrap_or(10_000));
    let root = tempfile::TempDir::new().expect("concurrency fixture");
    let trace = root.path().join("trace.jsonl");
    let release = root.path().join("release");
    let release_path = release.display().to_string();
    let wrapper = wrap_mock(
        root.path(),
        &trace,
        &[
            ("COMMAND_CODE_CMD_MOCK_MODE", "wait-release"),
            ("COMMAND_CODE_CMD_MOCK_RELEASE", &release_path),
        ],
    );
    let adapter = spawn_command_code_adapter(wrapper.display().to_string(), Some(2)).await;
    let client = Client::new();
    let mut requests = Vec::new();
    for index in 0..3 {
        let client = client.clone();
        let url = adapter.url.clone();
        requests.push(tokio::spawn(async move {
            let response = client
                .post(url)
                .json(&json!({
                    "model": MODEL,
                    "max_tokens": 128,
                    "stream": false,
                    "messages":[{"role":"user","content": format!("WAIT_RELEASE {index}")}]
                }))
                .send()
                .await
                .expect("send concurrent command-code turn");
            let status = response.status();
            let body = response
                .json::<Value>()
                .await
                .expect("decode concurrent turn");
            assert!(
                status.is_success(),
                "concurrent turn status {status}: {body}"
            );
            body
        }));
    }

    let mut last_health = Value::Null;
    let saturated = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            last_health = client
                .get(&adapter.health_url)
                .send()
                .await
                .expect("concurrency health")
                .json::<Value>()
                .await
                .expect("decode concurrency health");
            let status = &last_health["model_concurrency"][MODEL];
            if status["active"] == 2 && status["queued"] == 1 {
                break last_health.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("two Command Code turns should saturate maxConcurrency=2: {last_health}")
    });
    assert_eq!(
        saturated["model_concurrency"][MODEL],
        json!({"active":2,"limit":2,"available":false,"queued":1})
    );

    fs::write(&release, b"go").expect("release waiting cmd processes");
    for request in requests {
        let response = tokio::time::timeout(ACP_TIMEOUT, request)
            .await
            .expect("concurrent turn timed out")
            .expect("concurrent task failed");
        assert!(
            response_text(&response).contains("WAIT_RELEASE_OK"),
            "{response}"
        );
    }
}

async fn wait_for_active_provider_turns(client: &Client, health_url: &str, expected: u64) {
    loop {
        let health = client
            .get(health_url)
            .send()
            .await
            .expect("provider-turn health")
            .json::<Value>()
            .await
            .expect("decode provider-turn health");
        if health["active_provider_turns"].as_u64() == Some(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_acp_disconnect_kills_slow_cmd_within_two_seconds() {
    let root = tempfile::TempDir::new().expect("slow cmd dir");
    let program = root.path().join("cmd");
    fs::write(
        &program,
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t1\",\"toolName\":\"read_file\",\"description\":\"waiting\"}}'\nexec sleep 30\n",
    )
    .expect("write slow cmd");
    let mut permissions = fs::metadata(&program)
        .expect("slow cmd metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&program, permissions).expect("chmod slow cmd");

    let adapter = spawn_command_code_adapter(program.display().to_string(), Some(1)).await;
    let client = Client::new();
    let response = client
        .post(&adapter.url)
        .json(&subagent_request("SLOW_TURN", true))
        .send()
        .await
        .expect("start slow command-code stream");
    tokio::time::timeout(
        ACP_TIMEOUT,
        wait_for_active_provider_turns(&client, &adapter.health_url, 1),
    )
    .await
    .expect("slow command-code turn should start");
    drop(response);

    tokio::time::timeout(
        Duration::from_secs(2),
        wait_for_active_provider_turns(&client, &adapter.health_url, 0),
    )
    .await
    .expect("disconnect cancel should settle within 2s");
}
