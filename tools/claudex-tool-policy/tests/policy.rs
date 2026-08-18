use claudex_tool_policy::{PolicyContext, handle_event, handle_event_with_context};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    for session_id in ["sess", "sess-main"] {
        write_session_delegation(cache, session_id, required, now);
    }
}

fn write_session_delegation(cache: &Path, session_id: &str, required: bool, now: f64) {
    write_session_state(cache, session_id, required, false, now);
}

fn write_session_state(cache: &Path, session_id: &str, base: bool, opt_out: bool, now: f64) {
    let key = hex::encode(Sha256::digest(session_id.as_bytes()));
    let directory = cache.join("delegation-state-v2");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let required = base && !opt_out;
    fs::write(
        directory.join(format!("{key}.json")),
        json!({
            "version": 2,
            "session_key": key,
            "updated_at": now,
            "expires_at": now + 86_400.0,
            "base_delegation_required": base,
            "prompt_opt_out": opt_out,
            "delegation_required": required,
            "selected_workers_count": if base { 2 } else { 0 },
            "direct_main_execution": if required { "fallback-only" } else { "allowed" }
        })
        .to_string(),
    )
    .unwrap();
    fs::set_permissions(
        directory.join(format!("{key}.json")),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}

fn as_object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

fn explicit_context(cache: &Path, now: f64) -> PolicyContext {
    PolicyContext::new(cache.to_path_buf(), cache.to_path_buf(), now, true, false)
}

fn file_lock_path(cache: &Path, target: &Path) -> std::path::PathBuf {
    let absolute = fs::canonicalize(target).unwrap();
    let digest = hex::encode(Sha256::digest(absolute.to_string_lossy().as_bytes()));
    cache.join("file-locks").join(format!("{digest}.lock.json"))
}

fn session_state_path(cache: &Path, session_id: &str) -> std::path::PathBuf {
    let digest = hex::encode(Sha256::digest(session_id.as_bytes()));
    cache
        .join("delegation-state-v2")
        .join(format!("{digest}.json"))
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
fn main_session_atomic_lookup_tools_allowed_when_delegation_required() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    for tool_name in ["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch"] {
        let output = run(
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": tool_name,
                "tool_input": {"file_path": "/tmp/x", "pattern": "x", "url": "https://example.com"},
                "session_id": "sess-main"
            }),
            tmp.path(),
            &[],
        );
        assert_eq!(
            output,
            json!({}),
            "atomic lookup `{tool_name}` must stay in main"
        );
        let decision = output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str);
        assert_ne!(decision, Some("deny"), "unexpected denial for {tool_name}");
    }
}

#[test]
fn main_session_mutating_tools_denied_when_delegation_required() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    for tool_name in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
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
            Some("deny"),
            "unexpected decision for {tool_name}"
        );
        let reason = output
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(Value::as_str)
            .unwrap();
        assert!(reason.contains(tool_name));
        assert!(reason.contains("Agent/Task"));
        assert!(reason.contains("may stay in main"));
    }
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
fn file_lock_symlink_attack_is_denied_without_clobbering_target() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let lock_dir = tmp.path().join("file-locks");
    fs::create_dir(&lock_dir).unwrap();
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let victim = tmp.path().join("victim");
    fs::write(&victim, "do-not-touch\n").unwrap();
    symlink(&victim, file_lock_path(tmp.path(), &target)).unwrap();

    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": target, "content": "new"},
        "session_id": "session-a",
        "agent_id": "agent-a"
    });
    let output = handle_event_with_context(
        payload.as_object().unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(fs::read_to_string(victim).unwrap(), "do-not-touch\n");
}

