use std::{
    ffi::OsString,
    fs,
    net::TcpListener,
    os::unix::ffi::OsStringExt,
    process::{Child, Command, Output, Stdio},
    time::Duration,
};

use reqwest::Client;

#[tokio::test]
async fn serves_with_the_configured_mock_app_server() {
    let home = tempfile::tempdir().expect("create runtime home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(
        home.path().join(".codex/auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write mock auth");
    let port = unused_port();
    let listen = format!("127.0.0.1:{port}");
    let mut server = command(&["serve", "--model", "test-main-model", "--listen", &listen])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "runtime-token")
        .env(
            "CLAUDEX_APP_SERVER_PROGRAM",
            env!("CARGO_BIN_EXE_codex-mock"),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start runtime server");
    let client = Client::new();
    let url = format!("http://127.0.0.1:{port}/health");
    for _ in 0..100 {
        if client
            .get(&url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            stop(&mut server);
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stop(&mut server);
    panic!("runtime server did not become healthy");
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
    let mut authenticated = command(&["serve", "--model", "model", "--listen", "0.0.0.0:8318"]);
    authenticated
        .env("ANTHROPIC_AUTH_TOKEN", "real-token")
        .env("CLAUDEX_APP_SERVER_PROGRAM", "/missing/codex");
    assert_command_error(authenticated, "failed to start");

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
    let mut command = Command::new(env!("CARGO_BIN_EXE_claudex-app-server-adapter"));
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
