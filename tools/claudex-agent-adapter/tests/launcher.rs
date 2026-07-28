use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use reqwest::Client;
use serde_json::Value;
use tempfile::TempDir;

const ACTIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const FILE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CONCURRENT_LAUNCHERS: usize = 4;
const DAEMON_START_MARKER: &str = "=== claudex-agent-adapter daemon start ===";
const REPLACEMENT_READY_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn protocol_compatible_build_replacement_preserves_an_active_response() {
    let home = launcher_home();
    let port = unused_port();
    let stale_binary = home.path().join("stale/claudex-agent-adapter");
    fs::create_dir_all(stale_binary.parent().expect("stale binary directory"))
        .expect("create stale binary directory");
    fs::copy(env!("CARGO_BIN_EXE_stale-adapter-mock"), &stale_binary)
        .expect("copy stale adapter fixture");
    fs::set_permissions(&stale_binary, fs::Permissions::from_mode(0o755))
        .expect("make stale adapter executable");
    let entered = home.path().join("slow-entered");
    let release = home.path().join("slow-release");
    let mut stale = Command::new(&stale_binary)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .args(["--entered", entered.to_str().expect("entered path")])
        .args(["--release", release.to_str().expect("release path")])
        .spawn()
        .expect("start stale adapter fixture");
    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{port}");
    let stale_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("stale adapter pid");
    let slow = tokio::spawn({
        let client = client.clone();
        let base_url = base_url.clone();
        async move {
            client
                .get(format!("{base_url}/slow"))
                .send()
                .await
                .expect("active response")
                .text()
                .await
                .expect("active response body")
        }
    });
    wait_for_file(&entered).await;

    let mut ensure = ensure_command(&home, port, "20");
    let output = tokio::time::timeout(
        ACTIVE_REQUEST_TIMEOUT,
        tokio::task::spawn_blocking(move || ensure.output()),
    )
    .await
    .expect("replacement must become ready while the old response is active")
    .expect("ensure task")
    .expect("replace compatible stale build");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replacement_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("replacement adapter pid");
    assert_ne!(replacement_pid, stale_pid);
    assert!(!slow.is_finished());
    assert!(stale.try_wait().expect("inspect stale adapter").is_none());

    fs::write(&release, "release").expect("release active response");
    assert_eq!(slow.await.expect("active response task"), "complete");
    assert!(stale.wait().expect("reap stale adapter").success());
    terminate(replacement_pid);
    wait_for_exit(&client, &base_url).await;
}

#[tokio::test]
async fn concurrent_ensure_commands_start_exactly_one_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let barrier = Arc::new(Barrier::new(CONCURRENT_LAUNCHERS));
    let workers = (0..CONCURRENT_LAUNCHERS)
        .map(|_| ensure_command(&home, port, "20"))
        .map(|mut command| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                command.output().expect("run concurrent ensure")
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        let output = worker.join().expect("concurrent ensure worker");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert_eq!(daemon_start_count(&home), 1);
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("concurrently launched daemon pid");
    terminate(pid);
    wait_for_exit(&client, &base_url).await;
}

#[tokio::test]
async fn ensure_running_starts_reuses_and_replaces_the_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let first = ensure_command(&home, port, "20")
        .output()
        .expect("run ensure command");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let base_url = String::from_utf8(first.stdout)
        .expect("base URL output")
        .trim()
        .to_owned();
    assert_eq!(base_url, format!("http://127.0.0.1:{port}"));

    let client = Client::new();
    let initial = health(&client, &base_url).await;
    assert_eq!(initial["subscription_max_processes"], 20);
    let first_pid = initial["pid"].as_u64().expect("initial daemon pid");

    let reused = ensure_command(&home, port, "20")
        .output()
        .expect("reuse ensure command");
    assert!(reused.status.success());
    assert_eq!(
        health(&client, &base_url).await["pid"].as_u64(),
        Some(first_pid)
    );

    let authenticated = ensure_command(&home, port, "20")
        .env("ANTHROPIC_AUTH_TOKEN", "changed-token")
        .output()
        .expect("replace daemon after token change");
    assert!(authenticated.status.success());
    let authenticated_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("authenticated daemon pid");
    assert_ne!(authenticated_pid, first_pid);

    let replaced = ensure_command(&home, port, "7")
        .env("ANTHROPIC_AUTH_TOKEN", "changed-token")
        .output()
        .expect("replace ensure command");
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    let changed = health(&client, &base_url).await;
    assert_eq!(changed["subscription_max_processes"], 7);
    let replacement_pid = changed["pid"].as_u64().expect("replacement daemon pid");
    assert_ne!(replacement_pid, authenticated_pid);
    terminate(replacement_pid);
    wait_for_exit(&client, &base_url).await;
}

