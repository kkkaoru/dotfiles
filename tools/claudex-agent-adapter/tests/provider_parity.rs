//! Cross-provider contract coverage.
//!
//! These tests deliberately exercise the public HTTP bridge instead of comparing
//! provider fixture frames with one another. Each fixture speaks its native
//! protocol (Claude JSONL, ACP, command-code ACP, or Codex JSON-RPC); the
//! assertions are made only after Claudex has converted that protocol to the
//! Anthropic SSE contract.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use claudex_agent_adapter::{
    agent_backend::{AcpLaunch, AgentBackend},
    anthropic::Bridge,
    app_server::AppServer,
    grok_acp::GrokAcp,
    http_router,
};
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{sync::Mutex, task::JoinHandle};

const CLAUDE_MODEL: &str = "claude-haiku-4-5";
const GROK_MODEL: &str = "grok-parity-4.5";
const COMMAND_CODE_MODEL: &str = "meta/muse-spark-1.2-contributor";
const CODEX_MODEL: &str = "codex-parity-model";
const SESSION_ID: &str = "provider-parity-session";
const LOOKUP_TOOLS: &str = r#"[{"name":"lookup","description":"Look up a value","input_schema":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}}]"#;

static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct Endpoint {
    _root: TempDir,
    server: JoinHandle<()>,
    url: String,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Surface {
    ClaudeSubscription,
    ConfiguredGrok,
    CommandCode,
    CodexAppServer,
}

impl Surface {
    const ALL: [Self; 4] = [
        Self::ClaudeSubscription,
        Self::ConfiguredGrok,
        Self::CommandCode,
        Self::CodexAppServer,
    ];

    const fn model(self) -> &'static str {
        match self {
            Self::ClaudeSubscription => CLAUDE_MODEL,
            Self::ConfiguredGrok => GROK_MODEL,
            Self::CommandCode => COMMAND_CODE_MODEL,
            Self::CodexAppServer => CODEX_MODEL,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::ClaudeSubscription => "local Claude subscription",
            Self::ConfiguredGrok => "configured ACP/Grok",
            Self::CommandCode => "Command Code ACP",
            Self::CodexAppServer => "Codex app-server",
        }
    }

    const fn prompt(self) -> &'static str {
        match self {
            Self::ClaudeSubscription => "SUBSCRIPTION_STREAM_DELAY",
            Self::ConfiguredGrok => "provider parity progress",
            Self::CommandCode => "COMMAND_CODE_HEADLESS_OK",
            Self::CodexAppServer => "PROVIDER_TOOL_PROGRESS",
        }
    }

    const fn expected_text(self) -> &'static str {
        match self {
            Self::ClaudeSubscription => "STREAM_FIRSTSTREAM_SECOND",
            Self::ConfiguredGrok => "GROK_ACP_STREAM_OK",
            Self::CommandCode => "COMMAND_CODE_HEADLESS_OK",
            Self::CodexAppServer => "CODEX_PROVIDER_PROGRESS_OK",
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_canonical_provider_surfaces_obey_one_http_stream_contract() {
    for surface in Surface::ALL {
        let endpoint = spawn_surface(surface).await;
        let first = stream_turn(&endpoint, surface, surface.prompt(), SESSION_ID).await;
        assert_stream_contract(surface, &first, surface.expected_text());
        assert_native_progress(surface, &first);

        // A follow-up carries the previous assistant content and the same
        // transport identity. This is the public hook used by Claude Code to
        // recover the provider session instead of creating a fresh conversation.
        let first_text = streamed_text(&first);
        let follow_up = follow_up_request(surface, first_text);
        let second = expect_follow_up(&endpoint.url, follow_up, surface).await;
        assert_eq!(
            second.status(),
            StatusCode::OK,
            "{} follow-up status",
            surface.label()
        );
        let second_body = second
            .text()
            .await
            .unwrap_or_else(|error| panic!("{} follow-up body failed: {error}", surface.label()));
        let second_events = parse_sse(&second_body);
        assert_follow_up_contract(surface, &second_events);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_failures_have_one_terminal_error_shape_after_conversion() {
    // The native transports have different failure payloads, but the HTTP
    // surface must emit exactly one error and never duplicate its terminal
    // envelope. Keep the test on deterministic local fixtures; no provider
    // network or credentials are involved.
    for surface in [
        Surface::ClaudeSubscription,
        Surface::ConfiguredGrok,
        Surface::CommandCode,
        Surface::CodexAppServer,
    ] {
        let endpoint = spawn_failure_surface(surface).await;
        let body = error_request(surface);
        let response = send_stream(&endpoint.url, body, Some(SESSION_ID))
            .await
            .unwrap_or_else(|error| panic!("{} error request failed: {error}", surface.label()));
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|error| panic!("{} error body failed: {error}", surface.label()));
        let events = parse_sse(&text);
        let errors = events
            .iter()
            .filter(|event| event["type"] == "error")
            .count();
        assert_eq!(
            errors,
            1,
            "{} must expose exactly one terminal error: {text}",
            surface.label()
        );
        let message_stops = events
            .iter()
            .filter(|event| event["type"] == "message_stop")
            .count();
        assert!(
            message_stops <= 1,
            "{} error emitted duplicate message_stop frames: {text}",
            surface.label()
        );
        let stop_reason = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .and_then(|event| event.pointer("/delta/stop_reason"))
            .and_then(Value::as_str);
        assert_optional_error_stop_reason(stop_reason, status, surface);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_grok_stream_sends_one_acp_cancel_and_releases_the_turn() {
    let endpoint = spawn_grok_mode("cancellable-turns").await;
    let root = endpoint._root.path().to_owned();
    let mut response = Client::new()
        .post(&endpoint.url)
        .header("x-claude-code-session-id", SESSION_ID)
        .json(&request(
            Surface::ConfiguredGrok,
            "BLOCK UNTIL DISCONNECT",
            true,
        ))
        .send()
        .await
        .expect("start cancellable Grok stream");
    let first = tokio::time::timeout(Duration::from_secs(2), response.chunk())
        .await
        .expect("Grok did not produce an initial frame")
        .expect("read Grok initial frame")
        .expect("Grok stream ended before message_start");
    assert!(String::from_utf8_lossy(&first).contains("message_start"));
    wait_for_trace_event(&root.join("grok-acp-mock.jsonl"), "prompt_submitted").await;
    drop(response);
    tokio::time::timeout(
        Duration::from_secs(3),
        wait_for_trace_event(&root.join("grok-acp-mock.jsonl"), "cancel"),
    )
    .await
    .expect("Grok cancellation did not settle");

    let trace = read_trace(&root.join("grok-acp-mock.jsonl"));
    assert_eq!(
        trace
            .iter()
            .filter(|event| event.get("cancel").is_some())
            .count(),
        1
    );
}

async fn spawn_surface(surface: Surface) -> Endpoint {
    match surface {
        Surface::ClaudeSubscription => spawn_claude_subscription().await,
        Surface::ConfiguredGrok => spawn_grok_mode("coverage-updates").await,
        Surface::CommandCode => spawn_command_code().await,
        Surface::CodexAppServer => spawn_codex().await,
    }
}

async fn spawn_failure_surface(surface: Surface) -> Endpoint {
    match surface {
        Surface::ClaudeSubscription => spawn_claude_failure().await,
        Surface::ConfiguredGrok => spawn_grok_mode("fail-prompt").await,
        Surface::CommandCode => spawn_command_code_with_mode("boom").await,
        Surface::CodexAppServer => spawn_codex().await,
    }
}

async fn spawn_claude_failure() -> Endpoint {
    let root = tempfile::tempdir().expect("Claude subscription failure fixture root");
    let (source, isolated) = codex_homes(&root);
    let app = AppServer::spawn_with_program(
        CODEX_MODEL,
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &isolated,
    )
    .await
    .expect("spawn subscription failure support app-server");
    let program = root.path().join("claude-failing");
    fs::write(
        &program,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"fixture failure\"}'\nexit 7\n",
    )
    .expect("write Claude failure fixture");
    let mut permissions = fs::metadata(&program)
        .expect("read Claude failure fixture metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&program, permissions).expect("chmod Claude failure fixture");
    let bridge = Bridge::new_with_subscription_program(app, CLAUDE_MODEL.to_owned(), &program);
    endpoint(root, bridge, CLAUDE_MODEL).await
}

async fn spawn_claude_subscription() -> Endpoint {
    let root = tempfile::tempdir().expect("Claude subscription fixture root");
    let (source, isolated) = codex_homes(&root);
    let app = AppServer::spawn_with_program(
        CODEX_MODEL,
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &isolated,
    )
    .await
    .expect("spawn subscription support app-server");
    let bridge = Bridge::new_with_subscription_program(
        app,
        CLAUDE_MODEL.to_owned(),
        env!("CARGO_BIN_EXE_claude-mock"),
    );
    endpoint(root, bridge, CLAUDE_MODEL).await
}

async fn spawn_grok_mode(mode: &str) -> Endpoint {
    let root = tempfile::tempdir().expect("configured Grok fixture root");
    let launch = AcpLaunch {
        program: env!("CARGO_BIN_EXE_grok-acp-mock").to_owned(),
        arguments: vec!["--mode".to_owned(), mode.to_owned()],
    };
    let agent = spawn_configured(&root, GROK_MODEL, &launch).await;
    let bridge = Bridge::new_with_backend(
        AgentBackend::routed(vec![(
            GROK_MODEL.to_owned(),
            AgentBackend::configured_acp(agent),
        )]),
        GROK_MODEL.to_owned(),
    );
    endpoint(root, bridge, GROK_MODEL).await
}

async fn spawn_command_code() -> Endpoint {
    spawn_command_code_with_mode("").await
}

async fn spawn_command_code_with_mode(mode: &str) -> Endpoint {
    let root = tempfile::tempdir().expect("Command Code fixture root");
    let cmd = command_wrapper(&root, mode);
    let launch = AcpLaunch {
        program: env!("CARGO_BIN_EXE_command-code-acp").to_owned(),
        arguments: vec![
            "--model".to_owned(),
            "{model}".to_owned(),
            "--cmd".to_owned(),
            cmd.display().to_string(),
        ],
    };
    let agent = spawn_configured(&root, COMMAND_CODE_MODEL, &launch).await;
    let bridge = Bridge::new_with_backend(
        AgentBackend::routed(vec![(
            COMMAND_CODE_MODEL.to_owned(),
            AgentBackend::configured_acp(agent),
        )]),
        COMMAND_CODE_MODEL.to_owned(),
    );
    endpoint(root, bridge, COMMAND_CODE_MODEL).await
}

async fn spawn_codex() -> Endpoint {
    let root = tempfile::tempdir().expect("Codex fixture root");
    let (source, isolated) = codex_homes(&root);
    let app = AppServer::spawn_with_program(
        CODEX_MODEL,
        env!("CARGO_BIN_EXE_codex-mock"),
        &source,
        &isolated,
    )
    .await
    .expect("spawn Codex app-server fixture");
    let bridge = Bridge::new_with_backend(
        AgentBackend::routed(vec![(CODEX_MODEL.to_owned(), AgentBackend::codex(app))]),
        CODEX_MODEL.to_owned(),
    );
    endpoint(root, bridge, CODEX_MODEL).await
}

async fn spawn_configured(
    root: &TempDir,
    model: &str,
    launch: &AcpLaunch,
) -> std::sync::Arc<GrokAcp> {
    let lock = CWD_LOCK.get_or_init(|| Mutex::const_new(())).lock().await;
    let previous = std::env::current_dir().expect("read test cwd");
    std::env::set_current_dir(root.path()).expect("set fixture cwd");
    let agent = GrokAcp::spawn_configured(model, launch)
        .await
        .expect("spawn configured ACP fixture");
    std::env::set_current_dir(previous).expect("restore test cwd");
    drop(lock);
    agent
}

fn codex_homes(root: &TempDir) -> (PathBuf, PathBuf) {
    let source = root.path().join("source-codex");
    let isolated = root.path().join("isolated-codex");
    fs::create_dir(&source).expect("create source Codex home");
    fs::write(
        source.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write fixture Codex auth");
    (source, isolated)
}

fn command_wrapper(root: &TempDir, mode: &str) -> PathBuf {
    let trace = root.path().join("command-code-trace.jsonl");
    let wrapper = root.path().join("command-code-cmd");
    let mut script = format!(
        "#!/bin/sh\nexport COMMAND_CODE_CMD_MOCK_TRACE='{}'\n",
        trace.display()
    );
    if !mode.is_empty() {
        script.push_str(&format!("export COMMAND_CODE_CMD_MOCK_MODE='{mode}'\n"));
    }
    script.push_str(&format!(
        "exec '{}' \"$@\"\n",
        env!("CARGO_BIN_EXE_command-code-cmd-mock")
    ));
    fs::write(&wrapper, script).expect("write command-code wrapper");
    let mut permissions = fs::metadata(&wrapper)
        .expect("read command-code wrapper metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).expect("chmod command-code wrapper");
    wrapper
}

async fn endpoint(root: TempDir, bridge: Bridge, model: &str) -> Endpoint {
    let model = model.to_owned();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind parity endpoint");
    let address = listener.local_addr().expect("parity endpoint address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(std::sync::Arc::new(bridge), model, None),
        )
        .await
        .expect("serve parity endpoint");
    });
    Endpoint {
        _root: root,
        server,
        url: format!("http://{address}/v1/messages"),
    }
}

async fn stream_turn(
    endpoint: &Endpoint,
    surface: Surface,
    prompt: &str,
    session: &str,
) -> Vec<Value> {
    let response = send_stream(&endpoint.url, request(surface, prompt, true), Some(session))
        .await
        .unwrap_or_else(|error| panic!("{} request failed: {error}", surface.label()));
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{} status",
        surface.label()
    );
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("{} stream body failed: {error}", surface.label()));
    parse_sse(&body)
}

async fn send_stream(
    url: &str,
    body: Value,
    session: Option<&str>,
) -> Result<Response, reqwest::Error> {
    let client = Client::new();
    let mut request = client.post(url).json(&body);
    if let Some(session) = session {
        request = request.header("x-claude-code-session-id", session);
    }
    if body["model"] == CLAUDE_MODEL {
        request = request.header("x-claude-code-agent-id", "provider-parity-agent");
    }
    request.send().await
}

fn request(surface: Surface, prompt: &str, stream: bool) -> Value {
    let mut value = json!({
        "model": surface.model(),
        "max_tokens": 256,
        "stream": stream,
        "system": "provider parity contract",
        "tools": serde_json::from_str::<Value>(LOOKUP_TOOLS).expect("lookup tool fixture"),
        "messages": [{"role":"user","content":prompt}]
    });
    if matches!(surface, Surface::ClaudeSubscription | Surface::CommandCode) {
        value["system"] =
            json!("cc_is_subagent=true\n<claudex-agent-id>provider-parity</claudex-agent-id>");
    }
    value
}

fn follow_up_request(surface: Surface, prior: String) -> Value {
    let mut body = request(surface, surface.prompt(), true);
    body["messages"] = json!([
        {"role":"user","content":surface.prompt()},
        {"role":"assistant","content":[{"type":"text","text":prior}]},
        {"role":"user","content":"follow up using the same provider session"}
    ]);
    body
}

fn error_request(surface: Surface) -> Value {
    let prompt = match surface {
        Surface::ClaudeSubscription => "SUBSCRIPTION_FAILURE",
        Surface::ConfiguredGrok => "forced provider failure",
        Surface::CommandCode => "forced command failure",
        Surface::CodexAppServer => "TURN_FAILED",
    };
    request(surface, prompt, true)
}

fn parse_sse(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|chunk| {
            chunk
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str(data.trim()).ok())
        })
        .collect()
}

fn assert_stream_contract(surface: Surface, events: &[Value], expected_text: &str) {
    let starts = events
        .iter()
        .filter(|event| event["type"] == "message_start")
        .count();
    assert_eq!(
        starts,
        1,
        "{} message_start cardinality: {events:?}",
        surface.label()
    );
    let stops = events
        .iter()
        .filter(|event| event["type"] == "message_stop")
        .count();
    assert_eq!(
        stops,
        1,
        "{} message_stop cardinality: {events:?}",
        surface.label()
    );
    let model = events
        .iter()
        .find(|event| event["type"] == "message_start")
        .and_then(|event| event.pointer("/message/model"))
        .and_then(Value::as_str);
    assert_eq!(
        model,
        Some(surface.model()),
        "{} model identity",
        surface.label()
    );
    let deltas = events
        .iter()
        .filter(|event| event["type"] == "content_block_delta")
        .collect::<Vec<_>>();
    assert!(
        !deltas.is_empty(),
        "{} emitted no progress/text delta",
        surface.label()
    );
    let text = deltas
        .iter()
        .filter_map(|event| event.pointer("/delta/text").and_then(Value::as_str))
        .collect::<String>();
    assert!(
        text.contains(expected_text),
        "{} result missing {expected_text:?}: {text:?}",
        surface.label()
    );
    assert!(
        events.iter().all(|event| {
            event["type"] != "content_block_start"
                || event.pointer("/content_block/type").and_then(Value::as_str) != Some("tool_use")
        }),
        "{} provider tools must stay progress-only, not executable tool_use: {events:?}",
        surface.label()
    );
    let stop_reason = events
        .iter()
        .find(|event| event["type"] == "message_delta")
        .and_then(|event| event.pointer("/delta/stop_reason"))
        .and_then(Value::as_str);
    assert_eq!(
        stop_reason,
        Some("end_turn"),
        "{} stop reason",
        surface.label()
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "error")
            .count(),
        0,
        "{} successful stream contains an error",
        surface.label()
    );
}

