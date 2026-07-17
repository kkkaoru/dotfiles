use std::{
    ffi::OsString,
    fs,
    net::TcpListener,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::Duration,
};

use reqwest::Client;
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starts_each_provider_once_only_after_its_first_parallel_request() {
    let home = tempfile::tempdir().expect("create runtime home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(
        home.path().join(".codex/auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write mock auth");
    let codex_count = home.path().join("codex-starts");
    let grok_count = home.path().join("grok-starts");
    let codex = provider_wrapper(
        home.path(),
        "codex-wrapper",
        env!("CARGO_BIN_EXE_routing-codex-mock"),
        &codex_count,
    );
    let grok = provider_wrapper(
        home.path(),
        "grok-wrapper",
        env!("CARGO_BIN_EXE_grok-acp-mock"),
        &grok_count,
    );
    let port = unused_port();
    let listen = format!("127.0.0.1:{port}");
    let mut server = command(&[
        "serve",
        "--model",
        "gpt-model",
        "--backend-route",
        "gpt-model=codex-app-server",
        "--backend-route",
        "grok-model=grok-acp",
        "--listen",
        &listen,
    ])
    .current_dir(home.path())
    .env("HOME", home.path())
    .env("ANTHROPIC_AUTH_TOKEN", "runtime-token")
    .env("CLAUDEX_CODEX_PROGRAM", codex)
    .env("CLAUDEX_GROK_PROGRAM", grok)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .expect("start runtime server");
    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{port}");
    let initial = read_health(&client, &base_url).await;
    assert_eq!(initial["started_models"], json!([]));
    assert_eq!(start_count(&codex_count), 0);
    assert_eq!(start_count(&grok_count), 0);

    request_burst(&base_url, "gpt-model", "CODEX_ROUTED_OK").await;
    assert_eq!(
        read_health(&client, &base_url).await["started_models"],
        json!(["gpt-model"])
    );
    assert_eq!(start_count(&codex_count), 1);
    assert_eq!(start_count(&grok_count), 0);

    request_burst(&base_url, "grok-model", "GROK_ACP_STREAM_OK").await;
    assert_eq!(
        read_health(&client, &base_url).await["started_models"],
        json!(["gpt-model", "grok-model"])
    );
    assert_eq!(start_count(&codex_count), 1);
    assert_eq!(start_count(&grok_count), 1);
    stop(&mut server);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_first_request_does_not_restart_the_provider() {
    let home = tempfile::tempdir().expect("create cancellation runtime home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(home.path().join(".codex/auth.json"), "{}").expect("write mock auth");
    let starts = home.path().join("cancelled-starts");
    let codex = write_provider_wrapper(
        home.path(),
        "slow-codex-wrapper",
        env!("CARGO_BIN_EXE_routing-codex-mock"),
        &starts,
        "sleep 0.5\n",
    );
    let port = unused_port();
    let listen = format!("127.0.0.1:{port}");
    let mut server = command(&["serve", "--model", "gpt-model", "--listen", &listen])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "runtime-token")
        .env("CLAUDEX_CODEX_PROGRAM", codex)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start cancellation runtime server");
    let base_url = format!("http://127.0.0.1:{port}");
    read_health(&Client::new(), &base_url).await;

    let cancelled_url = base_url.clone();
    let first = tokio::spawn(async move { provider_request(&cancelled_url, "gpt-model", 0).await });
    wait_for_start(&starts).await;
    first.abort();
    assert!(first.await.is_err());

    let response = provider_request(&base_url, "gpt-model", 1).await;
    assert_eq!(
        response.pointer("/content/0/text"),
        Some(&json!("CODEX_ROUTED_OK"))
    );
    assert_eq!(start_count(&starts), 1);
    stop(&mut server);
}

async fn request_burst(base_url: &str, model: &'static str, expected: &'static str) {
    let mut requests = tokio::task::JoinSet::new();
    for index in 0..20 {
        let base_url = base_url.to_owned();
        requests.spawn(async move {
            let response = provider_request(&base_url, model, index).await;
            assert_eq!(response.pointer("/content/0/text"), Some(&json!(expected)));
        });
    }
    while let Some(result) = requests.join_next().await {
        result.expect("parallel provider request");
    }
}

async fn provider_request(base_url: &str, model: &str, index: usize) -> Value {
    Client::new()
        .post(format!("{base_url}/v1/messages"))
        .bearer_auth("runtime-token")
        .json(&json!({
            "model":model,
            "max_tokens":128,
            "messages":[{"role":"user","content":format!("request {index}")}]
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn read_health(client: &Client, base_url: &str) -> Value {
    for _ in 0..100 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && let Ok(response) = response.error_for_status()
            && let Ok(value) = response.json().await
        {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("runtime server did not become healthy")
}

fn provider_wrapper(root: &Path, name: &str, target: &str, count: &Path) -> std::path::PathBuf {
    write_provider_wrapper(root, name, target, count, "")
}

fn write_provider_wrapper(
    root: &Path,
    name: &str,
    target: &str,
    count: &Path,
    before_exec: &str,
) -> std::path::PathBuf {
    let wrapper = root.join(name);
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf 'start\\n' >> '{}'\n{before_exec}exec '{}' \"$@\"\n",
            count.display(),
            target
        ),
    )
    .expect("write provider wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
        .expect("make provider wrapper executable");
    wrapper
}

async fn wait_for_start(path: &Path) {
    for _ in 0..100 {
        if start_count(path) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("provider process did not start")
}

fn start_count(path: &Path) -> usize {
    fs::read_to_string(path).map_or(0, |contents| contents.lines().count())
}

fn stop(child: &mut Child) {
    child.kill().expect("stop runtime server");
    child.wait().expect("reap runtime server");
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("read ephemeral port")
        .port()
}

#[test]
fn reports_cli_and_server_configuration_errors() {
    assert_command_error(command(&["launch"]), "--model is required");
    assert_command_error(command(&["launch", "--model", "model"]), "requires `--`");
    assert_command_error(command(&[]), "command is required");
    assert_command_error(
        command(&["serve", "--model", "model", "--listen", "not-a-listener"]),
        "invalid --listen address",
    );
    assert_command_error(
        command(&[
            "serve",
            "--model",
            "model",
            "--subscription-max-processes",
            "0",
        ]),
        "positive integer",
    );
    assert_command_error(
        command(&[
            "serve",
            "--model",
            "model",
            "--subscription-timeout-minutes",
            "0",
        ]),
        "positive integer",
    );
    assert_command_error(
        command(&[
            "serve",
            "--model",
            "model",
            "--subscription-max-processes",
            "18446744073709551615",
        ]),
        "out of range",
    );
    assert_command_error(
        command(&[
            "serve",
            "--model",
            "model",
            "--subscription-timeout-minutes",
            "18446744073709551615",
        ]),
        "out of range",
    );
    let exposed = command(&["serve", "--model", "model", "--listen", "0.0.0.0:8318"]);
    assert_command_error(exposed, "ANTHROPIC_AUTH_TOKEN is required");
    let unknown = command(&["unknown"]);
    assert_command_error(unknown, "unknown command");
    assert_command_error(command(&["build-id", "extra"]), "unexpected arguments");
}

#[test]
fn rejects_a_non_utf8_run_claude_model() {
    let output = command(&["launch", "--model"])
        .arg(OsString::from_vec(vec![0xff]))
        .output()
        .expect("run invalid UTF-8 command");
    assert_error(
        output,
        "value for adapter option --model must be valid UTF-8",
    );

    let output = command(&["serve", "--model", "model"])
        .arg(OsString::from_vec(vec![0xff]))
        .output()
        .expect("run invalid UTF-8 adapter option");
    assert_error(output, "adapter options must be valid UTF-8");
}

fn command(arguments: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"));
    command.args(arguments).env_remove("ANTHROPIC_AUTH_TOKEN");
    command
}

fn assert_error(output: Output, expected: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_command_error(mut command: Command, expected: &str) {
    assert_error(command.output().expect("run invalid command"), expected);
}