#[tokio::test]
async fn ensure_running_replaces_the_renamed_legacy_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let current_binary = home.path().join("claudex-agent-adapter");
    let legacy_binary = home.path().join("claudex-app-server-adapter");
    for binary in [&current_binary, &legacy_binary] {
        fs::copy(env!("CARGO_BIN_EXE_claudex-agent-adapter"), binary)
            .expect("copy adapter under an installed name");
        fs::set_permissions(binary, fs::Permissions::from_mode(0o755))
            .expect("make copied adapter executable");
    }

    let mut legacy = Command::new(&legacy_binary)
        .args(["serve", "--model", "legacy-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"))
        .spawn()
        .expect("start renamed legacy daemon");
    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{port}");
    let legacy_pid = health_with_deadline(
        &client,
        &base_url,
        Instant::now() + REPLACEMENT_READY_TIMEOUT,
    )
    .await["pid"]
        .as_u64()
        .expect("legacy daemon pid");
    // Do not retain an idle connection to the daemon while it drains during
    // handover. The replacement readiness probe below uses a fresh client.
    drop(client);

    let output = Command::new(&current_binary)
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args(["--subscription-timeout-minutes", "120"])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"))
        .output()
        .expect("replace renamed legacy daemon");
    if !output.status.success() {
        let _cleanup = legacy.kill();
        panic!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let replacement_client = Client::new();
    let replacement = replacement_health(&replacement_client, &base_url, legacy_pid).await;
    let _status = legacy.wait().expect("reap renamed legacy daemon");
    assert_eq!(replacement["model"], "test-main-model");
    assert_ne!(replacement["pid"].as_u64(), Some(legacy_pid));
    terminate(replacement["pid"].as_u64().expect("replacement daemon pid"));
    wait_for_exit(&replacement_client, &base_url).await;
}

#[tokio::test]
async fn ensure_running_replaces_an_unavailable_health_endpoint() {
    let home = launcher_home();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stale endpoint");
    let port = listener
        .local_addr()
        .expect("stale endpoint address")
        .port();
    let stale = thread::spawn(move || {
        serve_stale_health_after_releasing_listener(listener, unavailable_health())
    });
    let output = ensure_command(&home, port, "20")
        .output()
        .expect("replace unavailable endpoint");
    stale.join().expect("stale endpoint thread");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let pid = health(&Client::new(), &base_url).await["pid"]
        .as_u64()
        .expect("replacement daemon pid");
    terminate(pid);
}

#[tokio::test]
async fn ensure_running_replaces_a_protocol_stale_foreign_endpoint() {
    let home = launcher_home();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stale endpoint");
    let port = listener
        .local_addr()
        .expect("stale endpoint address")
        .port();
    let body = format!(
        r#"{{"status":"ok","pid":{},"protocol_version":0,"build_id":"stale","model":"test-main-model","subscription_max_processes":20,"subscription_timeout_minutes":120}}"#,
        std::process::id()
    );
    let stale = thread::spawn(move || serve_stale_health(listener, 2, body));
    let output = ensure_command(&home, port, "20")
        .output()
        .expect("replace protocol-stale endpoint");
    stale.join().expect("stale endpoint thread");
    assert!(output.status.success());
    let base_url = format!("http://127.0.0.1:{port}");
    let pid = health(&Client::new(), &base_url).await["pid"]
        .as_u64()
        .expect("replacement daemon pid");
    terminate(pid);
}

