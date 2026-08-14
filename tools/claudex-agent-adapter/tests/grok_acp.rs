#[path = "support/coverage_profile.rs"]
mod coverage_profile;
#[path = "support/project_fixture.rs"]
mod project_fixture;

use std::{
    ffi::{OsStr, OsString},
    os::unix::fs::{PermissionsExt as _, symlink},
    path::Path,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use claudex_agent_adapter::{
    agent_backend::{AcpLaunch, AgentBackend},
    anthropic::Bridge,
    grok_acp::GrokAcp,
    http_router, provider_config,
};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::{Mutex, MutexGuard};

use project_fixture::ProjectFixture;

const LOOPBACK_EPHEMERAL_ADDRESS: &str = "127.0.0.1:0";
const PARALLEL_SESSION_CREATION_TIMEOUT: Duration = Duration::from_secs(1);
const PARALLEL_SESSION_COUNT: usize = 2;
const EXPECTED_PROVIDER_PROMPT_COUNT: usize = 1;
const ACP_THINKING_MARKER: &str = "ACP_THINKING_MARKER";

static PROCESS_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn grok_mock_program(root: &Path) -> std::path::PathBuf {
    coverage_profile::wrapped_program(root, env!("CARGO_BIN_EXE_grok-acp-mock"))
}

fn grok_mock_program_string(root: &Path) -> String {
    coverage_profile::wrapped_program_string(root, env!("CARGO_BIN_EXE_grok-acp-mock"))
}

async fn process_env_lock() -> MutexGuard<'static, ()> {
    PROCESS_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().await
}

struct ScopedEnv {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnv {
    fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: integration tests serialize process-wide environment changes with
        // PROCESS_ENV_LOCK and restore the previous value on drop.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: integration tests serialize process-wide environment changes with
        // PROCESS_ENV_LOCK and restore the previous value on drop.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: the guard restores a value captured from this process and is only
        // used while PROCESS_ENV_LOCK is held by the owning test.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

struct ScopedCurrentDir {
    previous: std::path::PathBuf,
}

impl ScopedCurrentDir {
    fn enter(path: &Path) -> Self {
        let previous = std::env::current_dir().expect("read current directory");
        std::env::set_current_dir(path).expect("set test current directory");
        Self { previous }
    }
}

impl Drop for ScopedCurrentDir {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

fn is_acp_thinking_delta(event: &Value) -> bool {
    event.pointer("/delta/type").and_then(Value::as_str) == Some("thinking_delta")
        && event
            .pointer("/delta/thinking")
            .and_then(Value::as_str)
            .is_some_and(|thinking| thinking.contains(ACP_THINKING_MARKER))
}

fn is_xai_subagent_thinking_delta(event: &Value) -> bool {
    event.pointer("/delta/type").and_then(Value::as_str) == Some("thinking_delta")
        && event
            .pointer("/delta/thinking")
            .and_then(Value::as_str)
            .is_some_and(|thinking| thinking.contains("Research") && thinking.contains("grok-4.6"))
}

#[tokio::test]
async fn streams_grok_acp_with_launch_scoped_model_effort_and_instructions() {
    let root = tempfile::tempdir().expect("Grok ACP fixture");
    let agent = GrokAcp::spawn_with_program(
        "grok-4.6",
        grok_mock_program(root.path()),
        root.path().to_owned(),
    )
    .await
    .expect("start Grok ACP mock");
    let backend = AgentBackend::grok(agent);
    assert!(backend.is_alive());
    assert_eq!(backend.kind().to_string(), "grok-acp");
    let response = backend
        .request(
            "thread/start",
            json!({
                "baseInstructions":"project policy\n\nCodex bridge policy",
                "developerInstructions":"Codex bridge policy"
            }),
        )
        .await
        .expect("create ACP session");
    let thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .unwrap();
    for effort in ["low", "mid", "xhigh", "max"] {
        let events = backend.subscribe_thread(thread_id);
        backend
            .request_detached(
                "turn/start",
                json!({
                    "threadId":thread_id,
                    "effort":effort,
                    "input":[{"type":"text","text":"user prompt"}]
                }),
            )
            .await
            .expect("start ACP turn");
        let first = recv(&events).await;
        let second = recv(&events).await;
        assert_eq!(
            first.pointer("/params/delta").and_then(Value::as_str),
            Some("GROK_ACP_STREAM_OK"),
            "unexpected first event: {first}"
        );
        assert_eq!(
            second.get("method").and_then(Value::as_str),
            Some("turn/completed")
        );
    }

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert_trace(&trace);
    assert!(!trace.iter().any(|event| event.get("cancel").is_some()));
    assert!(backend.request("unsupported", json!({})).await.is_err());
    assert!(
        backend
            .request_detached("unsupported", json!({}))
            .await
            .is_err()
    );
    assert!(backend.respond(json!(1), json!({})).await.is_err());
}

#[tokio::test]
async fn native_grok_and_configured_cline_thoughts_stream_as_anthropic_thinking_deltas() {
    for route in [
        AcpThoughtRoute::NativeGrok,
        AcpThoughtRoute::ConfiguredCline,
    ] {
        let root = tempfile::tempdir().expect("ACP thinking fixture");
        let (model, backend) = spawn_thinking_backend(route, root.path()).await;
        let bridge = Arc::new(Bridge::new_with_backend(backend, model.to_owned()));
        let listener = tokio::net::TcpListener::bind(LOOPBACK_EPHEMERAL_ADDRESS)
            .await
            .expect("bind ACP thinking adapter");
        let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let app = http_router(bridge, model.to_owned(), None);
        let server = tokio::spawn(axum::serve(listener, app).into_future());

        let response = tokio::time::timeout(
            Duration::from_secs(3),
            Client::new()
                .post(&url)
                .json(&json!({
                    "model":model,
                    "max_tokens":64,
                    "stream":true,
                    "system":"cc_is_subagent=true",
                    "messages":[{"role":"user","content":"stream one thought"}]
                }))
                .send(),
        )
        .await
        .unwrap_or_else(|_| panic!("{} request timed out", route.label()))
        .unwrap_or_else(|error| panic!("{} request failed: {error}", route.label()));
        let body = tokio::time::timeout(Duration::from_secs(3), response.text())
            .await
            .unwrap_or_else(|_| panic!("{} stream timed out", route.label()))
            .unwrap_or_else(|error| panic!("{} stream body failed: {error}", route.label()));
        let events = parse_sse(&body);
        assert!(
            events.iter().any(is_acp_thinking_delta),
            "{} did not expose its ACP thought as Anthropic thinking_delta: {body}",
            route.label()
        );
        assert!(
            events.iter().any(|event| event["type"] == "message_stop"),
            "{} stream did not terminate: {body}",
            route.label()
        );
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn native_grok_xai_subagent_spawn_streams_as_anthropic_thinking() {
    let root = tempfile::tempdir().expect("xAI spawn fixture");
    let agent = spawn_mock("xai-spawn", root.path()).await;
    let backend = AgentBackend::grok(agent);
    let bridge = Arc::new(Bridge::new_with_backend(backend, "xai-spawn".to_owned()));
    let listener = tokio::net::TcpListener::bind(LOOPBACK_EPHEMERAL_ADDRESS)
        .await
        .expect("bind xAI spawn adapter");
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "xai-spawn".to_owned(), None))
            .await
            .expect("serve xAI spawn adapter");
    });

