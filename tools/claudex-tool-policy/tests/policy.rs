use claudex_tool_policy::handle_event;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    keys: Vec<&'static str>,
}

impl EnvGuard {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let keys: Vec<_> = pairs.iter().map(|(k, _)| *k).collect();
        for (key, value) in pairs {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        Self { keys }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in &self.keys {
            unsafe { std::env::remove_var(key) };
        }
    }
}

fn write_delegation(cache: &Path, required: bool) {
    fs::write(
        cache.join("delegation-state.json"),
        json!({
            "delegation_required": required,
            "selected_workers_count": 2,
            "direct_main_execution": "fallback-only"
        })
        .to_string(),
    )
    .unwrap();
}

fn as_object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

fn run(payload: Value, cache: &Path, extra: &[(&'static str, Option<&str>)]) -> Value {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut pairs = vec![
        ("CLAUDEX_ACTIVE", Some("1")),
        ("CLAUDEX_SUBAGENT_FIRST", Some("1")),
        ("CLAUDEX_CACHE_DIR", Some(cache.to_str().unwrap())),
        ("CLAUDEX_ALLOW_MAIN_TOOLS", None),
        ("CLAUDEX_NONINTERACTIVE_CHILD", None),
        ("CLAUDEX_GROK_ACP", None),
        ("CLAUDEX_PROVIDER_ACP", None),
    ];
    pairs.extend_from_slice(extra);
    let _guard = EnvGuard::set(&pairs);
    handle_event(&as_object(payload))
}

#[test]
fn main_session_bash_allowed_when_delegation_required() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "session_id": "sess-main"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(output, json!({}));
}

#[test]
fn main_session_delegated_tools_allowed_with_advisory() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    for tool_name in [
        "Read",
        "Write",
        "Edit",
        "MultiEdit",
        "NotebookEdit",
        "Grep",
        "Glob",
        "LS",
        "WebSearch",
        "WebFetch",
    ] {
        let output = run(
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": tool_name,
                "tool_input": {"file_path": "/tmp/x", "content": "hello"},
                "session_id": "sess-main"
            }),
            tmp.path(),
            &[],
        );
        let decision = output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str);
        assert_eq!(
            decision,
            Some("allow"),
            "unexpected decision for {tool_name}"
        );
        assert_ne!(decision, Some("deny"), "unexpected denial for {tool_name}");
        let reason = output
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(Value::as_str)
            .unwrap();
        assert!(reason.contains(tool_name));
        assert!(reason.contains("Agent/Task"));
        assert!(reason.contains("fallback-only"));
    }
}

#[test]
fn main_session_write_allowed_with_delegation_advisory() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/x", "content": "hello"},
            "session_id": "sess-main"
        }),
        tmp.path(),
        &[],
    );
    let decision = output
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str);
    assert_eq!(decision, Some("allow"));
    assert_ne!(decision, Some("deny"));
    let reason = output
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(Value::as_str)
        .unwrap();
    assert!(reason.contains("Agent/Task"));
    assert!(reason.contains("fallback-only"));
}

#[test]
fn subagent_bash_allowed() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "session_id": "sess-main",
            "agent_id": "agent-1",
            "agent_type": "claudex-gpt"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(output, json!({}));
}

#[test]
fn subagent_read_explicitly_allowed_despite_main_denylist() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/x"},
            "session_id": "sess",
            "agent_id": "agent-worker",
            "agent_type": "claudex-fugu"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
    let reason = output["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(reason.contains("do not apply"));
}

#[test]
fn subagent_detected_via_transcript_path_without_agent_id() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Grep",
            "tool_input": {"pattern": "x"},
            "session_id": "sess",
            "transcript_path": "/tmp/projects/abc/subagents/agent-xyz.jsonl"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
}

#[test]
fn file_lock_blocks_second_writer() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "fn main() {}\n").unwrap();
    let first = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Edit",
            "tool_input": {
                "file_path": target.to_str().unwrap(),
                "old_string": "a",
                "new_string": "b"
            },
            "session_id": "sess",
            "agent_id": "agent-a"
        }),
        tmp.path(),
        &[],
    );
    assert_ne!(
        first
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
    let second = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {
                "file_path": target.to_str().unwrap(),
                "content": "x"
            },
            "session_id": "sess",
            "agent_id": "agent-b"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = second["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("agent-a"));
}

#[test]
fn subagent_stop_releases_locks() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("owned.rs");
    fs::write(&target, "ok\n").unwrap();
    let _ = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {
                "file_path": target.to_str().unwrap(),
                "content": "ok"
            },
            "session_id": "sess",
            "agent_id": "agent-a"
        }),
        tmp.path(),
        &[],
    );
    let _ = run(
        json!({
            "hook_event_name": "SubagentStop",
            "agent_id": "agent-a",
            "session_id": "sess"
        }),
        tmp.path(),
        &[],
    );
    let allowed = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {
                "file_path": target.to_str().unwrap(),
                "content": "next"
            },
            "session_id": "sess",
            "agent_id": "agent-b"
        }),
        tmp.path(),
        &[],
    );
    assert_ne!(
        allowed
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn recursive_subagent_stop_returns_success_without_mutating_locks() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("owned.rs");
    fs::write(&target, "ok\n").unwrap();
    let _ = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "ok"},
            "session_id": "sess",
            "agent_id": "agent-a"
        }),
        tmp.path(),
        &[],
    );

    let recursive = run(
        json!({
            "hook_event_name": "SubagentStop",
            "stop_hook_active": true,
            "agent_id": "agent-a",
            "session_id": "sess"
        }),
        tmp.path(),
        &[],
    );
    assert_ne!(
        recursive
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );

    let blocked = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "next"},
            "session_id": "sess",
            "agent_id": "agent-b"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(
        blocked
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn allow_main_tools_override() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/x"},
            "session_id": "sess"
        }),
        tmp.path(),
        &[("CLAUDEX_ALLOW_MAIN_TOOLS", Some("1"))],
    );
    assert_eq!(output, json!({}));
}
