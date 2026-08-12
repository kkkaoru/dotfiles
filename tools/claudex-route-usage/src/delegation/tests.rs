use super::prompt::{
    current_prompt_opts_out, effective_summary, session_id, session_key, state_path,
};
use super::publish::{session_record, validated_record, write_delegation_state};
use super::{
    LEGACY_STATE_FILE, MAX_SESSION_ID_BYTES, STATE_DIRECTORY, STATE_KEYS, STATE_TTL_SECONDS,
};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

fn cache_dir(home: &Path) -> PathBuf {
    home.join(".cache/claudex")
}

fn routed_summary(agent: &str) -> Value {
    serde_json::json!({
        "delegation_required": true,
        "direct_main_execution": "fallback-only",
        "selected_workers": [{"agent": agent, "model": "gpt-5.6-luna"}]
    })
}

fn read_state(home: &Path, id: &str) -> Value {
    let path = state_path(&cache_dir(home), id);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn spawn_writer(
    home: PathBuf,
    barrier: Arc<Barrier>,
    timestamp: i32,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        barrier.wait();
        write_delegation_state(
            &home,
            Some("shared-session"),
            &routed_summary(&format!("worker-{timestamp}")),
            f64::from(timestamp),
        )
        .unwrap();
    })
}

#[test]
fn current_session_field_is_authoritative_and_bounded() {
    assert_eq!(
        session_id(&serde_json::json!({"session_id":" current ", "sessionId":"legacy"})),
        Some("current")
    );
    assert_eq!(
        session_id(&serde_json::json!({"sessionId":"legacy"})),
        Some("legacy")
    );
    assert_eq!(
        session_id(&serde_json::json!({"session_id":42, "sessionId":"legacy"})),
        None
    );
    assert_eq!(session_id(&serde_json::json!({"session_id":"\n"})), None);
    let long = "x".repeat(MAX_SESSION_ID_BYTES + 1);
    assert_eq!(session_id(&serde_json::json!({"session_id":long})), None);
    let multibyte = "é".repeat((MAX_SESSION_ID_BYTES / 2) + 1);
    assert_eq!(
        session_id(&serde_json::json!({"session_id":multibyte})),
        None
    );
}

#[test]
fn session_path_hashes_untrusted_ids_without_traversal() {
    let cache = Path::new("/tmp/cache");
    let path = state_path(cache, "../../escape/session");
    assert_eq!(path.parent(), Some(cache.join(STATE_DIRECTORY).as_path()));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(name.len(), 69);
    assert!(!name.contains(".."));
    assert!(!name.contains("session"));
}

#[test]
fn two_sessions_get_private_independent_v2_states_and_neutral_tombstone() {
    let root = tempfile::tempdir().unwrap();
    write_delegation_state(
        root.path(),
        Some("session-a"),
        &routed_summary("worker-a"),
        12.0,
    )
    .unwrap();
    let direct = effective_summary(
        routed_summary("worker-b"),
        Some(&serde_json::json!({"prompt":"Do not delegate this request"})),
    );
    write_delegation_state(root.path(), Some("session-b"), &direct, 13.0).unwrap();

    let first = read_state(root.path(), "session-a");
    let second = read_state(root.path(), "session-b");
    assert_eq!(first["delegation_required"], true);
    assert_eq!(first["selected_workers_count"], 1);
    assert_eq!(first["base_delegation_required"], true);
    assert_eq!(first["prompt_opt_out"], false);
    assert_eq!(first["expires_at"], 12.0 + STATE_TTL_SECONDS);
    assert_eq!(second["delegation_required"], false);
    assert_eq!(second["selected_workers_count"], 1);
    assert_eq!(second["base_delegation_required"], true);
    assert_eq!(second["prompt_opt_out"], true);
    assert_eq!(second.as_object().unwrap().len(), STATE_KEYS.len());
    assert_ne!(first["session_key"], second["session_key"]);
    let legacy: Value =
        serde_json::from_slice(&fs::read(cache_dir(root.path()).join(LEGACY_STATE_FILE)).unwrap())
            .unwrap();
    assert_eq!(legacy["delegation_required"], false);
    assert_eq!(legacy["version"], 1);

    let mode = fs::metadata(state_path(&cache_dir(root.path()), "session-a"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn concurrent_writers_cannot_replace_a_newer_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for timestamp in 1..=8 {
        let home = home.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(spawn_writer(home, barrier, timestamp));
    }
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(read_state(root.path(), "shared-session")["updated_at"], 8.0);
}

#[test]
fn older_write_cannot_replace_newer_policy() {
    let root = tempfile::tempdir().unwrap();
    write_delegation_state(
        root.path(),
        Some("session"),
        &routed_summary("newer-worker"),
        20.0,
    )
    .unwrap();
    let direct = effective_summary(
        routed_summary("older-worker"),
        Some(&serde_json::json!({"prompt":"do not delegate"})),
    );
    write_delegation_state(root.path(), Some("session"), &direct, 10.0).unwrap();
    let state = read_state(root.path(), "session");
    assert_eq!(state["updated_at"], 20.0);
    assert_eq!(state["delegation_required"], true);
}

#[test]
fn short_ttl_is_rejected_by_route_validator() {
    let key = session_key("session");
    let mut record = session_record(&routed_summary("worker"), &key, 1_000.0);
    record["expires_at"] = Value::from(1_000.0 + STATE_TTL_SECONDS - 1.0);
    assert!(validated_record(&record, &key, 1_001.0).is_none());
}

#[test]
fn symlinked_state_directory_is_rejected_without_writing_through_it() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let cache = cache_dir(root.path());
    fs::create_dir_all(&cache).unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, cache.join(STATE_DIRECTORY)).unwrap();

    assert!(
        write_delegation_state(
            root.path(),
            Some("session"),
            &routed_summary("worker"),
            12.0,
        )
        .is_err()
    );
    assert!(fs::read_dir(outside).unwrap().next().is_none());
    assert!(!cache.join(LEGACY_STATE_FILE).exists());
    assert!(
        !cache
            .join(format!("{STATE_DIRECTORY}/{}.json", session_key("session")))
            .exists()
    );
}