    let response = Client::new()
        .post(&url)
        .json(&json!({
            "model":"xai-spawn",
            "max_tokens":64,
            "stream":true,
            "messages":[{"role":"user","content":"delegate research"}]
        }))
        .send()
        .await
        .expect("send xAI spawn request");
    let body = response.text().await.expect("read xAI spawn stream");
    let events = parse_sse(&body);
    assert!(
        events.iter().any(is_xai_subagent_thinking_delta),
        "xAI spawn was not exposed as an Anthropic thinking_delta: {body}"
    );
    assert!(
        events.iter().any(|event| event["type"] == "message_stop"),
        "xAI spawn stream did not terminate: {body}"
    );
    assert!(
        !events.iter().any(|event| {
            event["type"] == "content_block_start"
                && event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
        }),
        "xAI spawn incorrectly became a tool_use block: {body}"
    );
    server.abort();
    let _ = server.await;
}

#[derive(Clone, Copy)]
enum AcpThoughtRoute {
    NativeGrok,
    ConfiguredCline,
}

impl AcpThoughtRoute {
    const fn label(self) -> &'static str {
        match self {
            Self::NativeGrok => "native Grok",
            Self::ConfiguredCline => "configured Cline",
        }
    }
}

async fn spawn_thinking_backend(
    route: AcpThoughtRoute,
    root: &Path,
) -> (&'static str, Arc<AgentBackend>) {
    match route {
        AcpThoughtRoute::NativeGrok => {
            let model = "grok-thinking-stream";
            let agent =
                GrokAcp::spawn_with_program(model, grok_mock_program(root), root.to_owned())
                    .await
                    .expect("start native Grok thinking mock");
            (model, AgentBackend::grok(agent))
        }
        AcpThoughtRoute::ConfiguredCline => {
            let _process_env_lock = process_env_lock().await;
            let model = "cline-pass/deepseek-v4-flash";
            let launch = AcpLaunch {
                program: grok_mock_program_string(root),
                arguments: vec!["--mode".to_owned(), "cline-thinking-stream".to_owned()],
            };
            let _current_dir = ScopedCurrentDir::enter(root);
            let agent = GrokAcp::spawn_configured(model, &launch).await;
            let agent = agent.expect("start configured Cline thinking mock");
            (model, AgentBackend::configured_acp(agent))
        }
    }
}