#[test]
fn owner_readable_lock_directory_is_repaired_instead_of_denying_as_another_agent() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("owned.rs");
    fs::write(&target, "ok\n").unwrap();
    let lock_dir = tmp.path().join("file-locks");
    fs::create_dir(&lock_dir).unwrap();
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o755)).unwrap();

    let output = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "new"},
            "session_id": "session-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny"),
        "owner-controlled 0755 file-locks must be repaired, not reported as another agent: {output}"
    );
    let reason = output
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !reason.contains("another agent"),
        "infrastructure denial leaked the unnamed-holder fallback: {reason}"
    );
    assert_eq!(
        fs::metadata(&lock_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn stale_file_lock_is_reclaimed_under_guard_after_ttl() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let first = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": target, "content": "first"},
        "session_id": "session-a",
        "agent_id": "agent-a"
    });
    let first_result = handle_event_with_context(
        first.as_object().unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        first_result
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );

    let second = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": target, "content": "second"},
        "session_id": "session-b",
        "agent_id": "agent-b"
    });
    let second_result = handle_event_with_context(
        second.as_object().unwrap(),
        &explicit_context(tmp.path(), 1_000.0 + 5.0 * 60.0 + 1.0),
    );
    assert_ne!(
        second_result
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
    let record: Value = serde_json::from_slice(
        &fs::read(file_lock_path(tmp.path(), &tmp.path().join("shared.rs"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["agent_id"], "agent-b");
    assert_eq!(record["session_id"], "session-b");
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
fn same_agent_can_refresh_its_own_lock() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let first = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "session-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        first
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "session-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_ne!(
        second
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn file_lock_in_group_readable_dir_reports_real_holder() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let first = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "session-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        first
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
    fs::set_permissions(
        tmp.path().join("file-locks"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "session-a",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = second["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("agent-a"));
    assert!(!reason.contains("another agent"));
}

#[test]
fn stale_file_lock_in_group_readable_dir_is_reclaimed() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let first = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "session-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        first
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
    fs::set_permissions(
        tmp.path().join("file-locks"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "session-b",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0 + 5.0 * 60.0 + 1.0),
    );
    assert_ne!(
        second
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn session_end_releases_session_locks() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("owned.rs");
    fs::write(&target, "ok\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "ok"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "SessionEnd",
            "session_id": "sess"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    let allowed = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "next"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_002.0),
    );
    assert_ne!(
        allowed
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn session_end_does_not_release_other_session_locks() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("owned.rs");
    fs::write(&target, "ok\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "ok"},
            "session_id": "sess-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "SessionEnd",
            "session_id": "sess-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    let blocked = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "next"},
            "session_id": "sess-a",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_002.0),
    );
    assert_eq!(blocked["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = blocked["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("agent-a"));
}

#[test]
fn lock_conflict_reports_agent_type_and_id() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "sess",
            "agent_id": "agent-a",
            "agent_type": "claudex-grok"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = second["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("claudex-grok (agent-a)"));
    assert!(!reason.contains("another agent"));
}

#[test]
fn malformed_lock_is_reclaimed() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let lock_dir = tmp.path().join("file-locks");
    fs::create_dir(&lock_dir).unwrap();
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let lock_path = file_lock_path(tmp.path(), &target);
    fs::write(&lock_path, "not-json\n").unwrap();
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "new"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn lock_store_failure_fails_open() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("not-a-directory");
    fs::write(&cache, "x\n").unwrap();
    let output = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/x", "content": "new"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &PolicyContext::new(cache, tmp.path().to_path_buf(), 1_000.0, true, false),
    );
    assert_ne!(
        output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn fresh_lock_is_not_reclaimed_before_ttl() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "session-a",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "session-b",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0 + 5.0 * 60.0),
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn main_session_write_does_not_report_file_lock() {
    let tmp = TempDir::new().unwrap();
    write_session_delegation(tmp.path(), "sess-main", true, 1_000.0);
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "worker"},
            "session_id": "sess-main",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let main = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "main"},
            "session_id": "sess-main"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(main["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = main["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("Agent/Task"));
    assert!(!reason.contains("locked by"));
}

#[test]
fn post_tool_use_releases_path_lock() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("owned.rs");
    fs::write(&target, "ok\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "ok"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "ok"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    let allowed = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "next"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_002.0),
    );
    assert_ne!(
        allowed
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn multi_edit_conflict_rolls_back_earlier_paths() {
    let tmp = TempDir::new().unwrap();
    let first = tmp.path().join("first.rs");
    let second = tmp.path().join("second.rs");
    fs::write(&first, "one\n").unwrap();
    fs::write(&second, "two\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": second, "content": "held"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let denied = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "MultiEdit",
            "tool_input": {
                "edits": [
                    {"file_path": first, "old_string": "one", "new_string": "ONE"},
                    {"path": second, "old_string": "two", "new_string": "TWO"}
                ]
            },
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
    let allowed = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": first, "content": "free"},
            "session_id": "sess",
            "agent_id": "agent-c"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_002.0),
    );
    assert_ne!(
        allowed
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn tilde_path_and_notebook_lock_the_resolved_file() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("note.ipynb");
    fs::write(&target, "{}\n").unwrap();
    let first = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "NotebookEdit",
            "tool_input": {"notebook_path": "~/note.ipynb"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        first
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "x"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn non_object_lock_record_is_reclaimed() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let lock_dir = tmp.path().join("file-locks");
    fs::create_dir(&lock_dir).unwrap();
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let lock_path = file_lock_path(tmp.path(), &target);
    fs::write(&lock_path, "[]\n").unwrap();
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "new"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn oversized_lock_record_is_reclaimed() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let lock_dir = tmp.path().join("file-locks");
    fs::create_dir(&lock_dir).unwrap();
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let lock_path = file_lock_path(tmp.path(), &target);
    fs::write(&lock_path, vec![b'x'; 16_384]).unwrap();
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "new"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    assert_ne!(
        output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );
}

#[test]
fn group_writable_lock_record_is_denied_as_unsafe() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let lock_path = file_lock_path(tmp.path(), &target);
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = second["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("unsafe or unreadable"));
    assert!(!reason.contains("another agent"));
}

#[test]
fn file_lock_busy_guard_reports_busy_not_another_agent() {
    use std::os::fd::AsRawFd as _;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared.rs");
    fs::write(&target, "source\n").unwrap();
    let _ = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "first"},
            "session_id": "sess",
            "agent_id": "agent-a"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_000.0),
    );
    let lock_path = file_lock_path(tmp.path(), &target);
    let name = lock_path.file_name().unwrap().to_str().unwrap();
    let digest = name.strip_suffix(".lock.json").unwrap();
    let guard = tmp
        .path()
        .join("file-locks")
        .join(format!("{digest}.guard"));
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&guard)
        .unwrap();
    let locked = unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, 0);
    let second = handle_event_with_context(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": target, "content": "second"},
            "session_id": "sess",
            "agent_id": "agent-b"
        })
        .as_object()
        .unwrap(),
        &explicit_context(tmp.path(), 1_001.0),
    );
    assert_eq!(second["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = second["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("lock is busy"));
    assert!(!reason.contains("another agent"));
}