fn assert_follow_up_contract(surface: Surface, events: &[Value]) {
    let starts = events
        .iter()
        .filter(|event| event["type"] == "message_start")
        .count();
    let stops = events
        .iter()
        .filter(|event| event["type"] == "message_stop")
        .count();
    assert_eq!(starts, 1, "{} follow-up message_start", surface.label());
    assert_eq!(stops, 1, "{} follow-up message_stop", surface.label());
    let model = events
        .iter()
        .find(|event| event["type"] == "message_start")
        .and_then(|event| event.pointer("/message/model"))
        .and_then(Value::as_str);
    assert_eq!(
        model,
        Some(surface.model()),
        "{} follow-up model",
        surface.label()
    );
    let text = streamed_text(events);
    assert!(
        !text.trim().is_empty(),
        "{} follow-up emitted no assistant text",
        surface.label()
    );
    let stop_reason = events
        .iter()
        .find(|event| event["type"] == "message_delta")
        .and_then(|event| event.pointer("/delta/stop_reason"))
        .and_then(Value::as_str);
    assert_eq!(
        stop_reason,
        Some("end_turn"),
        "{} follow-up stop reason",
        surface.label()
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "error")
            .count(),
        0,
        "{} follow-up contains an error",
        surface.label()
    );
}

fn assert_native_progress(surface: Surface, events: &[Value]) {
    let expected = match surface {
        Surface::ClaudeSubscription => return,
        Surface::ConfiguredGrok | Surface::CodexAppServer => "Read config",
        Surface::CommandCode => "read_file",
    };
    let thinking = events
        .iter()
        .filter_map(|event| event.pointer("/delta/thinking").and_then(Value::as_str))
        .collect::<String>();
    assert!(
        thinking.contains(expected),
        "{} native tool/progress chrome missing {expected:?}: {thinking:?}",
        surface.label()
    );
}

fn streamed_text(events: &[Value]) -> String {
    events
        .iter()
        .filter_map(|event| event.pointer("/delta/text").and_then(Value::as_str))
        .collect()
}

async fn expect_follow_up(url: &str, request: Value, surface: Surface) -> reqwest::Response {
    send_stream(url, request, Some(SESSION_ID))
        .await
        .unwrap_or_else(|error| panic!("{} follow-up request failed: {error}", surface.label()))
}

fn assert_optional_error_stop_reason(
    stop_reason: Option<&str>,
    status: StatusCode,
    surface: Surface,
) {
    if let Some(stop_reason) = stop_reason {
        assert_eq!(
            stop_reason,
            "error",
            "{} error stop reason (HTTP status {status})",
            surface.label()
        );
    }
}

async fn wait_for_trace_event(path: &Path, key: &str) {
    tokio::time::timeout(
        Duration::from_secs(3),
        wait_for_trace_event_inner(path, key),
    )
    .await
    .unwrap_or_else(|_| panic!("trace {} did not contain {key}", path.display()));
}

async fn wait_for_trace_event_inner(path: &Path, key: &str) {
    loop {
        if read_trace(path)
            .iter()
            .any(|event| event.get(key).is_some())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn read_trace(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