fn parse_sse(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str(data).ok())
        })
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_config_high_reaches_the_exact_native_grok_argv() {
    let _process_env_lock = process_env_lock().await;
    let root = tempfile::tempdir().expect("provider route fixture");
    let config = root.path().join("providers.json");
    std::fs::write(
        &config,
        r#"{
            "version":1,
            "mainProviders":["grok"],
            "providers":[{
                "id":"grok",
                "agent":"claudex-grok",
                "defaultModel":"grok-4.6",
                "effort":"high",
                "backend":"grok-acp"
            }],
            "fallback":{"agent":"fallback","model":"claude-sonnet-5","effort":"high"}
        }"#,
    )
    .expect("write provider config");
    let loaded = provider_config::load(&config).expect("load provider config");
    assert_eq!(loaded.routes[0].effort.as_deref(), Some("high"));

    let wrapper = root.path().join("grok-wrapper");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ncd '{}' || exit 1\nexec '{}' \"$@\"\n",
            root.path().display(),
            grok_mock_program(root.path()).display()
        ),
    )
    .expect("write Grok wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
        .expect("make Grok wrapper executable");

    let _program_env = ScopedEnv::set("CLAUDEX_GROK_PROGRAM", &wrapper);
    let backend = AgentBackend::spawn_routes(&loaded.routes);
    let bridge = Arc::new(
        Bridge::new_with_backend(backend, "grok-4.6".to_owned())
            .with_model_catalog(loaded.model_catalog.clone()),
    );
    let listener = tokio::net::TcpListener::bind(LOOPBACK_EPHEMERAL_ADDRESS)
        .await
        .expect("bind configured Grok adapter");
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "grok-4.6".to_owned(), None))
            .await
            .expect("serve configured Grok adapter");
    });
    let client = Client::new();
    let mut responses = Vec::new();
    for effort in ["low", "max"] {
        responses.push((
            effort,
            client
                .post(&url)
                .json(&json!({
                    "model":"grok-4.6",
                    "output_config":{"effort":effort},
                    "messages":[{"role":"user","content":format!("request {effort}")}]
                }))
                .send()
                .await,
        ));
    }
    for (effort, response) in responses {
        let response = response
            .unwrap_or_else(|error| panic!("send {effort} Grok request: {error}"))
            .error_for_status()
            .unwrap_or_else(|error| panic!("{effort} Grok request status: {error}"))
            .json::<Value>()
            .await
            .unwrap_or_else(|error| panic!("decode {effort} Grok response: {error}"));
        assert_eq!(response["stop_reason"], "end_turn");
    }

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert!(trace.iter().any(|event| event["arguments"]
        == json!([
            "--model",
            "grok-4.6",
            "--reasoning-effort",
            "high",
            "agent",
            "--always-approve",
            "--no-leader",
            "stdio"
        ])));
    assert!(trace.iter().all(|event| event.get("set_model").is_none()));
    assert!(trace.iter().all(|event| event.get("set_effort").is_none()));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn generated_plugin_and_parent_child_marker_form_a_grok_boundary_contract() {
    let _process_env_lock = process_env_lock().await;
    let root = tempfile::tempdir().expect("Grok plugin boundary fixture");
    let home = root.path().join("home");
    std::fs::create_dir(&home).expect("create isolated Grok home");
    let grok = root.path().join("grok");
    symlink(grok_mock_program(root.path()), &grok).expect("symlink mock as grok");

    let _home_env = ScopedEnv::set("HOME", &home);
    let _plugin_env = ScopedEnv::remove("CLAUDEX_GROK_PLUGIN_DIR");
    let spawned =
        GrokAcp::spawn_with_program("nested-boundary", &grok, root.path().to_owned()).await;
    let agent = spawned.expect("start mock through real grok program-name branch");

    let plugin = home.join(".cache/claudex/grok-native-high-plugin-v3");
    let profile = std::fs::read_to_string(plugin.join("agents/claudex-high.md"))
        .expect("read generated provider-local high profile");
    assert!(profile.contains("effort: high"));
    assert!(!profile.contains("\nmodel:"));
    for invalid in ["claudex-xhigh.md", "claudex-max.md", "claudex-gpt.md"] {
        assert!(!plugin.join("agents").join(invalid).exists());
    }
    assert!(
        home.join(".grok/hooks/claudex-agent-adapter.json")
            .is_file()
    );
    assert!(!plugin.join("hooks/hooks.json").exists());
    assert!(plugin.join("bin/reject-cross-provider-agent.sh").is_file());

    let response = agent.create_session(json!({})).await.unwrap();
    let thread_id = response["thread"]["id"].as_str().unwrap();
    let events = agent.subscribe_thread(thread_id);
    agent
        .start_turn(json!({
            "threadId":thread_id,
            "input":"Return the mock child boundary marker"
        }))
        .await
        .unwrap();
    assert_eq!(
        recv(&events).await["params"]["delta"],
        "GROK_ACP_CHILD_BOUNDARY_MARKER"
    );
    assert_eq!(recv(&events).await["params"]["turn"]["status"], "completed");

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert!(trace.iter().any(|event| event["arguments"]
        == json!([
            "--model",
            "nested-boundary",
            "--reasoning-effort",
            "high",
            "agent",
            "--always-approve",
            "--no-leader",
            "--plugin-dir",
            plugin,
            "stdio"
        ])));
    assert!(trace.iter().all(|event| event.get("set_model").is_none()));
    assert!(trace.iter().all(|event| event.get("set_effort").is_none()));
}

#[tokio::test]
async fn creates_grok_acp_session_in_the_request_working_directory() {
    let root = tempfile::tempdir().expect("request cwd fixture");
    let active_cwd = root.path().join("active-child");
    std::fs::create_dir(&active_cwd).expect("create active child cwd");
    let active_cwd = std::fs::canonicalize(active_cwd).expect("canonicalize active child cwd");
    let agent = GrokAcp::spawn_with_program(
        "request-cwd",
        grok_mock_program(root.path()),
        root.path().to_owned(),
    )
    .await
    .expect("start request cwd mock");

    agent
        .create_session(json!({
            "cwd":"/adapter/isolated/runtime/must-not-win",
            "baseInstructions":format!(
                "Project policy\n- Primary working directory: {}\nBridge policy",
                active_cwd.display()
            )
        }))
        .await
        .expect("create request cwd session");

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert!(
        trace.iter().any(|event| {
            event.pointer("/new_session/cwd").and_then(Value::as_str) == active_cwd.to_str()
        }),
        "trace={trace:?}"
    );
}

#[tokio::test]
async fn grok_attaches_launch_mcp_in_session_new_without_session_load() {
    let root = tempfile::tempdir().expect("launch MCP wire fixture");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("mcp-wire", root.path()).await;
    let response = agent
        .create_session(json!({
            "dynamicTools":[{"name":"Agent","description":"Launch a SubAgent"}]
        }))
        .await
        .expect("create Grok session with launch MCP");
    let session_id = response["thread"]["id"]
        .as_str()
        .expect("launch MCP session ID")
        .to_owned();
    let events = agent.subscribe_thread(&session_id);
    agent
        .start_turn(json!({
            "threadId":session_id,
            "input":"prompt with launch MCP"
        }))
        .await
        .expect("prompt with launch MCP");
    assert_eq!(recv(&events).await["params"]["delta"], "GROK_ACP_STREAM_OK");
    assert_eq!(recv(&events).await["params"]["turn"]["status"], "completed");

    let trace = read_trace(&trace_path);
    let new_session = trace
        .iter()
        .find(|event| event.get("new_session").is_some())
        .expect("session/new trace");
    let mcp_servers = new_session
        .pointer("/new_session/mcpServers")
        .and_then(Value::as_array)
        .expect("launch MCP in session/new");
    assert_eq!(mcp_servers.len(), 1);
    assert_eq!(mcp_servers[0]["name"], "claudex-launch");
    assert!(
        trace
            .iter()
            .all(|event| event.get("load_session").is_none()),
        "Grok launch MCP must not use session/load: {trace:?}"
    );
}

