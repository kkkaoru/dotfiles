use super::store::{
    RecordGuard, RecordPaths, atomic_write_record, claim_id, directory_entries, ensure_lock_dir,
    holder_of, is_stale, lock_record, owner_matches, read_record, record_paths, sync_remove,
};
use super::{deny_locked, resolve_absolute};
use crate::env::nonempty_str;
use crate::policy::PolicyContext;
use serde_json::Value;
use std::fs::File;

struct AcquiredClaim {
    paths: RecordPaths,
    claim_id: String,
}

fn acquire_one(
    directory: &File,
    file_path: &str,
    agent_id: &str,
    session_id: &str,
    now: f64,
    home: &std::path::Path,
) -> Result<Option<AcquiredClaim>, Value> {
    let absolute = resolve_absolute(file_path, home);
    let Ok(paths) = record_paths(directory, &absolute) else {
        return Err(deny_locked(&absolute, None));
    };
    let Ok(_guard) = RecordGuard::acquire(&paths.directory, &paths.guard_name) else {
        return Err(deny_locked(&absolute, None));
    };
    let existing = match read_record(&paths.directory, &paths.record_name) {
        Ok(existing) => existing,
        Err(_) => return Err(deny_locked(&absolute, None)),
    };
    if let Some(record) = existing.as_ref() {
        if owner_matches(record, agent_id, session_id) {
            let current_claim = record
                .get("claim_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| claim_id(agent_id, &absolute, now));
            let refreshed = lock_record(&absolute, agent_id, session_id, &current_claim, now);
            return atomic_write_record(&paths.directory, &paths.record_name, &refreshed)
                .map(|()| None)
                .map_err(|_| deny_locked(&absolute, None));
        }
        if !is_stale(record, now) {
            return Err(deny_locked(&absolute, holder_of(record)));
        }
    }
    let claim = claim_id(agent_id, &absolute, now);
    let record = lock_record(&absolute, agent_id, session_id, &claim, now);
    atomic_write_record(&paths.directory, &paths.record_name, &record)
        .map_err(|_| deny_locked(&absolute, None))?;
    Ok(Some(AcquiredClaim {
        paths,
        claim_id: claim,
    }))
}

fn rollback_claim(claim: &AcquiredClaim) {
    let Ok(_guard) = RecordGuard::acquire(&claim.paths.directory, &claim.paths.guard_name) else {
        return;
    };
    let Ok(Some(record)) = read_record(&claim.paths.directory, &claim.paths.record_name) else {
        return;
    };
    if record.get("claim_id").and_then(Value::as_str) == Some(claim.claim_id.as_str()) {
        let _ = sync_remove(&claim.paths.directory, &claim.paths.record_name);
    }
}

fn rollback(claims: &[AcquiredClaim]) {
    for claim in claims.iter().rev() {
        rollback_claim(claim);
    }
}

/// Acquire locks for `paths`. Returns `Some(deny)` on conflict or unsafe state.
pub(crate) fn acquire_locks(
    payload: &serde_json::Map<String, Value>,
    paths: &[String],
    context: &PolicyContext,
) -> Option<Value> {
    let agent_id = nonempty_str(payload.get("agent_id"))?;
    let session_id = crate::state::session_id(payload)?;
    let directory = match ensure_lock_dir(context) {
        Ok(directory) => directory,
        Err(_) => return paths.first().map(|path| deny_locked(path, None)),
    };
    let mut claims = Vec::new();
    for file_path in paths {
        match acquire_one(
            &directory,
            file_path,
            agent_id,
            session_id,
            context.now_seconds(),
            context.home_dir(),
        ) {
            Ok(Some(claim)) => claims.push(claim),
            Ok(None) => {}
            Err(denied) => {
                rollback(&claims);
                return Some(denied);
            }
        }
    }
    None
}

fn release_record(paths: &RecordPaths, agent_id: &str, session_id: &str) -> std::io::Result<()> {
    let _guard = RecordGuard::acquire(&paths.directory, &paths.guard_name)?;
    let Some(record) = read_record(&paths.directory, &paths.record_name)? else {
        return Ok(());
    };
    if owner_matches(&record, agent_id, session_id) {
        sync_remove(&paths.directory, &paths.record_name)?;
    }
    Ok(())
}

pub(crate) fn release_paths(
    payload: &serde_json::Map<String, Value>,
    paths: &[String],
    context: &PolicyContext,
) {
    let Some(agent_id) = nonempty_str(payload.get("agent_id")) else {
        return;
    };
    let Some(session_id) = crate::state::session_id(payload) else {
        return;
    };
    let Ok(directory) = ensure_lock_dir(context) else {
        return;
    };
    for file_path in paths {
        let absolute = resolve_absolute(file_path, context.home_dir());
        if let Ok(paths) = record_paths(&directory, &absolute) {
            let _ = release_record(&paths, agent_id, session_id);
        }
    }
}

fn digest_from_record_name(name: &str) -> Option<String> {
    let digest = name.strip_suffix(".lock.json")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_owned())
}

pub(crate) fn release_agent_locks(
    payload: &serde_json::Map<String, Value>,
    context: &PolicyContext,
) {
    let Some(agent_id) = nonempty_str(payload.get("agent_id")) else {
        return;
    };
    let Some(session_id) = crate::state::session_id(payload) else {
        return;
    };
    let Ok(directory) = ensure_lock_dir(context) else {
        return;
    };
    let Some(entries) = directory_entries(&directory) else {
        return;
    };
    for name in entries {
        let Some(digest) = digest_from_record_name(&name) else {
            continue;
        };
        let paths = RecordPaths {
            directory: directory
                .try_clone()
                .expect("lock directory fd remains valid"),
            record_name: name,
            guard_name: format!("{digest}.guard"),
        };
        let _ = release_record(&paths, agent_id, session_id);
    }
}
