use crate::deny;
use crate::env::{home_dir, load_json_object, nonempty_str, now_seconds};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;

pub(crate) const LOCK_TTL_SECONDS: f64 = 45.0 * 60.0;

pub(crate) fn lock_dir() -> PathBuf {
    let path = crate::env::cache_dir().join("file-locks");
    let _ = fs::create_dir_all(&path);
    path
}

fn path_from_edit(edit: &Value) -> Option<String> {
    let obj = edit.as_object()?;
    nonempty_str(obj.get("file_path"))
        .or_else(|| nonempty_str(obj.get("path")))
        .map(str::to_string)
}

fn collect_edit_paths(tool_input: &Map<String, Value>, paths: &mut Vec<String>) {
    let Some(edits) = tool_input.get("edits").and_then(Value::as_array) else {
        return;
    };
    for edit in edits {
        if let Some(path) = path_from_edit(edit) {
            paths.push(path);
        }
    }
}

pub(crate) fn tool_file_paths(_tool_name: &str, tool_input: &Map<String, Value>) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(value) = nonempty_str(tool_input.get(key)) {
            paths.push(value.to_string());
        }
    }
    collect_edit_paths(tool_input, &mut paths);
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

fn resolve_absolute(path: &str) -> String {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else if path == "~" {
        home_dir()
    } else {
        PathBuf::from(path)
    };
    fs::canonicalize(&expanded)
        .unwrap_or(expanded)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn lock_file_for(path: &str) -> PathBuf {
    let absolute = resolve_absolute(path);
    let digest = hex::encode(Sha256::digest(absolute.as_bytes()));
    lock_dir().join(format!("{digest}.lock.json"))
}

fn lock_is_stale(lock: &Value, now: f64) -> bool {
    lock.get("acquired_at")
        .and_then(Value::as_f64)
        .is_some_and(|acquired_at| now - acquired_at > LOCK_TTL_SECONDS)
}

fn write_lock(path: &Path, payload: &Value) {
    if let Ok(text) = serde_json::to_string(payload) {
        let _ = fs::write(path, format!("{text}\n"));
        let _ = fs::set_permissions(path, Permissions::from_mode(0o600));
    }
}

fn lock_record(absolute: &str, agent_id: &str, session_id: Option<&Value>, now: f64) -> Value {
    let mut map = Map::new();
    map.insert("path".into(), Value::String(absolute.into()));
    map.insert("agent_id".into(), Value::String(agent_id.into()));
    map.insert(
        "session_id".into(),
        session_id.cloned().unwrap_or(Value::Null),
    );
    map.insert("pid".into(), Value::from(process::id()));
    map.insert("acquired_at".into(), Value::from(now));
    Value::Object(map)
}

fn deny_locked(absolute: &str, holder: Option<&str>) -> Value {
    let holder_text = holder.unwrap_or("another agent");
    deny(
        "PreToolUse",
        &format!(
            "File `{absolute}` is locked by SubAgent `{holder_text}`. \
             Partition write scopes so parallel workers do not edit the same path, \
             or wait for that worker to finish before retrying."
        ),
    )
}

fn deny_locked_race(absolute: &str, holder: Option<&str>) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "File `{absolute}` is locked by SubAgent `{}`. \
             Partition write scopes so parallel workers do not edit the same path.",
            holder.unwrap_or("another agent")
        ),
    )
}

fn rollback(acquired: &[PathBuf]) {
    for path in acquired {
        let _ = fs::remove_file(path);
    }
}

fn refresh_own_lock(lock_path: &Path, mut existing: Value, now: f64) {
    if let Some(obj) = existing.as_object_mut() {
        obj.insert("acquired_at".into(), Value::from(now));
        obj.insert("pid".into(), Value::from(process::id()));
    }
    write_lock(lock_path, &existing);
}