#[tokio::test]
async fn launch_mcp_session_new_failure_reaches_session_creation_error() {
    let _process_env_lock = process_env_lock().await;
    let root = tempfile::tempdir().expect("launch MCP failure fixture");
    let _current_dir = ScopedCurrentDir::enter(root.path());
    let agent = GrokAcp::spawn_configured(
        "mcp-failure",
        &AcpLaunch {
            program: grok_mock_program_string(root.path()),
            arguments: vec!["--mode".to_owned(), "fail-mcp-new".to_owned()],
        },
    )
    .await
    .expect("start configured ACP MCP failure fixture");
    let error = agent
        .create_session(json!({
            "dynamicTools":[{"name":"Agent","description":"Launch a SubAgent"}]
        }))
        .await
        .expect_err("failed launch MCP session/new must fail session creation");
    let message = error.to_string();
    assert!(
        message.contains("launch MCP attachment during session/new failed"),
        "session creation hid launch MCP failure: {message}"
    );
    assert!(
        message.contains("session/new failed"),
        "session creation hid underlying ACP method: {message}"
    );
    agent.shutdown().await;
}

fn assert_trace(trace: &[Value]) {
    assert!(trace.iter().any(|event| event["arguments"]
        == json!([
            "--model",
            "grok-4.6",
            "--reasoning-effort",
            "high",
            "agent",
            "--always-approve",
            "--no-leader",
            "stdio"
        ])));
    assert!(trace.iter().any(|event| event["claudex_grok_acp"] == "1"));
    assert!(trace.iter().all(|event| event.get("set_model").is_none()));
    assert!(trace.iter().all(|event| event.get("set_effort").is_none()));
    assert!(
        trace
            .iter()
            .all(|event| event.pointer("/new_session/_meta/modelId").is_none())
    );
    assert!(trace.iter().any(
        |event| event.pointer("/permission_response/outcome/optionId")
            == Some(&json!("allow-once"))
    ));
    let prompt = trace
        .iter()
        .find_map(|event| {
            event
                .pointer("/prompt/prompt/0/text")
                .and_then(Value::as_str)
        })
        .expect("prompt trace");
    assert!(prompt.starts_with("project policy\n\nClaudex SubAgent routing on ACP:"));
    assert!(prompt.contains("Agent or Task"));
    assert!(prompt.contains("selected_workers"));
    assert!(prompt.contains("spawn_subagent"));
    assert!(!prompt.contains("claudex-xhigh"));
    assert!(prompt.ends_with("\n\nuser prompt"));
}

#[tokio::test]
async fn reports_acp_startup_effort_and_prompt_failures() {
    let missing = GrokAcp::spawn_with_program(
        "model",
        "/definitely/missing/grok",
        std::env::current_dir().unwrap(),
    )
    .await;
    assert!(missing.is_err());
    let root = tempfile::tempdir().expect("protocol fixture");
    let incompatible = GrokAcp::spawn_with_program(
        "bad-version",
        grok_mock_program(root.path()),
        root.path().to_owned(),
    )
    .await;
    assert!(incompatible.is_err());

    for model in ["fail-initialize", "fail-auth"] {
        let root = tempfile::tempdir().expect("startup error fixture");
        let failed = GrokAcp::spawn_with_program(
            model,
            grok_mock_program(root.path()),
            root.path().to_owned(),
        )
        .await;
        assert!(failed.is_err());
    }

    let root = tempfile::tempdir().expect("session error fixture");
    let agent = spawn_mock("fail-session", root.path()).await;
    assert!(agent.create_session(json!({})).await.is_err());

    for (model, effort, expected) in [("fail-prompt", None::<&str>, "Internal error")] {
        let root = tempfile::tempdir().expect("error fixture");
        let agent = spawn_mock(model, root.path()).await;
        let response = agent.create_session(json!({})).await.unwrap();
        let thread_id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .unwrap();
        let events = agent.subscribe_thread(thread_id);
        agent
            .start_turn(json!({"threadId":thread_id,"effort":effort,"input":null}))
            .await
            .unwrap();
        let event = recv(&events).await;
        assert_eq!(event.get("method").and_then(Value::as_str), Some("error"));
        let message = event
            .pointer("/params/error/message")
            .and_then(Value::as_str)
            .unwrap();
        assert!(message.contains(expected), "unexpected error: {message}");
    }

    let root = tempfile::tempdir().expect("no-auth fixture");
    let agent = spawn_mock("no-auth", root.path()).await;
    assert!(agent.is_alive());
}

#[cfg(unix)]
#[tokio::test]
async fn configured_opencode_route_uses_the_stderr_watcher_path() {
    let _process_env_lock = process_env_lock().await;
    let root = tempfile::tempdir().expect("opencode fixture");
    let opencode = root.path().join("opencode");
    std::os::unix::fs::symlink(grok_mock_program(root.path()), &opencode)
        .expect("opencode fixture symlink");
    let _current_dir = ScopedCurrentDir::enter(root.path());
    let result = GrokAcp::spawn_configured(
        "opencode-go/test-model",
        &AcpLaunch {
            program: opencode.to_string_lossy().into_owned(),
            arguments: Vec::new(),
        },
    )
    .await;
    let agent = match result {
        Ok(agent) => agent,
        Err(error) => {
            let trace = std::fs::read_to_string(root.path().join("grok-acp-mock.jsonl"))
                .unwrap_or_else(|read_error| format!("trace unavailable: {read_error}"));
            panic!("start configured opencode fixture: {error}; trace: {trace}");
        }
    };
    assert!(agent.is_alive());
    agent.shutdown().await;
}