#[test]
fn ensure_running_rejects_non_loopback_without_a_real_token() {
    let home = launcher_home();
    let output = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", "0.0.0.0:8318"])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .output()
        .expect("run rejected ensure command");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ANTHROPIC_AUTH_TOKEN is required"));
}

#[tokio::test]
async fn ensure_running_connects_through_loopback_for_an_exposed_listener() {
    let home = launcher_home();
    let port = unused_port();
    let output = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", &format!("0.0.0.0:{port}")])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "real-token")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"))
        .output()
        .expect("run exposed adapter");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("http://127.0.0.1:{port}")
    );
    let base_url = format!("http://127.0.0.1:{port}");
    let pid = health(&Client::new(), &base_url).await["pid"]
        .as_u64()
        .expect("exposed daemon pid");
    terminate(pid);
}

#[test]
fn ensure_running_reports_missing_or_invalid_environment() {
    let binary = env!("CARGO_BIN_EXE_claudex-agent-adapter");
    let missing_model = Command::new(binary)
        .arg("ensure")
        .output()
        .expect("run without model");
    assert_error(missing_model, "--model or --provider-config is required");

    let invalid_listen = Command::new(binary)
        .args([
            "ensure",
            "--model",
            "test-main-model",
            "--listen",
            "invalid",
        ])
        .output()
        .expect("run with invalid listener");
    assert_error(invalid_listen, "invalid --listen address");

    let missing_home = Command::new(binary)
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", "127.0.0.1:1"])
        .env_remove("HOME")
        .output()
        .expect("run without home");
    assert_error(missing_home, "HOME is required");

    let authenticated_exposed = Command::new(binary)
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", "0.0.0.0:8318"])
        .env("ANTHROPIC_AUTH_TOKEN", "real-token")
        .env_remove("HOME")
        .output()
        .expect("run exposed listener with authentication");
    assert_error(authenticated_exposed, "HOME is required");
}

