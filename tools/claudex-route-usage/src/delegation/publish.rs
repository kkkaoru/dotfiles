use super::fs::{c_name, open_at, open_cache_layout, validate_private_file};
use super::lock::ExclusiveFileLock;
use super::prompt::session_key;
use super::{
    LEGACY_STATE_FILE, MAX_FUTURE_SKEW_SECONDS, MAX_SELECTED_WORKERS, MAX_STATE_BYTES, STATE_KEYS,
    STATE_TTL_SECONDS, STATE_VERSION,
};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

fn read_bounded_object(directory: &File, name: &str) -> Result<Option<Value>> {
    let file = match open_at(
        directory,
        name,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_private_file(&file, MAX_STATE_BYTES)?;
    let metadata = file.metadata()?;
    let mut payload = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut payload)
        .context("read delegation state")?;
    if payload.len() as u64 > MAX_STATE_BYTES {
        bail!("delegation state exceeds size limit");
    }
    Ok(serde_json::from_slice::<Value>(&payload)
        .ok()
        .filter(Value::is_object))
}

fn write_atomic(directory: &File, name: &str, value: &Value) -> Result<()> {
    let payload = serde_json::to_vec(value)?;
    if payload.len() as u64 > MAX_STATE_BYTES {
        bail!("delegation state exceeds size limit");
    }
    let temporary_name = format!(
        ".claudex-state-{}-{}",
        process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut temporary = open_at(
        directory,
        &temporary_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0o600,
    )
    .context("create temporary delegation state")?;
    let result = (|| -> Result<()> {
        temporary.write_all(&payload)?;
        temporary.sync_all()?;
        let old_name = c_name(name)?;
        let temporary_name = c_name(&temporary_name)?;
        let renamed = unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                temporary_name.as_ptr(),
                directory.as_raw_fd(),
                old_name.as_ptr(),
            )
        };
        if renamed != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        directory.sync_all()?;
        Ok(())
    })();
    if result.is_err()
        && let Ok(temporary_name) = c_name(&temporary_name)
    {
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temporary_name.as_ptr(), 0);
        }
    }
    result
}

fn exact_state_keys(object: &Map<String, Value>) -> bool {
    object.len() == STATE_KEYS.len() && STATE_KEYS.iter().all(|key| object.contains_key(*key))
}

pub(super) fn validated_record<'a>(
    value: &'a Value,
    key: &str,
    now: f64,
) -> Option<&'a Map<String, Value>> {
    let object = value.as_object()?;
    if !exact_state_keys(object)
        || object.get("version")?.as_u64()? != STATE_VERSION
        || object.get("session_key")?.as_str()? != key
    {
        return None;
    }
    let updated = object.get("updated_at")?.as_f64()?;
    let expires = object.get("expires_at")?.as_f64()?;
    if !updated.is_finite()
        || !expires.is_finite()
        || updated < 0.0
        || expires < updated
        || expires - updated != STATE_TTL_SECONDS
        || updated > now + MAX_FUTURE_SKEW_SECONDS
        || now > expires
    {
        return None;
    }
    let base = object.get("base_delegation_required")?.as_bool()?;
    let opted_out = object.get("prompt_opt_out")?.as_bool()?;
    let required = object.get("delegation_required")?.as_bool()?;
    let count = object.get("selected_workers_count")?.as_u64()?;
    let direct = object.get("direct_main_execution")?.as_str()?;
    if count > MAX_SELECTED_WORKERS
        || required != (base && !opted_out)
        || (required && (count == 0 || direct != "fallback-only"))
        || (!required && direct != "allowed")
    {
        return None;
    }
    Some(object)
}

fn existing_is_newer(directory: &File, name: &str, key: &str, now: f64) -> Result<bool> {
    let Some(existing) = read_bounded_object(directory, name)? else {
        return Ok(false);
    };
    Ok(validated_record(&existing, key, now)
        .and_then(|record| record.get("updated_at"))
        .and_then(Value::as_f64)
        .is_some_and(|stored| stored > now))
}