#[tokio::test]
async fn startup_timeout_terminates_a_provider_that_ignores_initialize() {
    let root = tempfile::tempdir().expect("initialize timeout fixture");
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        GrokAcp::spawn_with_program(
            "ignored-initialize",
            grok_mock_program(root.path()),
            root.path().to_owned(),
        ),
    )
    .await
    .expect("initialize timeout fixture hung");
    let error = match result {
        Ok(_) => panic!("ignored initialize must fail startup"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("timed out"),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn grok_plugin_reports_a_stale_alias_directory_that_cannot_be_removed() {
    let _process_env_lock = process_env_lock().await;
    let home = tempfile::tempdir().expect("plugin home");
    let agents = home
        .path()
        .join(".cache/claudex/grok-native-high-plugin-v3/agents");
    std::fs::create_dir_all(&agents).expect("plugin agents directory");
    std::fs::create_dir(agents.join("claudex-gpt.md")).expect("stale alias directory");
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("plugin bin directory");
    let grok = bin.join("grok");
    std::os::unix::fs::symlink(grok_mock_program(home.path()), &grok)
        .expect("grok fixture symlink");

    let _home_env = ScopedEnv::set("HOME", home.path());
    let cwd = tempfile::tempdir().expect("plugin cwd");
    let result = GrokAcp::spawn_with_program("grok-model", grok, cwd.path().to_owned()).await;

    let error = match result {
        Ok(_) => panic!("stale alias directory must fail plugin preparation"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("remove stale Grok shadow"));
}

#[tokio::test]
async fn driver_reaps_a_provider_that_exits_during_session_creation() {
    let root = tempfile::tempdir().expect("provider exit fixture");
    let agent = spawn_mock("exit-once-session", root.path()).await;
    assert!(agent.create_session(json!({})).await.is_err());
    assert!(!agent.is_alive());
    agent.shutdown().await;
}

#[tokio::test]
async fn driver_stops_when_provider_io_closes_before_the_process_exits() {
    let root = tempfile::tempdir().expect("provider I/O fixture");
    let agent = spawn_mock("close-io", root.path()).await;
    std::fs::write(root.path().join("grok-acp-close-io"), b"close")
        .expect("request provider I/O close");

    let observed = tokio::time::timeout(Duration::from_secs(5), wait_for_driver_stop(&agent))
        .await
        .is_ok();
    if !observed {
        agent.shutdown().await;
        panic!("driver did not observe provider I/O closure");
    }
    agent.shutdown().await;
}

async fn wait_for_driver_stop(agent: &GrokAcp) {
    while agent.is_alive() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn scheduler_rejects_a_turn_without_a_thread_id() {
    let root = tempfile::tempdir().expect("missing thread id fixture");
    let agent = spawn_mock("", root.path()).await;
    assert!(
        agent
            .start_turn(json!({"input":"missing thread id"}))
            .await
            .is_err()
    );
    agent.shutdown().await;
}

#[tokio::test]
async fn forwards_grok_tool_subagent_retry_and_usage_updates() {
    let root = tempfile::tempdir().expect("coverage update fixture");
    let agent = spawn_mock("coverage-updates", root.path()).await;
    let response = agent.create_session(json!({})).await.unwrap();
    let thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .unwrap();
    let events = agent.subscribe_thread(thread_id);
    agent
        .start_turn(json!({"threadId":thread_id,"input":"coverage"}))
        .await
        .unwrap();

    let received = recv_until_completed(&events).await;
    let combined = received.iter().map(Value::to_string).collect::<String>();
    assert!(combined.contains("Completed search"));
    assert!(combined.contains("SubAgent started"));
    assert!(combined.contains("Retrying provider"));
    assert!(combined.contains("tokenUsage"));
}

#[tokio::test]
async fn bridge_renders_provider_tools_as_progress_without_a_follow_up_tool_turn() {
    let root = tempfile::tempdir().expect("provider progress fixture");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("coverage-updates", root.path()).await;
    let backend = AgentBackend::routed(vec![(
        "coverage-updates".to_owned(),
        AgentBackend::grok(agent),
    )]);
    let bridge = Arc::new(Bridge::new_with_backend(
        backend,
        "coverage-updates".to_owned(),
    ));
    let listener = tokio::net::TcpListener::bind(LOOPBACK_EPHEMERAL_ADDRESS)
        .await
        .expect("bind provider progress server");
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "coverage-updates".to_owned(), None),
        )
        .await
        .expect("serve provider progress bridge");
    });

    let response: Value = Client::new()
        .post(url)
        .json(&json!({
            "model":"coverage-updates",
            "messages":[{"role":"user","content":"show provider progress"}]
        }))
        .send()
        .await
        .expect("request provider progress")
        .error_for_status()
        .expect("provider progress status")
        .json()
        .await
        .expect("provider progress response");

    assert_eq!(response["stop_reason"], "end_turn");
    let content = response["content"].as_array().expect("response content");
    // Provider tool chrome is live-only and stripped from the committed answer,
    // matching Claude Code (tool cards are not assistant text).
    assert!(content.iter().all(|block| block["type"] != "tool_use"));
    assert!(
        content.iter().all(|block| {
            block["text"]
                .as_str()
                .is_none_or(|text| !text.contains('▶'))
        }),
        "progress chrome must not remain in committed text: {response}"
    );
    let text = content
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<String>();
    assert!(text.contains("GROK_ACP_STREAM_OK"), "response={response}");
    let trace = read_trace(&trace_path);
    assert_eq!(
        trace
            .iter()
            .filter(|event| event.get("prompt").is_some())
            .count(),
        EXPECTED_PROVIDER_PROMPT_COUNT,
        "provider display must not cause a follow-up ACP prompt"
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn grok_read_only_end_turn_streams_an_empty_main_segment() {
    let root = tempfile::tempdir().expect("read-empty fixture");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("read-empty", root.path()).await;
    let backend = AgentBackend::routed(vec![("read-empty".to_owned(), AgentBackend::grok(agent))]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, "read-empty".to_owned()));
    let listener = tokio::net::TcpListener::bind(LOOPBACK_EPHEMERAL_ADDRESS)
        .await
        .expect("bind read-empty server");
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, http_router(bridge, "read-empty".to_owned(), None))
            .await
            .expect("serve read-empty bridge");
    });

    let response = tokio::time::timeout(
        Duration::from_secs(3),
        Client::new()
            .post(url)
            .json(&json!({
                "model":"read-empty",
                "max_tokens":64,
                "stream":true,
                "messages":[{"role":"user","content":"inspect config"}]
            }))
            .send(),
    )
    .await
    .expect("read-empty request timeout")
    .expect("read-empty request")
    .error_for_status()
    .expect("read-empty response status");
    let body = tokio::time::timeout(Duration::from_secs(3), response.text())
        .await
        .expect("read-empty stream timeout")
        .expect("read-empty stream body");
    let events = parse_sse(&body);
    let completion = events
        .iter()
        .find(|event| event["type"] == "message_delta")
        .expect("empty main stream completion");
    assert_eq!(completion["delta"]["stop_reason"], "end_turn");
    assert!(events.iter().any(|event| event["type"] == "message_stop"));
    assert!(events.iter().all(|event| {
        event.pointer("/delta/type").and_then(Value::as_str) != Some("text_delta")
    }));
    assert!(events.iter().any(|event| {
        event.pointer("/delta/type").and_then(Value::as_str) == Some("thinking_delta")
            && event
                .pointer("/delta/thinking")
                .and_then(Value::as_str)
                .map(|thinking| thinking.contains("▶ Read"))
                == Some(true)
    }));

    let trace = read_trace(&trace_path);
    assert_eq!(
        trace
            .iter()
            .filter(|event| event.get("prompt").is_some())
            .count(),
        1,
        "a provider Read must finish without an assistant follow-up prompt"
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn streams_two_grok_acp_sessions_concurrently() {
    let root = tempfile::tempdir().expect("concurrent fixture");
    let agent = spawn_mock("concurrent-turns", root.path()).await;
    let first = agent.create_session(json!({})).await.unwrap();
    let first_id = first.pointer("/thread/id").and_then(Value::as_str).unwrap();
    let first_events = agent.subscribe_thread(first_id);
    agent
        .start_turn(json!({"threadId":first_id,"effort":"mid","input":"first"}))
        .await
        .unwrap();

    let second = tokio::time::timeout(
        PARALLEL_SESSION_CREATION_TIMEOUT,
        agent.create_session(json!({})),
    )
    .await
    .expect("session creation blocked behind an active turn")
    .unwrap();
    let second_id = second
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .unwrap();
    let second_events = agent.subscribe_thread(second_id);
    agent
        .start_turn(json!({"threadId":second_id,"effort":"max","input":"second"}))
        .await
        .unwrap();

    let (first_stream, second_stream) = tokio::join!(recv(&first_events), recv(&second_events));
    for event in [first_stream, second_stream] {
        assert_eq!(
            event.pointer("/params/delta").and_then(Value::as_str),
            Some("GROK_ACP_STREAM_OK"),
            "unexpected concurrent event: {event}"
        );
    }
    let (first_done, second_done) = tokio::join!(recv(&first_events), recv(&second_events));
    assert_eq!(first_done["method"], "turn/completed");
    assert_eq!(second_done["method"], "turn/completed");

    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert!(
        trace.iter().all(|event| event.get("set_model").is_none()),
        "native Grok turns must not override launch-scoped model/effort: {trace:?}"
    );
}

#[tokio::test]
async fn queues_parallel_grok_acp_session_requests_without_dropping_them() {
    let root = tempfile::tempdir().expect("parallel session fixture");
    let agent = spawn_mock("", root.path()).await;

    let (first, second) = tokio::time::timeout(PARALLEL_SESSION_CREATION_TIMEOUT, async {
        tokio::join!(
            agent.create_session(json!({})),
            agent.create_session(json!({}))
        )
    })
    .await
    .expect("queued session creation stalled");
    let first = first.expect("first session");
    let second = second.expect("second session");

    assert_ne!(first["thread"]["id"], second["thread"]["id"]);
    let trace = read_trace(&root.path().join("grok-acp-mock.jsonl"));
    assert_eq!(
        trace
            .iter()
            .filter(|event| event.get("new_session").is_some())
            .count(),
        PARALLEL_SESSION_COUNT
    );
}

#[tokio::test]
async fn parallel_session_failure_does_not_interrupt_an_active_turn() {
    let root = tempfile::tempdir().expect("parallel session failure fixture");
    let agent = spawn_mock("fail-parallel-session", root.path()).await;
    let (events, session_id) = start_delayed_turn(&agent).await;

    assert!(agent.create_session(json!({})).await.is_err());
    assert!(agent.is_alive());
    assert_completed_turn(&events).await;
    agent.cancel_turn(&session_id).await.expect("settled turn");
    assert!(agent.is_alive());
}

#[tokio::test]
async fn dropped_parallel_session_request_does_not_interrupt_or_restart_provider() {
    let root = tempfile::tempdir().expect("dropped parallel session fixture");
    let trace = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("dropped-parallel-session", root.path()).await;
    let (events, _session_id) = start_delayed_turn(&agent).await;
    let request_agent = Arc::clone(&agent);
    let request = tokio::spawn(async move { request_agent.create_session(json!({})).await });
    wait_for_trace_count(&trace, "new_session", 2).await;
    request.abort();

    assert_completed_turn(&events).await;
    let next = tokio::time::timeout(Duration::from_secs(1), agent.create_session(json!({})))
        .await
        .expect("provider stopped accepting sessions after a requester disconnected")
        .expect("provider rejected a session after a requester disconnected");
    assert!(next.pointer("/thread/id").is_some());
    assert!(agent.is_alive());
}

async fn start_delayed_turn(
    agent: &Arc<GrokAcp>,
) -> (claudex_agent_adapter::app_server::ThreadEvents, String) {
    let session = agent
        .create_session(json!({}))
        .await
        .expect("first session");
    let session_id = session["thread"]["id"]
        .as_str()
        .expect("session ID")
        .to_owned();
    let events = agent.subscribe_thread(&session_id);
    agent
        .start_turn(json!({"threadId":session_id,"input":"stay alive"}))
        .await
        .expect("start delayed turn");
    (events, session_id)
}

async fn assert_completed_turn(events: &claudex_agent_adapter::app_server::ThreadEvents) {
    assert_eq!(recv(events).await["params"]["delta"], "GROK_ACP_STREAM_OK");
    assert_eq!(recv(events).await["method"], "turn/completed");
}

#[tokio::test]
async fn dropping_http_stream_cancels_the_active_acp_prompt() {
    let root = ProjectFixture::new("disconnect");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("cancellable-turns", root.path()).await;
    let backend = AgentBackend::routed(vec![(
        "cancellable-turns".to_owned(),
        AgentBackend::grok(Arc::clone(&agent)),
    )]);
    let bridge = Arc::new(Bridge::new_with_backend(
        backend,
        "cancellable-turns".to_owned(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(bridge, "cancellable-turns".to_owned(), None),
        )
        .await
        .unwrap();
    });

    let client = Client::new();
    let started = Instant::now();
    let mut response = client
        .post(&url)
        .json(&json!({
            "model":"cancellable-turns",
            "stream":true,
            "messages":[{"role":"user","content":"BLOCK UNTIL DISCONNECT"}]
        }))
        .send()
        .await
        .expect("start cancellable HTTP stream");
    let first_frame = tokio::time::timeout(Duration::from_millis(150), response.chunk())
        .await
        .expect("message_start was buffered behind the ACP prompt")
        .expect("read initial ACP stream frame")
        .expect("ACP stream ended before message_start");
    assert!(
        String::from_utf8_lossy(&first_frame).contains("event: message_start"),
        "unexpected initial ACP stream frame: {}",
        String::from_utf8_lossy(&first_frame)
    );
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "message_start was buffered behind provider setup"
    );
    wait_for_trace_count(&trace_path, "prompt_submitted", 1).await;
    drop(response);

    let trace = wait_for_trace_count(&trace_path, "cancel", 1).await;
    let cancelled_session = trace
        .iter()
        .find_map(|event| event.pointer("/cancel/sessionId").and_then(Value::as_str))
        .expect("session/cancel trace");
    assert_eq!(cancelled_session, "grok-session-1");

    let completed: Value = client
        .post(&url)
        .json(&json!({
            "model":"cancellable-turns",
            "messages":[{"role":"user","content":"COMPLETE NORMALLY"}]
        }))
        .send()
        .await
        .expect("start independent session")
        .error_for_status()
        .expect("independent session status")
        .json()
        .await
        .expect("independent session response");
    assert_eq!(completed["content"][0]["text"], "GROK_ACP_STREAM_OK");
    assert_eq!(completed["stop_reason"], "end_turn");
    server.abort();
}

