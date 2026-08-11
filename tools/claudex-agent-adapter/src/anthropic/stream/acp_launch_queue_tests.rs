use std::fs;

use serde_json::{Value, json};

use super::{
    launch_args_from_entry, peek_pending_launch_arguments_for, peek_pending_launch_arguments_from,
    take_pending_launch_arguments_for, take_pending_launch_arguments_from,
};

#[test]
fn missing_and_malformed_queues_have_no_pending_launch() {
    let root = tempfile::tempdir().expect("queue fixture");
    let path = root.path().join("missing/queue.jsonl");
    assert_eq!(
        peek_pending_launch_arguments_from(&path, 1_000.0, None),
        None
    );

    fs::create_dir_all(path.parent().unwrap()).expect("queue parent");
    fs::write(&path, "\nnot-json\n{}\n").expect("malformed queue");
    assert_eq!(
        peek_pending_launch_arguments_from(&path, 1_000.0, None),
        None
    );
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

    let peeked = peek_pending_launch_arguments_from(&path, 1_000.0, None).expect("peeked Agent");
    assert_eq!(peeked["prompt"], "first");
    assert_eq!(peeked["_toolName"], "agent");

    let first = take_pending_launch_arguments_from(&path, 1_000.0, None).expect("first Agent");
    assert_eq!(first, peeked);
    let rewritten = fs::read_to_string(&path).expect("rewritten queue");
    assert!(!rewritten.contains("stale"));
    assert!(!rewritten.contains("first"));
    assert!(rewritten.contains("Bash"));
    assert!(rewritten.contains("second"));

    let second = take_pending_launch_arguments_from(&path, 1_000.0, None).expect("second Task");
    assert_eq!(second["prompt"], "second");
    assert_eq!(second["_toolName"], "Task");
    assert!(fs::read_to_string(&path).unwrap().contains("Bash"));
    assert_eq!(
        take_pending_launch_arguments_from(&path, 1_000.0, None),
        None
    );
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
        None,
    )
    .expect("explicit tool name");
    assert_eq!(explicit["_toolName"], "Agent");

    assert_eq!(
        launch_args_from_entry(
            &json!({"ts":1_000.0,"name":"Agent","arguments":"raw"}),
            1_000.0,
            None,
        ),
        Some(json!("raw"))
    );
    assert_eq!(
        launch_args_from_entry(&json!({"ts":1_000.0,"name":"agent"}), 1_000.0, None),
        Some(json!({"_toolName":"agent"}))
    );
}

#[test]
fn does_not_drain_another_claude_session_launch() {
    let root = tempfile::tempdir().expect("queue fixture");
    let path = root.path().join("queue.jsonl");
    let entries = [
        json!({"ts":960.0,"name":"Agent","owner":"session-b","arguments":{"prompt":"other-tui"}}),
        json!({"ts":970.0,"name":"Agent","owner":"session-a","arguments":{"prompt":"this-tui"}}),
        json!({"ts":980.0,"name":"Agent","arguments":{"prompt":"legacy-global"}}),
    ];
    let body = entries
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&path, body).expect("queue entries");

    let taken = take_pending_launch_arguments_from(&path, 1_000.0, Some("session-a"))
        .expect("session-a launch");
    assert_eq!(taken["prompt"], "this-tui");
    let leftover = fs::read_to_string(&path).expect("leftover queue");
    assert!(leftover.contains("other-tui"));
    assert!(leftover.contains("legacy-global"));
    assert!(!leftover.contains("this-tui"));

    assert_eq!(
        take_pending_launch_arguments_from(&path, 1_000.0, None).expect("untagged global")["prompt"],
        "legacy-global"
    );
    assert!(
        take_pending_launch_arguments_from(&path, 1_000.0, Some("session-b"))
            .expect("session-b launch")["prompt"]
            == "other-tui"
    );
}

#[test]
fn unmatched_take_does_not_rewrite_live_queue() {
    let root = tempfile::tempdir().expect("queue fixture");
    let path = root.path().join("queue.jsonl");
    let body = json!({"ts":970.0,"name":"Agent","owner":"session-b","arguments":{"prompt":"other"}})
        .to_string() + "\n";
    fs::write(&path, &body).expect("queue");
    let inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).expect("meta"));

    assert_eq!(
        take_pending_launch_arguments_from(&path, 1_000.0, Some("session-a")),
        None
    );
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).expect("meta")),
        inode,
        "polling take with no match must not rewrite the launch queue"
    );
    assert_eq!(fs::read_to_string(&path).expect("unchanged"), body);
}

#[test]
fn owner_scoped_queue_reads_the_per_session_file() {
    let root = tempfile::tempdir().expect("owner queue fixture");
    let global = root.path().join("launch-queue.jsonl");
    let owner_path = crate::launch_mcp::launch_queue_path(root.path(), Some("session-a"));
    let ts = super::now_secs();
    fs::write(
        &owner_path,
        json!({"ts":ts,"name":"Agent","owner":"session-a","arguments":{"prompt":"owned"}})
            .to_string()
            + "\n",
    )
    .expect("owner queue");
    fs::write(
        &global,
        json!({"ts":ts,"name":"Agent","arguments":{"prompt":"global"}}).to_string() + "\n",
    )
    .expect("global queue");
    let previous = std::env::var_os("CLAUDEX_LAUNCH_QUEUE");
    unsafe { std::env::set_var("CLAUDEX_LAUNCH_QUEUE", &global) };
    let peeked = peek_pending_launch_arguments_for(Some("session-a")).expect("owned peek");
    assert_eq!(peeked["prompt"], "owned");
    let taken = take_pending_launch_arguments_for(Some("session-a")).expect("owned take");
    assert_eq!(taken["prompt"], "owned");
    assert_eq!(take_pending_launch_arguments_for(Some("session-a")), None);
    assert_eq!(
        take_pending_launch_arguments_for(None).expect("global take")["prompt"],
        "global"
    );
    match previous {
        Some(value) => unsafe { std::env::set_var("CLAUDEX_LAUNCH_QUEUE", value) },
        None => unsafe { std::env::remove_var("CLAUDEX_LAUNCH_QUEUE") },
    }
}
