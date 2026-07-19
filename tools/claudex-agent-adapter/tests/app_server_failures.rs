use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use claudex_agent_adapter::app_server::AppServer;
use serde_json::json;

struct Fixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    isolated: PathBuf,
    program: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let root = tempfile::tempdir().expect("create app-server fixture");
        let source = root.path().join("source");
        let isolated = root.path().join("isolated");
        let program = root.path().join("app-server");
        std::fs::create_dir(&source).expect("create source home");
        std::fs::write(source.join("auth.json"), "{}").expect("write auth");
        std::fs::write(&program, format!("#!/bin/sh\n{body}")).expect("write fixture program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("make fixture executable");
        Self {
            _root: root,
            source,
            isolated,
            program,
        }
    }

    async fn spawn(&self) -> std::sync::Arc<AppServer> {
        AppServer::spawn_with_program("test-model", &self.program, &self.source, &self.isolated)
            .await
            .expect("spawn fixture app-server")
    }
}

const INITIALIZE: &str = r#"
read line
printf '%s\n' '{"id":1,"result":{"userAgent":"fixture"}}'
read line
"#;

#[tokio::test]
async fn dispatches_success_error_and_ignores_malformed_messages() {
    let fixture = Fixture::new(&format!(
        r#"{INITIALIZE}
printf '%s\n' 'not-json'
printf '%s\n' '{{}}'
printf '%s\n' '{{"id":"not-numeric","result":{{}}}}'
printf '%s\n' '{{"id":999,"result":{{}}}}'
read line
printf '%s\n' '{{"method":"turn/completed","params":{{"threadId":"observed","turn":{{"status":"completed"}}}}}}'
printf '%s\n' '{{"id":2,"result":{{"value":"ok"}}}}'
read line
printf '%s\n' '{{"id":3,"error":{{"code":-32000,"message":"forced"}}}}'
while read line; do :; done
"#
    ));
    let server = fixture.spawn().await;
    let events = server.subscribe_thread("observed");

    assert_eq!(
        server.request("fixture/success", json!({})).await.unwrap(),
        json!({"value":"ok"})
    );
    assert_eq!(events.recv().await.unwrap()["method"], "turn/completed");
    let error = server
        .request("fixture/error", json!({}))
        .await
        .expect_err("JSON-RPC error must propagate");
    assert!(error.to_string().contains("forced"));
}

#[tokio::test]
async fn detached_errors_are_delivered_to_the_matching_thread() {
    let fixture = Fixture::new(&format!(
        r#"{INITIALIZE}
read line
printf '%s\n' '{{"id":2,"error":{{"code":-32001,"message":"detached failure"}}}}'
while read line; do :; done
"#
    ));
    let server = fixture.spawn().await;
    let events = server.subscribe_thread("thread-detached");

    server
        .request_detached("turn/start", json!({"threadId":"thread-detached"}))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("detached error event")
        .expect("event dispatcher remains open");
    assert_eq!(event["method"], "error");
    assert!(
        event["params"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("detached failure")
    );
}

#[tokio::test]
async fn closing_output_fails_pending_requests_and_closes_subscribers() {
    let fixture = Fixture::new(&format!(
        r"{INITIALIZE}
read line
"
    ));
    let server = fixture.spawn().await;
    let events = server.subscribe_thread("closed");

    let error = server
        .request("fixture/close", json!({}))
        .await
        .expect_err("closed output must fail pending request");
    assert!(error.to_string().contains("closed its output"));
    assert!(!server.is_alive());
    assert!(events.recv().await.is_none());
    assert!(
        server
            .request("fixture/write-after-close", json!({}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn closing_output_reports_a_detached_turn_before_closing_subscribers() {
    let fixture = Fixture::new(&format!(
        r"{INITIALIZE}
read line
"
    ));
    let server = fixture.spawn().await;
    let events = server.subscribe_thread("detached-close");

    server
        .request_detached("turn/start", json!({"threadId":"detached-close"}))
        .await
        .expect("flush detached request");
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("provider exit event")
        .expect("queued error before close");
    assert_eq!(event["method"], "error");
    assert!(
        event["params"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("closed its output")
    );
    assert!(events.recv().await.is_none());
}

#[tokio::test]
async fn initialization_fails_when_response_channel_closes() {
    let fixture = Fixture::new("read line\n");
    let error = AppServer::spawn_with_program(
        "test-model",
        &fixture.program,
        &fixture.source,
        &fixture.isolated,
    )
    .await
    .err()
    .expect("closed initialization channel must fail");
    assert!(error.to_string().contains("initialization failed"));
}

#[tokio::test]
async fn initialization_fails_when_acknowledgement_cannot_be_written() {
    let fixture = Fixture::new(
        r#"
read line
exec 0<&-
printf '%s\n' '{"id":1,"result":{}}'
sleep 1
"#,
    );
    let error = AppServer::spawn_with_program(
        "test-model",
        &fixture.program,
        &fixture.source,
        &fixture.isolated,
    )
    .await
    .err()
    .expect("initialized acknowledgement must fail");
    assert!(
        error
            .to_string()
            .contains("acknowledge app-server initialization")
    );
}