#[tokio::test]
async fn native_grok_ignores_per_turn_effort_without_session_setup() {
    let root = ProjectFixture::new("launch-scoped-effort");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("blocked-effort", root.path()).await;
    let response = agent.create_session(json!({})).await.unwrap();
    let session_id = response["thread"]["id"].as_str().unwrap();
    let events = agent.subscribe_thread(session_id);

    agent
        .start_turn(json!({
            "threadId":session_id,
            "effort":"max",
            "input":"USE THE LAUNCH-SCOPED EFFORT"
        }))
        .await
        .unwrap();

    assert_eq!(recv(&events).await["params"]["delta"], "GROK_ACP_STREAM_OK");
    assert_eq!(recv(&events).await["params"]["turn"]["status"], "completed");
    let trace = read_trace(&trace_path);
    assert!(trace.iter().all(|event| event.get("set_model").is_none()));
    assert!(trace.iter().all(|event| event.get("set_effort").is_none()));
}

#[tokio::test]
async fn cancelled_turn_releases_capacity_for_another_session() {
    let root = ProjectFixture::new("cancel");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("cancellable-turns", root.path()).await;
    let capacity = agent.turn_capacity();
    let mut blocked = Vec::with_capacity(capacity);
    for index in 0..capacity {
        let response = agent.create_session(json!({})).await.unwrap();
        let session_id = response["thread"]["id"].as_str().unwrap().to_owned();
        let events = agent.subscribe_thread(&session_id);
        agent
            .start_turn(json!({
                "threadId":session_id,
                "priority":"user",
                "input":format!("BLOCK {index}")
            }))
            .await
            .unwrap();
        blocked.push((session_id, events));
    }
    wait_for_trace_count(&trace_path, "prompt", capacity).await;

    let next = agent.create_session(json!({})).await.unwrap();
    let next_id = next["thread"]["id"].as_str().unwrap().to_owned();
    let next_events = agent.subscribe_thread(&next_id);
    let queued_agent = Arc::clone(&agent);
    let queued_id = next_id.clone();
    let mut queued = tokio::spawn(async move {
        queued_agent
            .start_turn(json!({
                "threadId":queued_id,
                "priority":"user",
                "input":"COMPLETE AFTER CANCEL"
            }))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut queued)
            .await
            .is_err(),
        "turn started without available capacity"
    );

    agent.cancel_turn(&blocked[0].0).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut queued)
        .await
        .expect("queued turn did not acquire released capacity")
        .expect("queued turn task failed")
        .expect("queued turn start failed");
    let cancelled = recv(&blocked[0].1).await;
    assert_eq!(cancelled["params"]["turn"]["status"], "cancelled");

    let output = recv(&next_events).await;
    assert_eq!(output["params"]["delta"], "GROK_ACP_STREAM_OK");
    let completed = recv(&next_events).await;
    assert_eq!(completed["params"]["turn"]["status"], "completed");
}