#[test]
fn allow_main_tools_override() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), true);
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/x", "content": "hello"},
            "session_id": "sess"
        }),
        tmp.path(),
        &[("CLAUDEX_ALLOW_MAIN_TOOLS", Some("1"))],
    );
    assert_eq!(output, json!({}));
}

#[test]
fn main_session_write_allowed_when_delegation_not_required() {
    let tmp = TempDir::new().unwrap();
    write_delegation(tmp.path(), false);
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
    assert_eq!(output, json!({}));
}

#[test]
fn routing_cache_selected_workers_is_not_a_cross_session_fallback() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("usage-routing.json"),
        json!({
            "summary": {
                "selected_workers": [{"agent": "claudex-grok"}]
            }
        })
        .to_string(),
    )
    .unwrap();
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
    assert_eq!(output, json!({}));
}

#[test]
fn routing_cache_selected_workers_allows_main_read() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("usage-routing.json"),
        json!({
            "summary": {
                "selected_workers": [{"agent": "claudex-grok"}]
            }
        })
        .to_string(),
    )
    .unwrap();
    let output = run(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/x"},
            "session_id": "sess-main"
        }),
        tmp.path(),
        &[],
    );
    assert_eq!(output, json!({}));
}

#[test]
fn explicit_context_keeps_two_sessions_isolated_without_environment_mutation() {
    let tmp = TempDir::new().unwrap();
    write_session_state(tmp.path(), "session-b", true, true, 1_000.0);
    write_session_delegation(tmp.path(), "session-a", true, 1_000.0);
    let context = PolicyContext::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        1_001.0,
        true,
        false,
    );
    let mut payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "sessionId": "session-b"
    });
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );

    payload.as_object_mut().unwrap().remove("sessionId");
    payload["session_id"] = Value::from("session-a");
    let denied = handle_event_with_context(payload.as_object().unwrap(), &context);
    assert_eq!(
        denied
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(Value::as_str),
        Some("deny")
    );

    payload.as_object_mut().unwrap().remove("session_id");
    payload["sessionId"] = Value::from("session-b");
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );
    payload["sessionId"] = Value::from("session-missing");
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );
}