#[tokio::test]
async fn run_claude_forwards_arguments_environment_stderr_and_status() {
    let home = launcher_home();
    let policy_directory = home.path().join(".config/claudex");
    fs::create_dir_all(&policy_directory).expect("create model policy directory");
    fs::write(
        policy_directory.join("disabled-subagent-models.json"),
        r#"{"version":1,"disabledModels":["configured-model"]}"#,
    )
    .expect("write model policy");
    let port = unused_port();
    let claude = home.path().join("claude");
    fs::write(
        &claude,
        r#"#!/bin/sh
printf 'args=%s\n' "$*"
printf 'base=%s effort=%s subagent=%s\n' "$ANTHROPIC_BASE_URL" "$CLAUDE_CODE_ALWAYS_ENABLE_EFFORT" "${CLAUDE_CODE_SUBAGENT_MODEL-unset}"
printf 'api_key=%s anthropic_model=%s bedrock=%s foundry=%s vertex=%s\n' \
    "${ANTHROPIC_API_KEY-unset}" "${ANTHROPIC_MODEL-unset}" \
    "${CLAUDE_CODE_USE_BEDROCK-unset}" "${CLAUDE_CODE_USE_FOUNDRY-unset}" \
    "${CLAUDE_CODE_USE_VERTEX-unset}"
printf 'custom_headers=%s\n' "$ANTHROPIC_CUSTOM_HEADERS"
printf 'resolved_models=%s\n' "$CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS"
printf "Advisor disabled — base model 'test-main-model' has no advisor rank\n" >&2
printf 'kept stderr\n' >&2
exit 23
"#,
    )
    .expect("write Claude mock");
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755))
        .expect("make Claude mock executable");
    let path = format!(
        "{}:{}",
        home.path().display(),
        std::env::var("PATH").expect("PATH")
    );

    let output = common_command(&home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args(["--subscription-timeout-minutes", "120", "--"])
        .arg("--continue")
        .env("PATH", &path)
        .env("CLAUDE_CODE_ALWAYS_ENABLE_EFFORT", "configured-by-fish")
        .env("CLAUDE_CODE_SUBAGENT_MODEL", "wrong-model")
        .env("ANTHROPIC_API_KEY", "must-not-leak")
        .env("ANTHROPIC_MODEL", "must-not-override")
        .env("CLAUDE_CODE_USE_BEDROCK", "1")
        .env("CLAUDE_CODE_USE_FOUNDRY", "1")
        .env("CLAUDE_CODE_USE_VERTEX", "1")
        .env(
            "ANTHROPIC_CUSTOM_HEADERS",
            "x-user-header: keep\nx-claudex-working-directory: forged\nx-claudex-disabled-subagent-models: forged",
        )
        .env(
            "CLAUDEX_DISABLED_SUBAGENT_MODELS",
            "grok-4.5,gpt-5.6-sol,grok-4.5",
        )
        .env("CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS", "forged")
        .output()
        .expect("run Claude wrapper");
    assert_eq!(output.status.code(), Some(23));
    let stdout = String::from_utf8(output.stdout).expect("Claude stdout");
    assert!(stdout.contains("args=--model test-main-model --continue"));
    assert!(stdout.contains("effort=configured-by-fish subagent=unset"));
    assert!(
        stdout.contains(
            "api_key=unset anthropic_model=unset bedrock=unset foundry=unset vertex=unset"
        )
    );
    assert_terminal_policy_headers(&stdout);
    assert!(stdout.contains("resolved_models=configured-model,gpt-5.6-sol,grok-4.5"));
    let stderr = String::from_utf8(output.stderr).expect("Claude stderr");
    assert_eq!(stderr, "kept stderr\n");

    assert_inherited_launch(&home, port, &path);

    let base_url = format!("http://127.0.0.1:{port}");
    let pid = health(&Client::new(), &base_url).await["pid"]
        .as_u64()
        .expect("wrapper daemon pid");
    terminate(pid);

    assert_duplicate_model_is_rejected(&home, port, &claude);
    assert_invalid_subagent_policy_is_rejected(&home, port, &claude);
}

fn assert_duplicate_model_is_rejected(home: &TempDir, port: u16, claude: &std::path::Path) {
    let output = common_command(home, port, "20")
        .args([
            "launch",
            "--model",
            "test-main-model",
            "--",
            "--model",
            "other",
        ])
        .env("CLAUDEX_CLAUDE_PROGRAM", claude)
        .output()
        .expect("reject duplicate model");
    assert_error(output, "pass the main model to adapter option --model");
}

fn assert_terminal_policy_headers(output: &str) {
    assert!(output.contains("custom_headers=x-user-header: keep"));
    assert!(output.contains("x-claudex-working-directory:"));
    assert!(
        output
            .contains("x-claudex-disabled-subagent-models: configured-model,gpt-5.6-sol,grok-4.5")
    );
    assert!(!output.contains("forged"));
}

