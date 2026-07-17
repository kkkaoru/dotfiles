use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use reqwest::Client;
use serde_json::Value;
use tempfile::TempDir;

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
async fn ensure_running_replaces_an_unavailable_health_endpoint() {
    let home = launcher_home();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stale endpoint");
    let port = listener
        .local_addr()
        .expect("stale endpoint address")
        .port();
    let stale = thread::spawn(move || serve_stale_health(listener, 2, unavailable_health()));
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
    assert_error(missing_model, "--model is required");

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
    let port = unused_port();
    let claude = home.path().join("claude-mock");
    fs::write(
        &claude,
        r#"#!/bin/sh
printf 'args=%s\n' "$*"
printf 'base=%s effort=%s subagent=%s\n' "$ANTHROPIC_BASE_URL" "$CLAUDE_CODE_ALWAYS_ENABLE_EFFORT" "${CLAUDE_CODE_SUBAGENT_MODEL-unset}"
printf 'api_key=%s anthropic_model=%s bedrock=%s foundry=%s vertex=%s\n' \
    "${ANTHROPIC_API_KEY-unset}" "${ANTHROPIC_MODEL-unset}" \
    "${CLAUDE_CODE_USE_BEDROCK-unset}" "${CLAUDE_CODE_USE_FOUNDRY-unset}" \
    "${CLAUDE_CODE_USE_VERTEX-unset}"
printf "Advisor disabled — base model 'test-main-model' has no advisor rank\n" >&2
printf 'kept stderr\n' >&2
exit 23
"#,
    )
    .expect("write Claude mock");
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755))
        .expect("make Claude mock executable");

    let output = common_command(&home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args(["--subscription-timeout-minutes", "120", "--"])
        .arg("--continue")
        .env("CLAUDEX_CLAUDE_PROGRAM", &claude)
        .env("CLAUDE_CODE_ALWAYS_ENABLE_EFFORT", "configured-by-fish")
        .env("CLAUDE_CODE_SUBAGENT_MODEL", "wrong-model")
        .env("ANTHROPIC_API_KEY", "must-not-leak")
        .env("ANTHROPIC_MODEL", "must-not-override")
        .env("CLAUDE_CODE_USE_BEDROCK", "1")
        .env("CLAUDE_CODE_USE_FOUNDRY", "1")
        .env("CLAUDE_CODE_USE_VERTEX", "1")
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
    let stderr = String::from_utf8(output.stderr).expect("Claude stderr");
    assert_eq!(stderr, "kept stderr\n");

    let base_url = format!("http://127.0.0.1:{port}");
    let pid = health(&Client::new(), &base_url).await["pid"]
        .as_u64()
        .expect("wrapper daemon pid");
    terminate(pid);

    let rejected = common_command(&home, port, "20")
        .args([
            "launch",
            "--model",
            "test-main-model",
            "--",
            "--model",
            "other",
        ])
        .env("CLAUDEX_CLAUDE_PROGRAM", &claude)
        .output()
        .expect("reject duplicate model");
    assert_error(rejected, "pass the main model to adapter option --model");
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
    for _ in 0..20 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && let Ok(response) = response.error_for_status()
            && let Ok(value) = response.json().await
        {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("adapter health did not become readable")
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

fn assert_error(output: std::process::Output, expected: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