#[test]
fn explicit_context_rejects_stale_bad_key_and_malformed_schema_fail_open() {
    let tmp = TempDir::new().unwrap();
    let context = PolicyContext::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        100_000.0,
        true,
        false,
    );
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "session_id": "session-a"
    });
    for (timestamp, key, count) in [
        (1.0, hex::encode(Sha256::digest(b"session-a")), 1_u64),
        (100_301.0, hex::encode(Sha256::digest(b"session-a")), 1),
        (100_000.0, hex::encode(Sha256::digest(b"other")), 1),
        (100_000.0, hex::encode(Sha256::digest(b"session-a")), 999),
    ] {
        let expected_path = {
            let correct = hex::encode(Sha256::digest(b"session-a"));
            let directory = tmp.path().join("delegation-state-v2");
            fs::create_dir_all(&directory).unwrap();
            directory.join(format!("{correct}.json"))
        };
        fs::write(
            expected_path,
            json!({
                "version": 2,
                "session_key": key,
                "updated_at": timestamp,
                "expires_at": timestamp + 86_400.0,
                "base_delegation_required": true,
                "prompt_opt_out": false,
                "delegation_required": true,
                "selected_workers_count": count,
                "direct_main_execution": "fallback-only"
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            handle_event_with_context(payload.as_object().unwrap(), &context),
            json!({})
        );
    }
}

#[test]
fn explicit_context_strictly_rejects_missing_extra_and_inconsistent_state_fields() {
    let tmp = TempDir::new().unwrap();
    let context = explicit_context(tmp.path(), 1_001.0);
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "session_id": "session-a"
    });
    write_session_delegation(tmp.path(), "session-a", true, 1_000.0);
    let path = session_state_path(tmp.path(), "session-a");
    let valid: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    let mut variants = Vec::new();
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("expires_at");
    variants.push(missing);
    let mut extra = valid.clone();
    extra["unexpected"] = Value::Bool(true);
    variants.push(extra);
    let mut inconsistent = valid.clone();
    inconsistent["prompt_opt_out"] = Value::Bool(true);
    variants.push(inconsistent);
    let mut bad_expiry = valid.clone();
    bad_expiry["expires_at"] = Value::from(999.0);
    variants.push(bad_expiry);
    let mut short_ttl = valid.clone();
    short_ttl["expires_at"] = (short_ttl["updated_at"].as_f64().unwrap() + 86_399.0).into();
    variants.push(short_ttl);
    let mut overlong_ttl = valid;
    overlong_ttl["expires_at"] = Value::from(87_401.0);
    variants.push(overlong_ttl);

    for variant in variants {
        fs::write(&path, serde_json::to_vec(&variant).unwrap()).unwrap();
        assert_eq!(
            handle_event_with_context(payload.as_object().unwrap(), &context),
            json!({}),
            "invalid state must fail open: {variant}"
        );
    }
}

#[test]
fn explicit_context_rejects_oversized_state_and_symlinked_state_directory() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let context = PolicyContext::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        1_001.0,
        true,
        false,
    );
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "session_id": "session-a"
    });
    write_session_delegation(tmp.path(), "session-a", true, 1_000.0);
    fs::write(
        tmp.path().join("delegation-state-v2").join(format!(
            "{}.json",
            hex::encode(Sha256::digest(b"session-a"))
        )),
        vec![b' '; 16_385],
    )
    .unwrap();
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );

    fs::remove_dir_all(tmp.path().join("delegation-state-v2")).unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    write_session_delegation(&outside, "session-a", true, 1_000.0);
    symlink(
        outside.join("delegation-state-v2"),
        tmp.path().join("delegation-state-v2"),
    )
    .unwrap();
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );
}

#[test]
fn explicit_context_rejects_symlinked_cache_chain() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let real_dot_cache = tmp.path().join("real-dot-cache");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&real_dot_cache).unwrap();
    symlink(&real_dot_cache, home.join(".cache")).unwrap();
    let cache = home.join(".cache/claudex");
    fs::create_dir(&cache).unwrap();
    write_session_delegation(&cache, "session-a", true, 1_000.0);

    let context = PolicyContext::new(cache, home, 1_001.0, true, false);
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "session_id": "session-a"
    });
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );
}

#[test]
fn explicit_context_rejects_group_writable_state_file_and_directory() {
    let tmp = TempDir::new().unwrap();
    let context = explicit_context(tmp.path(), 1_001.0);
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "session_id": "session-a"
    });
    write_session_delegation(tmp.path(), "session-a", true, 1_000.0);
    let state_path = session_state_path(tmp.path(), "session-a");
    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o620)).unwrap();
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );

    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(
        tmp.path().join("delegation-state-v2"),
        fs::Permissions::from_mode(0o720),
    )
    .unwrap();
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );
}

#[test]
fn legacy_global_delegation_state_is_ignored() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("delegation-state.json"),
        json!({
            "delegation_required": true,
            "selected_workers_count": 99,
            "direct_main_execution": "fallback-only"
        })
        .to_string(),
    )
    .unwrap();
    let context = PolicyContext::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        1_001.0,
        true,
        false,
    );
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/x", "content": "hello"},
        "session_id": "session-a"
    });
    assert_eq!(
        handle_event_with_context(payload.as_object().unwrap(), &context),
        json!({})
    );
}
