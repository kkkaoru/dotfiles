#![cfg_attr(coverage_nightly, coverage(off))]

use axum::body::to_bytes;
use std::os::unix::fs::PermissionsExt;

use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn messages_wrapper_forwards_tool_presence_to_the_inner_router() {
    let root = tempfile::tempdir().expect("message wrapper fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nwhile IFS= read -r line; do id=$(printf '%s\\n' \"$line\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p'); printf '{\"id\":%s,\"result\":{}}\\n' \"$id\"; done\n",
    )
    .expect("app-server fixture");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("make app-server fixture executable");
    let app = crate::app_server::AppServer::spawn_with_program(
        "main",
        &program,
        &source,
        &root.path().join("isolated"),
    )
    .await
    .expect("start app-server fixture");
    let bridge = std::sync::Arc::new(Bridge::new_with_backend(
        crate::agent_backend::AgentBackend::codex(app),
        "main".to_owned(),
    ));
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"user","content":"hello"})],
        tools: vec![json!({"name":"Read"})],
        stream: true,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(2), bridge.messages(request))
            .await
            .expect("message wrapper should not hang")
            .expect("streaming response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn internal_agent_notification_is_acknowledged_without_provider_turn() {
    let (_root, log, bridge) = message_fixture().await;
    let before = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_log_marker(&log, "\"method\":\"initialized\""),
    )
    .await
    .expect("provider fixture should finish initialization");
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        // Claude Code can append a transcript assistant element after a
        // lifecycle notification during resume. It must still be handled
        // as an internal acknowledgement, not a provider turn.
        messages: vec![
            json!({
                "role":"user",
                "content":[
                    {"type":"text","text":"<task-notification><status>completed</status></task-notification>"},
                    {"type":"text","text":"If this event is something the user would act on now, send a PushNotification."}
                ]
            }),
            json!({"role":"assistant","content":[{"type":"text","text":"acknowledged"}]}),
        ],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(2), bridge.messages(request))
            .await
            .expect("internal notification should return immediately")
            .expect("internal notification response");
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read internal notification response");
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 response");
    let after = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(before, after, "notification must not start a provider turn");
    assert!(!body.contains("agent-message"));
    assert!(body.contains("\"stop_reason\":\"end_turn\""));
}

#[tokio::test]
async fn mixed_user_history_drops_agent_messages_before_provider_turn() {
    let (_root, log, bridge) = message_fixture().await;
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_log_marker(&log, "\"method\":\"initialized\""),
    )
    .await
    .expect("provider fixture should finish initialization");
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![
            json!({"role":"user","content":"first instruction"}),
            json!({"role":"user","content":"<agent-message from=\"general-purpose\">queued result</agent-message>"}),
            json!({"role":"user","content":[
                {"type":"text","text":"<task-notification>queued completion</task-notification>"},
                {"type":"text","text":"latest user instruction"}
            ]}),
        ],
        tools: Vec::new(),
        stream: true,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let _response =
        tokio::time::timeout(std::time::Duration::from_secs(2), bridge.messages(request))
            .await
            .expect("user turn should not wait for internal notifications")
            .expect("provider response");
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_log_marker(&log, "\"method\":\"turn/start\""),
    )
    .await
    .expect("provider should receive the real user turn");
    let provider_input = std::fs::read_to_string(&log).unwrap_or_default();
    let turn_line = provider_input
        .lines()
        .find(|line| line.contains("\"method\":\"turn/start\""))
        .expect("provider turn request");
    let turn: Value = serde_json::from_str(turn_line).expect("turn request JSON");
    let turn_input = turn["params"]["input"].to_string();
    assert!(
        !turn_input.contains("agent-message"),
        "turn input: {turn_input}"
    );
    assert!(
        !turn_input.contains("task-notification"),
        "turn input: {turn_input}"
    );
    assert!(turn_input.contains("latest user instruction"));
}

pub(super) async fn message_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::sync::Arc<Bridge>,
) {
    let root = tempfile::tempdir().expect("message notification fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let log = root.path().join("provider-input.log");
    let program = root.path().join("app-server");
    std::fs::write(
        &program,
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{}'
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
*'"method":"thread/start"'*) result='{{"thread":{{"id":"thread-1"}}}}' ;;
*) result='{{}}' ;;
  esac
  printf '{{"id":%s,"result":%s}}\n' "$id" "$result"
done
"#,
            log.display()
        ),
    )
    .expect("app-server fixture");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("make app-server fixture executable");
    let app = crate::app_server::AppServer::spawn_with_program(
        "main",
        &program,
        &source,
        &root.path().join("isolated"),
    )
    .await
    .expect("start app-server fixture");
    let bridge = std::sync::Arc::new(Bridge::new_with_backend(
        crate::agent_backend::AgentBackend::codex(app),
        "main".to_owned(),
    ));
    (root, log, bridge)
}

pub(super) async fn wait_for_log_marker(path: &std::path::Path, marker: &str) -> String {
    let mut current = String::new();
    while !current.contains(marker) {
        current = std::fs::read_to_string(path).unwrap_or_default();
        tokio::task::yield_now().await;
    }
    current
}