#[tokio::test]
async fn ignored_cancellation_invalidates_only_that_session_and_recovers_capacity() {
    let root = ProjectFixture::new("ignored");
    let trace_path = root.path().join("grok-acp-mock.jsonl");
    let agent = spawn_mock("ignored-cancellation", root.path()).await;
    let capacity = agent.turn_capacity();
    let mut blocked = Vec::with_capacity(capacity);
    for index in 0..capacity {
        let response = agent.create_session(json!({})).await.unwrap();
        let session_id = response["thread"]["id"].as_str().unwrap().to_owned();
        agent
            .start_turn(json!({
                "threadId":session_id,
                "priority":"user",
                "input":format!("BLOCK {index}")
            }))
            .await
            .unwrap();
        blocked.push(session_id);
    }
    wait_for_trace_count(&trace_path, "prompt_submitted", capacity).await;

    let cancelled_id = blocked[0].clone();
    let cancelling_agent = Arc::clone(&agent);
    let mut cancellation =
        tokio::spawn(async move { cancelling_agent.cancel_turn(&cancelled_id).await });
    wait_for_trace_count(&trace_path, "cancel", 1).await;

    let next = tokio::time::timeout(Duration::from_secs(1), agent.create_session(json!({})))
        .await
        .expect("cancellation settlement blocked independent session creation")
        .unwrap();
    let next_id = next["thread"]["id"].as_str().unwrap().to_owned();
    let next_events = agent.subscribe_thread(&next_id);
    let queued_agent = Arc::clone(&agent);
    let queued_id = next_id.clone();
    let mut queued = tokio::spawn(async move {
        queued_agent
            .start_turn(json!({
                "threadId":queued_id,
                "priority":"user",
                "input":"COMPLETE AFTER TIMEOUT"
            }))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut queued)
            .await
            .is_err(),
        "turn started before cancellation settlement timed out"
    );

    let error = tokio::time::timeout(Duration::from_secs(3), &mut cancellation)
        .await
        .expect("ignored cancellation did not reach its settlement timeout")
        .expect("cancellation task failed")
        .expect_err("ignored cancellation unexpectedly settled");
    assert!(error.to_string().contains("did not settle within"));
    tokio::time::timeout(Duration::from_secs(1), &mut queued)
        .await
        .expect("queued turn did not recover released capacity")
        .expect("queued turn task failed")
        .expect("queued turn start failed");
    assert_eq!(
        recv(&next_events).await["params"]["delta"],
        "GROK_ACP_STREAM_OK"
    );
    assert_eq!(
        recv(&next_events).await["params"]["turn"]["status"],
        "completed"
    );

    let invalidated = agent
        .start_turn(json!({
            "threadId":blocked[0],
            "priority":"user",
            "input":"MUST NOT REUSE"
        }))
        .await
        .expect_err("timed-out session was reused");
    assert!(invalidated.to_string().contains("was invalidated"));
}

