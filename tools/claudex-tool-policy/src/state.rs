use crate::policy::PolicyContext;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};

const STATE_VERSION: u64 = 2;
const STATE_DIRECTORY: &str = "delegation-state-v2";
const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_STATE_BYTES: u64 = 16 * 1024;
const MAX_SELECTED_WORKERS: u64 = 256;
const STATE_TTL_SECONDS: f64 = 86_400.0;
const MAX_FUTURE_SKEW_SECONDS: f64 = 300.0;
const STATE_KEYS: &[&str] = &[
    "version",
    "session_key",
    "updated_at",
    "expires_at",
    "base_delegation_required",
    "prompt_opt_out",
    "delegation_required",
    "selected_workers_count",
    "direct_main_execution",
];

/// Prefer the current snake-case field. Its presence prevents fallback to the
/// legacy camel-case field even when its value is malformed.
pub(crate) fn session_id(payload: &Map<String, Value>) -> Option<&str> {
    let value = match payload.get("session_id") {
        Some(value) => value,
        None => payload.get("sessionId")?,
    };
    let id = value.as_str()?.trim();
    (!id.is_empty() && id.len() <= MAX_SESSION_ID_BYTES && !id.chars().any(char::is_control))
        .then_some(id)
}

fn session_key(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}

pub(crate) fn state_path(cache_dir: &Path, id: &str) -> PathBuf {
    cache_dir
        .join(STATE_DIRECTORY)
        .join(format!("{}.json", session_key(id)))
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn owner_controlled_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).ok().is_some_and(|metadata| {
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == current_uid()
            && metadata.mode() & 0o022 == 0
    })
}

fn owner_controlled_cache_chain(home_dir: &Path, cache_dir: &Path) -> bool {
    if !owner_controlled_directory(cache_dir) {
        return false;
    }
    let Ok(relative) = cache_dir.strip_prefix(home_dir) else {
        // An explicit cache override may live outside HOME. Its parent is not
        // part of the configured trust root, so validate the cache itself.
        return true;
    };
    if !owner_controlled_directory(home_dir) {
        return false;
    }
    let mut current = home_dir.to_path_buf();
    relative.components().all(|component| {
        let Component::Normal(segment) = component else {
            return false;
        };
        current.push(segment);
        owner_controlled_directory(&current)
    })
}

fn state_directory_is_safe(context: &PolicyContext) -> bool {
    owner_controlled_cache_chain(context.home_dir(), context.cache_dir())
        && owner_controlled_directory(&context.cache_dir().join(STATE_DIRECTORY))
}

fn owner_controlled_file(file: &File) -> bool {
    file.metadata().ok().is_some_and(|metadata| {
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == current_uid()
            && metadata.mode() & 0o022 == 0
            && metadata.len() <= MAX_STATE_BYTES
    })
}

fn read_bounded_object(path: &Path) -> Option<Value> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = options.open(path).ok()?;
    if !owner_controlled_file(&file) {
        return None;
    }
    let metadata = file.metadata().ok()?;
    let mut payload = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut payload)
        .ok()?;
    if payload.len() as u64 > MAX_STATE_BYTES {
        return None;
    }
    serde_json::from_slice::<Value>(&payload)
        .ok()
        .filter(Value::is_object)
}

fn valid_timestamps(state: &Value, now: f64) -> bool {
    if !now.is_finite() || now < 0.0 {
        return false;
    }
    let Some(updated) = state.get("updated_at").and_then(Value::as_f64) else {
        return false;
    };
    let Some(expires) = state.get("expires_at").and_then(Value::as_f64) else {
        return false;
    };
    updated.is_finite()
        && expires.is_finite()
        && updated >= 0.0
        && expires >= updated
        && expires - updated <= STATE_TTL_SECONDS
        && updated <= now + MAX_FUTURE_SKEW_SECONDS
        && now <= expires
}

fn validated_requirement(state: &Value, expected_key: &str, now: f64) -> Option<bool> {
    let object = state.as_object()?;
    if object.len() != STATE_KEYS.len()
        || !STATE_KEYS.iter().all(|key| object.contains_key(*key))
        || state.get("version").and_then(Value::as_u64) != Some(STATE_VERSION)
        || state.get("session_key").and_then(Value::as_str) != Some(expected_key)
        || !valid_timestamps(state, now)
    {
        return None;
    }
    let base = state.get("base_delegation_required")?.as_bool()?;
    let opted_out = state.get("prompt_opt_out")?.as_bool()?;
    let required = state.get("delegation_required")?.as_bool()?;
    let count = state.get("selected_workers_count")?.as_u64()?;
    if count > MAX_SELECTED_WORKERS || required != (base && !opted_out) {
        return None;
    }
    let direct = state.get("direct_main_execution")?.as_str()?;
    if (required && (count == 0 || direct != "fallback-only")) || (!required && direct != "allowed")
    {
        return None;
    }
    Some(required)
}

/// Invalid, stale, missing, or cross-session state always fails open.
pub(crate) fn delegation_required(payload: &Map<String, Value>, context: &PolicyContext) -> bool {
    if !context.subagent_first() || context.allow_main_tools() {
        return false;
    }
    let Some(id) = session_id(payload) else {
        return false;
    };
    if !state_directory_is_safe(context) {
        return false;
    }
    let expected_key = session_key(id);
    read_bounded_object(&state_path(context.cache_dir(), id))
        .and_then(|state| validated_requirement(&state, &expected_key, context.now_seconds()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_session_field_is_authoritative() {
        let current = serde_json::json!({"session_id":"current", "sessionId":"legacy"});
        assert_eq!(session_id(current.as_object().unwrap()), Some("current"));
        let legacy = serde_json::json!({"sessionId":"legacy"});
        assert_eq!(session_id(legacy.as_object().unwrap()), Some("legacy"));
        let invalid = serde_json::json!({"session_id":42, "sessionId":"legacy"});
        assert_eq!(session_id(invalid.as_object().unwrap()), None);
        let multibyte = "é".repeat((MAX_SESSION_ID_BYTES / 2) + 1);
        let oversized = serde_json::json!({"session_id":multibyte});
        assert_eq!(session_id(oversized.as_object().unwrap()), None);
    }

    #[test]
    fn state_path_hashes_untrusted_session_id() {
        let cache = Path::new("/tmp/cache");
        let path = state_path(cache, "../../../escape");
        assert_eq!(path.parent(), Some(cache.join(STATE_DIRECTORY).as_path()));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 69);
        assert!(!name.contains(".."));
    }
}