#[test]
fn symlinked_cache_chain_is_rejected_without_writing_through_it() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside-cache");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.path().join(".cache")).unwrap();

    assert!(
        write_delegation_state(
            root.path(),
            Some("session"),
            &routed_summary("worker"),
            12.0,
        )
        .is_err()
    );
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[test]
fn current_prompt_opt_out_retains_choices_and_does_not_consult_legacy_prompt() {
    let summary = routed_summary("worker-a");
    let normal = effective_summary(
        summary.clone(),
        Some(&serde_json::json!({
            "prompt":"Please delegate normally",
            "user_prompt":"do not delegate"
        })),
    );
    assert_eq!(normal["selected_workers"], summary["selected_workers"]);
    assert_eq!(normal["delegation_required"], true);
    assert_eq!(normal["base_delegation_required"], true);
    assert_eq!(normal["prompt_delegation_opt_out"], false);

    let opted_out = effective_summary(
        summary.clone(),
        Some(&serde_json::json!({"prompt":"Please DO NOT DELEGATE this turn"})),
    );
    assert_eq!(opted_out["selected_workers"], summary["selected_workers"]);
    assert_eq!(opted_out["delegation_required"], false);
    assert_eq!(opted_out["direct_main_execution"], "allowed");
    assert_eq!(opted_out["base_delegation_required"], true);
    assert_eq!(opted_out["prompt_delegation_opt_out"], true);
}

#[test]
fn opt_out_literals_are_unicode_apostrophe_normalized_and_boundary_aware() {
    for prompt in [
        "do not delegate",
        "DON’T DELEGATE this",
        "dont delegate, please",
        "no delegation",
        "work without delegation",
        "do not use subagents",
        "DON'T USE SUBAGENTS",
        "dont use subagents",
        "no subagents",
        "この処理は委譲しないでください",
        "今回は委譲しない",
        "サブエージェントを使わない方針",
    ] {
        assert!(
            current_prompt_opts_out(&serde_json::json!({"prompt": prompt})),
            "expected opt-out: {prompt}"
        );
    }
    for prompt in [
        "do not delegated work",
        "undo not delegate marker",
        "without delegational overhead",
        "no subagentship",
        "do not use subagents2",
        "delegation is useful",
    ] {
        assert!(
            !current_prompt_opts_out(&serde_json::json!({"prompt": prompt})),
            "false positive: {prompt}"
        );
    }
}

#[test]
fn migration_failure_preserves_existing_global_state_and_skips_v2_publication() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let cache = cache_dir(root.path());
    fs::create_dir_all(&cache).unwrap();
    let legacy_path = cache.join(LEGACY_STATE_FILE);
    fs::write(
        &legacy_path,
        serde_json::json!({"delegation_required":true}).to_string(),
    )
    .unwrap();
    let outside = root.path().join("outside-lock");
    fs::write(&outside, "unchanged").unwrap();
    symlink(&outside, cache.join("delegation-state.migration.lock")).unwrap();

    let error = write_delegation_state(
        root.path(),
        Some("session"),
        &effective_summary(routed_summary("worker"), None),
        12.0,
    )
    .unwrap_err();
    assert!(error.to_string().contains("delegation lock"));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "unchanged");
    let legacy: Value = serde_json::from_slice(&fs::read(legacy_path).unwrap()).unwrap();
    assert_eq!(legacy["delegation_required"], true);
    assert!(!state_path(&cache, "session").exists());
}