fn write_session_state(directory: &File, key: &str, value: &Value, now: f64) -> Result<()> {
    let name = format!("{key}.json");
    let lock_name = format!("{name}.lock");
    let _guard = ExclusiveFileLock::acquire(directory, &lock_name)?;
    if existing_is_newer(directory, &name, key, now)? {
        return Ok(());
    }
    write_atomic(directory, &name, value)
}

fn migration_tombstone(now: f64) -> Value {
    serde_json::json!({
        "version": 1,
        "updated_at": now,
        "delegation_required": false,
        "selected_workers_count": 0,
        "direct_main_execution": "allowed"
    })
}

fn write_legacy_tombstone(cache: &File, now: f64) -> Result<()> {
    let _guard = ExclusiveFileLock::acquire(cache, "delegation-state.migration.lock")?;
    let already_neutral = read_bounded_object(cache, LEGACY_STATE_FILE)
        .ok()
        .flatten()
        .is_some_and(|state| {
            state.get("delegation_required").and_then(Value::as_bool) == Some(false)
                && state.get("selected_workers_count").and_then(Value::as_u64) == Some(0)
                && state.get("direct_main_execution").and_then(Value::as_str) == Some("allowed")
        });
    if already_neutral {
        return Ok(());
    }
    write_atomic(cache, LEGACY_STATE_FILE, &migration_tombstone(now))
}

pub(super) fn session_record(summary: &Value, key: &str, now: f64) -> Value {
    let workers = crate::hook::effective_workers(summary).len();
    let base = summary
        .get("base_delegation_required")
        .and_then(Value::as_bool)
        .or_else(|| summary.get("delegation_required").and_then(Value::as_bool))
        .unwrap_or(false)
        && workers > 0;
    let opted_out = summary
        .get("prompt_delegation_opt_out")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let required = base && !opted_out;
    let mut object = Map::new();
    object.insert("version".into(), Value::from(STATE_VERSION));
    object.insert("session_key".into(), Value::from(key));
    object.insert("updated_at".into(), Value::from(now));
    object.insert("expires_at".into(), Value::from(now + STATE_TTL_SECONDS));
    object.insert("base_delegation_required".into(), Value::Bool(base));
    object.insert("prompt_opt_out".into(), Value::Bool(opted_out));
    object.insert("delegation_required".into(), Value::Bool(required));
    object.insert("selected_workers_count".into(), Value::from(workers as u64));
    object.insert(
        "direct_main_execution".into(),
        Value::from(if required { "fallback-only" } else { "allowed" }),
    );
    Value::Object(object)
}

/// Publish one UserPromptSubmit policy snapshot. The caller is responsible for
/// keeping this off SubagentStart paths.
pub fn write_delegation_state_at(
    home: &Path,
    cache_root: &Path,
    id: Option<&str>,
    summary: &Value,
    now: f64,
) -> Result<()> {
    if !now.is_finite() || now < 0.0 {
        bail!("delegation state timestamp must be finite and non-negative");
    }
    let (cache, state) = open_cache_layout(home, cache_root)?;
    write_legacy_tombstone(&cache, now)?;
    let Some(id) = id else {
        return Ok(());
    };
    let key = session_key(id);
    let record = session_record(summary, &key, now);
    if validated_record(&record, &key, now).is_none() {
        bail!("refusing to publish inconsistent delegation state");
    }
    write_session_state(&state, &key, &record, now)?;
    // Re-neutralize after v2 publication to narrow the compatibility race
    // with a still-running v1 route hook that does not honor our lock.
    write_legacy_tombstone(&cache, now)
}

/// Resolve the same cache root consumed by the policy hook. This compatibility
/// wrapper is retained for unit callers that provide only HOME.
#[cfg(test)]
pub fn write_delegation_state(
    home: &Path,
    id: Option<&str>,
    summary: &Value,
    now: f64,
) -> Result<()> {
    let cache_root = std::env::var_os("CLAUDEX_CACHE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::cache_dir(home));
    write_delegation_state_at(home, &cache_root, id, summary, now)
}
