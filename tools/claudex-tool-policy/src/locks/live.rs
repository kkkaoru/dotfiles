use super::store::{
    LockIdentity, RecordGuard, RecordPaths, SAME_SESSION_STEAL_TTL_SECONDS, atomic_write_record,
    directory_entries, owner_matches, read_record, session_matches, sync_remove,
};
use crate::env::nonempty_str;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;

fn live_digest(session_id: &str, agent_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("{session_id}\0{agent_id}").as_bytes(),
    ))
}

fn live_name_digest(name: &str) -> Option<&str> {
    let digest = name.strip_suffix(".live.json")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(digest)
}

fn live_paths(directory: &File, session_id: &str, agent_id: &str) -> std::io::Result<RecordPaths> {
    let digest = live_digest(session_id, agent_id);
    Ok(RecordPaths {
        directory: directory.try_clone()?,
        record_name: format!("{digest}.live.json"),
        guard_name: format!("{digest}.live.guard"),
    })
}

fn live_record(identity: &LockIdentity<'_>, now: f64) -> Value {
    serde_json::json!({
        "agent_id": identity.agent_id,
        "session_id": identity.session_id,
        "updated_at": now,
    })
}

fn remove_if(paths: &RecordPaths, should_remove: impl Fn(&Value) -> bool) {
    let Ok(_guard) = RecordGuard::acquire(&paths.directory, &paths.guard_name) else {
        return;
    };
    let Ok(Some(record)) = read_record(&paths.directory, &paths.record_name) else {
        return;
    };
    if should_remove(&record) {
        let _ = sync_remove(&paths.directory, &paths.record_name);
    }
}

fn live_mark_is_fresh(record: &Value, now: f64) -> bool {
    let Some(updated) = record.get("updated_at").and_then(Value::as_f64) else {
        return false;
    };
    updated.is_finite()
        && updated >= 0.0
        && now.is_finite()
        && now >= updated
        && now - updated <= SAME_SESSION_STEAL_TTL_SECONDS
}

fn live_record_is_fresh(directory: &File, session_id: &str, agent_id: &str, now: f64) -> bool {
    let Ok(paths) = live_paths(directory, session_id, agent_id) else {
        return true;
    };
    match read_record(&paths.directory, &paths.record_name) {
        Ok(Some(live)) => live_mark_is_fresh(&live, now),
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Best-effort: a live writer keeps this mark until SubagentStop or SessionEnd.
pub(super) fn mark_live(directory: &File, identity: &LockIdentity<'_>, now: f64) {
    let Ok(paths) = live_paths(directory, identity.session_id, identity.agent_id) else {
        return;
    };
    let Ok(_guard) = RecordGuard::acquire(&paths.directory, &paths.guard_name) else {
        return;
    };
    let _ = atomic_write_record(
        &paths.directory,
        &paths.record_name,
        &live_record(identity, now),
    );
}

pub(super) fn unmark_agent(directory: &File, identity: &LockIdentity<'_>) {
    let Ok(paths) = live_paths(directory, identity.session_id, identity.agent_id) else {
        return;
    };
    remove_if(&paths, |record| {
        owner_matches(record, identity.agent_id, identity.session_id)
    });
}

pub(super) fn unmark_session(directory: &File, session_id: &str) {
    let Some(entries) = directory_entries(directory) else {
        return;
    };
    for name in entries {
        let Some(digest) = live_name_digest(&name) else {
            continue;
        };
        let guard_name = format!("{digest}.live.guard");
        let Ok(cloned) = directory.try_clone() else {
            continue;
        };
        let paths = RecordPaths {
            directory: cloned,
            record_name: name,
            guard_name,
        };
        remove_if(&paths, |record| session_matches(record, session_id));
    }
}

/// Missing identity is leftover. A live mark older than the same-session
/// steal window is leftover too (SubagentStop often never arrives). Unreadable
/// live state fails closed so a live sibling is not stolen.
pub(super) fn holder_is_live(directory: &File, record: &Value, now: f64) -> bool {
    let Some(agent_id) = nonempty_str(record.get("agent_id")) else {
        return false;
    };
    let Some(session_id) = nonempty_str(record.get("session_id")) else {
        return false;
    };
    live_record_is_fresh(directory, session_id, agent_id, now)
}

#[cfg(test)]
mod tests {
    use super::live_mark_is_fresh;
    use serde_json::json;

    #[test]
    fn live_mark_is_fresh_within_same_session_ttl() {
        assert!(live_mark_is_fresh(&json!({"updated_at": 1000.0}), 1090.0));
        assert!(!live_mark_is_fresh(&json!({"updated_at": 1000.0}), 1091.0));
        assert!(!live_mark_is_fresh(&json!({}), 1000.0));
        assert!(!live_mark_is_fresh(&json!({"updated_at": -1.0}), 1000.0));
    }
}