async fn spawn_mock(model: &str, cwd: &Path) -> std::sync::Arc<GrokAcp> {
    GrokAcp::spawn_with_program(model, grok_mock_program(cwd), cwd.to_owned())
        .await
        .expect("start Grok ACP mock")
}

async fn recv(events: &claudex_agent_adapter::app_server::ThreadEvents) -> Value {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("ACP event timeout")
        .expect("ACP event stream closed")
}

async fn recv_until_completed(
    events: &claudex_agent_adapter::app_server::ThreadEvents,
) -> Vec<Value> {
    let mut received = Vec::new();
    loop {
        let event = recv(events).await;
        let completed = event["method"] == "turn/completed";
        received.push(event);
        if completed {
            return received;
        }
    }
}

fn read_trace(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("read ACP trace")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse ACP trace"))
        .collect()
}

async fn wait_for_trace_count(path: &Path, key: &str, expected: usize) -> Vec<Value> {
    tokio::time::timeout(
        Duration::from_secs(2),
        poll_trace_count(path, key, expected),
    )
    .await
    .expect("ACP trace event timeout")
}

async fn poll_trace_count(path: &Path, key: &str, expected: usize) -> Vec<Value> {
    loop {
        let trace = try_read_trace(path).unwrap_or_default();
        if trace
            .iter()
            .filter(|event| event.get(key).is_some())
            .count()
            >= expected
        {
            return trace;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn try_read_trace(path: &Path) -> Option<Vec<Value>> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents
        .lines()
        .map(|line| serde_json::from_str(line).ok())
        .collect()
}
