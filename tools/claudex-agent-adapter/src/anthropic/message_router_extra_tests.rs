use axum::body::to_bytes;
use serde_json::{Value, json};

use super::{MessagesRequest, tests};

#[tokio::test]
async fn independent_notifications_are_acknowledged_one_by_one_without_batching() {
    let (_root, log, bridge) = tests::message_fixture().await;
    let before = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tests::wait_for_log_marker(&log, "\"method\":\"initialized\""),
    )
    .await
    .expect("provider fixture should finish initialization");

    let notification = |status: &str| MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({
            "role": "user",
            "content": format!(
                "<task-notification><status>{status}</status></task-notification>"
            )
        })],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    // Each Claude Code notification is its own request. Returning the first
    // acknowledgement before handling the second prevents a batch drain
    // from delaying the main session behind the slowest worker.
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        bridge.messages(notification("completed")),
    )
    .await
    .expect("first notification should return immediately")
    .expect("first notification response");
    let first_body = to_bytes(first.into_body(), usize::MAX)
        .await
        .expect("read first notification response");
    assert!(String::from_utf8_lossy(&first_body).contains("\"stop_reason\":\"end_turn\""));

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        bridge.messages(notification("stopped")),
    )
    .await
    .expect("second notification should return immediately")
    .expect("second notification response");
    let second_body = to_bytes(second.into_body(), usize::MAX)
        .await
        .expect("read second notification response");
    assert!(String::from_utf8_lossy(&second_body).contains("\"stop_reason\":\"end_turn\""));

    let after = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        before, after,
        "notifications must not be batched into provider turns"
    );
}

#[tokio::test]
async fn concurrent_notifications_are_acknowledged_independently() {
    let (_root, log, bridge) = tests::message_fixture().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tests::wait_for_log_marker(&log, "\"method\":\"initialized\""),
    )
    .await
    .expect("provider fixture should finish initialization");

    let notification = |status: &str| MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({
            "role":"user",
            "content":format!(
                "<agent-message from=\"worker-{status}\">{status}</agent-message>"
            )
        })],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let before = std::fs::read_to_string(&log).unwrap_or_default();
    let (first, second, third) = tokio::join!(
        bridge.messages(notification("one")),
        bridge.messages(notification("two")),
        bridge.messages(notification("three")),
    );
    for response in [first, second, third] {
        let body = to_bytes(
            response.expect("notification response").into_body(),
            usize::MAX,
        )
        .await
        .expect("read notification response");
        assert!(String::from_utf8_lossy(&body).contains("\"stop_reason\":\"end_turn\""));
    }
    let after = std::fs::read_to_string(&log).unwrap_or_default();
    let provider_turns = after
        .strip_prefix(&before)
        .unwrap_or(&after)
        .matches("\"log_event\":\"provider_turn_start\"")
        .count();
    assert_eq!(
        provider_turns, 0,
        "lifecycle notifications must never reach the provider"
    );
}