/// `Ok(true)` continue to create; `Ok(false)` already refreshed; `Err` deny.
fn reconcile_existing(
    lock_path: &Path,
    existing: Value,
    agent_id: &str,
    absolute: &str,
    now: f64,
    acquired: &[PathBuf],
) -> Result<bool, Value> {
    let holder = existing.get("agent_id").and_then(Value::as_str);
    if holder == Some(agent_id) {
        refresh_own_lock(lock_path, existing, now);
        return Ok(false);
    }
    if lock_is_stale(&existing, now) {
        let _ = fs::remove_file(lock_path);
        return Ok(true);
    }
    rollback(acquired);
    Err(deny_locked(absolute, holder))
}

fn write_lock_file(file: &mut File, record: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string(record).map_err(std::io::Error::other)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn claim_after_race(
    lock_path: &Path,
    record: &Value,
    agent_id: &str,
    absolute: &str,
    now: f64,
    acquired: &mut Vec<PathBuf>,
) -> Result<(), Value> {
    let existing = load_json_object(lock_path).unwrap_or(Value::Object(Map::new()));
    let holder = existing.get("agent_id").and_then(Value::as_str);
    if holder == Some(agent_id) || lock_is_stale(&existing, now) {
        write_lock(lock_path, record);
        acquired.push(lock_path.to_path_buf());
        return Ok(());
    }
    rollback(acquired);
    Err(deny_locked_race(absolute, holder))
}

fn create_lock(
    lock_path: &Path,
    absolute: &str,
    agent_id: &str,
    session_id: Option<&Value>,
    now: f64,
    acquired: &mut Vec<PathBuf>,
) -> Result<(), Value> {
    let record = lock_record(absolute, agent_id, session_id, now);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(lock_path)
    {
        Ok(mut file) => {
            if write_lock_file(&mut file, &record).is_err() {
                let _ = fs::remove_file(lock_path);
                rollback(acquired);
                return Err(deny_locked(absolute, None));
            }
            acquired.push(lock_path.to_path_buf());
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            claim_after_race(lock_path, &record, agent_id, absolute, now, acquired)
        }
        Err(_) => {
            rollback(acquired);
            Err(deny_locked(absolute, None))
        }
    }
}

fn acquire_one(
    file_path: &str,
    agent_id: &str,
    session_id: Option<&Value>,
    now: f64,
    acquired: &mut Vec<PathBuf>,
) -> Result<(), Value> {
    let lock_path = lock_file_for(file_path);
    let absolute = resolve_absolute(file_path);
    if let Some(existing) = load_json_object(&lock_path) {
        match reconcile_existing(&lock_path, existing, agent_id, &absolute, now, acquired)? {
            true => {}
            false => return Ok(()),
        }
    }
    create_lock(&lock_path, &absolute, agent_id, session_id, now, acquired)
}

/// Acquire locks for `paths`. Returns `Some(deny)` on conflict.
pub(crate) fn acquire_locks(payload: &Map<String, Value>, paths: &[String]) -> Option<Value> {
    let agent_id = nonempty_str(payload.get("agent_id"))?;
    let session_id = payload.get("session_id");
    let now = now_seconds();
    let mut acquired = Vec::new();
    for file_path in paths {
        if let Err(denied) = acquire_one(file_path, agent_id, session_id, now, &mut acquired) {
            return Some(denied);
        }
    }
    None
}

pub(crate) fn release_paths(agent_id: &str, paths: &[String]) {
    for file_path in paths {
        let lock_path = lock_file_for(file_path);
        let Some(existing) = load_json_object(&lock_path) else {
            continue;
        };
        if existing.get("agent_id").and_then(Value::as_str) == Some(agent_id) {
            let _ = fs::remove_file(lock_path);
        }
    }
}

fn is_lock_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".lock.json"))
}

pub(crate) fn release_agent_locks(agent_id: &str) {
    let Ok(entries) = fs::read_dir(lock_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_lock_file(&path) {
            continue;
        }
        let Some(existing) = load_json_object(&path) else {
            continue;
        };
        if existing.get("agent_id").and_then(Value::as_str) == Some(agent_id) {
            let _ = fs::remove_file(path);
        }
    }
}