fn assert_invalid_subagent_policy_is_rejected(home: &TempDir, port: u16, claude: &std::path::Path) {
    let output = common_command(home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--", "--continue"])
        .env("CLAUDEX_CLAUDE_PROGRAM", claude)
        .env("CLAUDEX_DISABLED_SUBAGENT_MODELS", "model with spaces")
        .output()
        .expect("reject invalid terminal model policy");
    assert_error(output, "contains an invalid model ID");
}

fn assert_inherited_launch(home: &TempDir, port: u16, path: &str) {
    let inherited = common_command(home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args([
            "--subscription-timeout-minutes",
            "120",
            "--inherit-claude-model",
            "--",
            "--agent",
            "claudex-orchestrator",
        ])
        .env("PATH", path)
        .output()
        .expect("run Claude wrapper with inherited model");
    assert_eq!(inherited.status.code(), Some(23));
    let inherited_stdout = String::from_utf8(inherited.stdout).expect("Claude stdout");
    assert!(inherited_stdout.contains("args=--agent claudex-orchestrator"));
    assert!(!inherited_stdout.contains("args=--model"));
}

fn ensure_command(home: &TempDir, port: u16, max_processes: &str) -> Command {
    let mut command = common_command(home, port, max_processes);
    command
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", max_processes])
        .args(["--subscription-timeout-minutes", "120"]);
    command
}

fn common_command(home: &TempDir, _port: u16, _max_processes: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"));
    command
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"));
    command
}

fn launcher_home() -> TempDir {
    let home = tempfile::tempdir().expect("create launcher home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(
        home.path().join(".codex/auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write mock auth");
    home
}

async fn health(client: &Client, base_url: &str) -> Value {
    for _ in 0..120 {
        let Ok(response) = client.get(format!("{base_url}/health")).send().await else {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        };
        let Ok(response) = response.error_for_status() else {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        };
        let Ok(value) = response.json().await else {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        };
        return value;
    }
    panic!("adapter health did not become readable")
}

async fn replacement_health(client: &Client, base_url: &str, legacy_pid: u64) -> Value {
    let deadline = Instant::now() + REPLACEMENT_READY_TIMEOUT;
    loop {
        if let Some(value) = fetch_test_health(client, base_url).await {
            if value["pid"].as_u64() != Some(legacy_pid) {
                return value;
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25).min(remaining)).await;
    }
    panic!("replacement adapter health did not become readable")
}

async fn health_with_deadline(client: &Client, base_url: &str, deadline: Instant) -> Value {
    loop {
        if let Some(value) = fetch_test_health(client, base_url).await {
            return value;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25).min(remaining)).await;
    }
    panic!("adapter health did not become readable")
}

async fn fetch_test_health(client: &Client, base_url: &str) -> Option<Value> {
    let response = client
        .get(format!("{base_url}/health"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    response.json().await.ok()
}

fn terminate(pid: u64) {
    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .expect("terminate daemon");
    assert!(status.success());
}

async fn wait_for_exit(client: &Client, base_url: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if client
            .get(format!("{base_url}/health"))
            .send()
            .await
            .is_err()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("adapter daemon did not exit");
}

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(ACTIVE_REQUEST_TIMEOUT, async {
        while !path.exists() {
            tokio::time::sleep(FILE_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("active request marker");
}

fn daemon_start_count(home: &TempDir) -> usize {
    fs::read_dir(home.path().join(".cache/claudex"))
        .expect("read launcher cache")
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .map(|contents| contents.matches(DAEMON_START_MARKER).count())
        .sum()
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("read ephemeral port")
        .port()
}

fn unavailable_health() -> String {
    r#"{"status":"unavailable","pid":null,"protocol_version":0,"build_id":"stale","model":"stale","subscription_max_processes":0,"subscription_timeout_minutes":0}"#.to_owned()
}

fn serve_stale_health(listener: TcpListener, responses: usize, body: String) {
    for _ in 0..responses {
        let (mut stream, _) = listener.accept().expect("accept health request");
        let mut request = [0_u8; 1024];
        let _bytes = stream.read(&mut request).expect("read health request");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write health response");
    }
}

fn serve_stale_health_after_releasing_listener(listener: TcpListener, body: String) {
    let (mut stream, _) = listener.accept().expect("accept health request");
    let mut request = [0_u8; 1024];
    let _bytes = stream.read(&mut request).expect("read health request");
    // `ensure` starts the replacement as soon as it receives this unavailable
    // health response. Release the port first so fixture scheduling cannot make
    // the replacement race a still-bound stale listener.
    drop(listener);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write health response");
}

fn assert_error(output: std::process::Output, expected: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
