use super::live::{holder_is_live, mark_live, unmark_agent, unmark_session};
use super::store::{
    LockIdentity, RecordGuard, RecordPaths, atomic_write_record, claim_id, directory_entries,
    ensure_lock_dir, holder_display, is_stale, is_stealable, lock_record, owner_matches,
    read_record, record_paths, session_matches, sync_remove,
};
use super::{deny_lock_busy, deny_lock_unsafe, deny_locked, resolve_absolute};
use crate::policy::PolicyContext;
use serde_json::Value;
use std::fs::File;
use std::io::ErrorKind;

struct AcquiredClaim {
    paths: RecordPaths,
    claim_id: String,
}

fn identity_from_payload(payload: &serde_json::Map<String, Value>) -> Option<LockIdentity<'_>> {
    Some(LockIdentity {
        agent_id: crate::state::agent_id(payload)?,
        session_id: crate::state::session_id(payload)?,
        agent_type: crate::state::agent_type(payload),
    })
}

fn take_guard(paths: &RecordPaths, absolute: &str) -> Result<RecordGuard, Option<Value>> {
    match RecordGuard::acquire(&paths.directory, &paths.guard_name) {
        Ok(guard) => Ok(guard),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Err(Some(deny_lock_busy(absolute))),
        Err(_) => Err(None),
    }
}

fn existing_record(paths: &RecordPaths, absolute: &str) -> Result<Option<Value>, Value> {
    match read_record(&paths.directory, &paths.record_name) {
        Ok(existing) => Ok(existing),
        Err(error) if error.kind() == ErrorKind::InvalidData => Ok(None),
        Err(_) => Err(deny_lock_unsafe(absolute)),
    }
}

fn refresh_owned(
    paths: &RecordPaths,
    record: &Value,
    identity: &LockIdentity<'_>,
    absolute: &str,
    now: f64,
) -> Option<AcquiredClaim> {
    let current_claim = record
        .get("claim_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| claim_id(identity.agent_id, absolute, now));
    let refreshed = lock_record(absolute, identity, &current_claim, now);
    if atomic_write_record(&paths.directory, &paths.record_name, &refreshed).is_err() {
        return None;
    }
    None
}

fn write_new_claim(
    paths: RecordPaths,
    identity: &LockIdentity<'_>,
    absolute: &str,
    now: f64,
) -> Option<AcquiredClaim> {
    let claim = claim_id(identity.agent_id, absolute, now);
    let record = lock_record(absolute, identity, &claim, now);
    atomic_write_record(&paths.directory, &paths.record_name, &record)
        .ok()
        .map(|()| AcquiredClaim {
            paths,
            claim_id: claim,
        })
}

fn claim_or_refresh(
    paths: RecordPaths,
    existing: Option<Value>,
    identity: &LockIdentity<'_>,
    absolute: &str,
    now: f64,
) -> Result<Option<AcquiredClaim>, Value> {
    if let Some(record) = existing.as_ref() {
        if owner_matches(record, identity.agent_id, identity.session_id) {
            return Ok(refresh_owned(&paths, record, identity, absolute, now));
        }
        let holder_live = holder_is_live(&paths.directory, record);
        if !is_stealable(record, identity.session_id, now, holder_live) {
            return Err(deny_locked(absolute, &holder_display(record)));
        }
    }
    Ok(write_new_claim(paths, identity, absolute, now))
}

fn acquire_one(
    directory: &File,
    file_path: &str,
    identity: &LockIdentity<'_>,
    now: f64,
    home: &std::path::Path,
) -> Result<Option<AcquiredClaim>, Value> {
    let absolute = resolve_absolute(file_path, home);
    let Ok(paths) = record_paths(directory, &absolute) else {
        return Ok(None);
    };
    let _guard = match take_guard(&paths, &absolute) {
        Ok(guard) => guard,
        Err(Some(denied)) => return Err(denied),
        Err(None) => return Ok(None),
    };
    let existing = existing_record(&paths, &absolute)?;
    claim_or_refresh(paths, existing, identity, &absolute, now)
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
/// Lock-store failures fail open so a broken cache cannot block every write.
pub(crate) fn acquire_locks(
    payload: &serde_json::Map<String, Value>,
    paths: &[String],
    context: &PolicyContext,
) -> Option<Value> {
    let identity = identity_from_payload(payload)?;
    let Ok(directory) = ensure_lock_dir(context) else {
        return None;
    };
    let now = context.now_seconds();
    let mut claims = Vec::new();
    for file_path in paths {
        match acquire_one(&directory, file_path, &identity, now, context.home_dir()) {
            Ok(claim) => {
                mark_live(&directory, &identity, now);
                claims.extend(claim);
            }
            Err(denied) => {
                rollback(&claims);
                return Some(denied);
            }
        }
    }
    None
}

fn release_record(
    paths: &RecordPaths,
    should_remove: impl Fn(&Value) -> bool,
) -> std::io::Result<()> {
    let _guard = RecordGuard::acquire(&paths.directory, &paths.guard_name)?;
    let Some(record) = read_record(&paths.directory, &paths.record_name)? else {
        return Ok(());
    };
    if should_remove(&record) {
        sync_remove(&paths.directory, &paths.record_name)?;
    }
    Ok(())
}

pub(crate) fn release_paths(
    payload: &serde_json::Map<String, Value>,
    paths: &[String],
    context: &PolicyContext,
) {
    let Some(identity) = identity_from_payload(payload) else {
        return;
    };
    let Ok(directory) = ensure_lock_dir(context) else {
        return;
    };
    let now = context.now_seconds();
    for file_path in paths {
        release_one_path(&directory, file_path, context, &identity, now);
    }
}

fn owned_or_stale(record: &Value, identity: &LockIdentity<'_>, now: f64) -> bool {
    is_stale(record, now) || owner_matches(record, identity.agent_id, identity.session_id)
}

fn release_one_path(
    directory: &File,
    file_path: &str,
    context: &PolicyContext,
    identity: &LockIdentity<'_>,
    now: f64,
) {
    let absolute = resolve_absolute(file_path, context.home_dir());
    let Ok(paths) = record_paths(directory, &absolute) else {
        return;
    };
    let released = release_record(&paths, |record| owned_or_stale(record, identity, now));
    drop(released);
}

fn digest_from_record_name(name: &str) -> Option<String> {
    let digest = name.strip_suffix(".lock.json")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_owned())
}

fn release_matching(context: &PolicyContext, should_remove: impl Fn(&Value) -> bool) {
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
        let _ = release_record(&paths, &should_remove);
    }
}

pub(crate) fn release_agent_locks(
    payload: &serde_json::Map<String, Value>,
    context: &PolicyContext,
) {
    let Some(identity) = identity_from_payload(payload) else {
        return;
    };
    let now = context.now_seconds();
    release_matching(context, |record| {
        is_stale(record, now) || owner_matches(record, identity.agent_id, identity.session_id)
    });
    if let Ok(directory) = ensure_lock_dir(context) {
        unmark_agent(&directory, &identity);
    }
}

pub(crate) fn release_session_locks(
    payload: &serde_json::Map<String, Value>,
    context: &PolicyContext,
) {
    let Some(session_id) = crate::state::session_id(payload) else {
        return;
    };
    let now = context.now_seconds();
    release_matching(context, |record| {
        is_stale(record, now) || session_matches(record, session_id)
    });
    if let Ok(directory) = ensure_lock_dir(context) {
        unmark_session(&directory, session_id);
    }
}
