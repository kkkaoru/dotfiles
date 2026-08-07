use std::fs;

use serde_json::{Value, json};

use super::{
    launch_args_from_entry, peek_pending_launch_arguments_from, take_pending_launch_arguments_from,
};

#[test]
fn missing_and_malformed_queues_have_no_pending_launch() {
    let root = tempfile::tempdir().expect("queue fixture");
    let path = root.path().join("missing/queue.jsonl");
    assert_eq!(peek_pending_launch_arguments_from(&path, 1_000.0), None);

    fs::create_dir_all(path.parent().unwrap()).expect("queue parent");
    fs::write(&path, "\nnot-json\n{}\n").expect("malformed queue");
    assert_eq!(peek_pending_launch_arguments_from(&path, 1_000.0), None);
}

#[test]
fn pops_oldest_fresh_launch_and_rewrites_only_live_entries() {
    let root = tempfile::tempdir().expect("queue fixture");
    let path = root.path().join("queue.jsonl");
    let entries = [
        json!({"ts":800.0,"name":"Agent","arguments":{"prompt":"stale"}}),
        json!({"ts":950.0,"name":"Bash","arguments":{"command":"true"}}),
        json!({"ts":960.0,"name":"agent","arguments":{"prompt":"first"}}),
        json!({"ts":970.0,"name":"Task","arguments":{"prompt":"second"}}),
    ];
    let body = entries
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&path, body).expect("queue entries");

    let peeked = peek_pending_launch_arguments_from(&path, 1_000.0).expect("peeked Agent");
    assert_eq!(peeked["prompt"], "first");
    assert_eq!(peeked["_toolName"], "agent");

    let first = take_pending_launch_arguments_from(&path, 1_000.0).expect("first Agent");
    assert_eq!(first, peeked);
    let rewritten = fs::read_to_string(&path).expect("rewritten queue");
    assert!(!rewritten.contains("stale"));
    assert!(!rewritten.contains("first"));
    assert!(rewritten.contains("Bash"));
    assert!(rewritten.contains("second"));

    let second = take_pending_launch_arguments_from(&path, 1_000.0).expect("second Task");
    assert_eq!(second["prompt"], "second");
    assert_eq!(second["_toolName"], "Task");
    assert!(fs::read_to_string(&path).unwrap().contains("Bash"));
    assert_eq!(take_pending_launch_arguments_from(&path, 1_000.0), None);
}

#[test]
fn launch_arguments_preserve_explicit_tool_names_and_scalar_values() {
    let explicit = launch_args_from_entry(
        &json!({
            "ts":1_000.0,
            "name":"Task",
            "arguments":{"prompt":"work","_toolName":"Agent"}
        }),
        1_000.0,
    )
    .expect("explicit tool name");
    assert_eq!(explicit["_toolName"], "Agent");

    assert_eq!(
        launch_args_from_entry(
            &json!({"ts":1_000.0,"name":"Agent","arguments":"raw"}),
            1_000.0,
        ),
        Some(json!("raw"))
    );
    assert_eq!(
        launch_args_from_entry(&json!({"ts":1_000.0,"name":"agent"}), 1_000.0),
        Some(json!({"_toolName":"agent"}))
    );
}
