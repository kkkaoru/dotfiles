use crate::policy::PolicyContext;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
#[cfg(test)]
use std::path::PathBuf;
use std::path::{Component, Path};

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

fn aliased_str<'a>(
    payload: &'a Map<String, Value>,
    current: &str,
    legacy: &str,
) -> Option<&'a str> {
    let value = match payload.get(current) {
        Some(value) => value,
        None => payload.get(legacy)?,
    };
    crate::env::nonempty_str(Some(value))
}

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

/// Claude Code SubAgent hooks send `agent_id` or `agentId`.
pub(crate) fn agent_id(payload: &Map<String, Value>) -> Option<&str> {
    aliased_str(payload, "agent_id", "agentId")
}

/// Claude Code SubAgent hooks send `agent_type` or `agentType`.
pub(crate) fn agent_type(payload: &Map<String, Value>) -> Option<&str> {
    aliased_str(payload, "agent_type", "agentType")
}

fn session_key(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}

#[cfg(test)]
pub(crate) fn state_path(cache_dir: &Path, id: &str) -> PathBuf {
    cache_dir
        .join(STATE_DIRECTORY)
        .join(format!("{}.json", session_key(id)))
}

pub(crate) fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn c_name(name: &str) -> std::io::Result<CString> {
    CString::new(name)
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidInput, "NUL in directory name"))
}

pub(crate) fn open_at(
    directory: &File,
    name: &str,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> std::io::Result<File> {
    let name = c_name(name)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn validate_directory(directory: &File, private: bool) -> std::io::Result<()> {
    let metadata = directory.metadata()?;
    let owner = metadata.uid() == current_uid() || (!private && metadata.uid() == 0);
    if !metadata.is_dir()
        || !owner
        || metadata.mode() & 0o022 != 0
        || (private && metadata.mode() & 0o777 != 0o700)
    {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "directory is not an owner-controlled private directory",
        ));
    }
    Ok(())
}

fn next_path_segment(component: Component<'_>) -> std::io::Result<Option<&std::ffi::OsStr>> {
    match component {
        Component::RootDir => Ok(None),
        Component::Normal(segment) => Ok(Some(segment)),
        _ => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "cache directory contains unsafe path components",
        )),
    }
}

fn mkdir_private(parent: &File, name: &str) -> std::io::Result<()> {
    let name_c = c_name(name)?;
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), 0o700) };
    if result == 0 {
        return Ok(());
    }
    let create_error = std::io::Error::last_os_error();
    if create_error.kind() == ErrorKind::AlreadyExists {
        Ok(())
    } else {
        Err(create_error)
    }
}

pub(crate) fn open_child_directory(
    parent: &File,
    name: &str,
    private: bool,
) -> std::io::Result<File> {
    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY;
    let child = match open_at(parent, name, flags, 0) {
        Ok(child) => child,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            mkdir_private(parent, name)?;
            open_at(parent, name, flags, 0)?
        }
        Err(error) => return Err(error),
    };
    validate_directory(&child, private)?;
    Ok(child)
}

fn open_directory_path(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "cache directory must be absolute",
        ));
    }
    // macOS exposes /var and /tmp as stable system symlinks to /private/var
    // and /private/tmp respectively. Resolve those aliases once, then walk the
    // resulting path exclusively through directory fds with O_NOFOLLOW;
    // user-controlled cache ancestors remain rejected.
    let path = if (path.starts_with("/var") && Path::new("/var").is_symlink())
        || (path.starts_with("/tmp") && Path::new("/tmp").is_symlink())
    {
        Path::new("/private").join(path.strip_prefix("/").unwrap_or(path))
    } else {
        path.to_path_buf()
    };
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open("/")?;
    validate_directory(&root, false)?;
    let mut current = root;
    for component in path.components() {
        let Some(segment) = next_path_segment(component)? else {
            continue;
        };
        let name = segment.to_str().ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                "cache directory contains non-UTF-8 path components",
            )
        })?;
        current = open_child_directory(&current, name, false)?;
    }
    Ok(current)
}

pub(crate) fn open_cache_directory(context: &PolicyContext) -> std::io::Result<File> {
    let cache = context.cache_dir();
    let default = context.home_dir().join(".cache/claudex");
    if cache == default {
        let home = open_directory_path(context.home_dir())?;
        validate_directory(&home, false)?;
        let dot_cache = open_child_directory(&home, ".cache", false)?;
        return open_child_directory(&dot_cache, "claudex", true);
    }
    let directory = open_directory_path(cache)?;
    validate_directory(&directory, false)?;
    Ok(directory)
}

pub(crate) fn open_state_directory(context: &PolicyContext) -> std::io::Result<File> {
    let cache = open_cache_directory(context)?;
    open_child_directory(&cache, STATE_DIRECTORY, true)
}

pub(crate) fn owner_controlled_file(file: &File) -> bool {
    file.metadata().ok().is_some_and(|metadata| {
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == current_uid()
            && metadata.mode() & 0o777 == 0o600
            && metadata.len() <= MAX_STATE_BYTES
    })
}

fn read_bounded_object(directory: &File, name: &str) -> Option<Value> {
    let file = open_at(
        directory,
        name,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    )
    .ok()?;
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
        && expires - updated == STATE_TTL_SECONDS
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
    let expected_key = session_key(id);
    let Ok(directory) = open_state_directory(context) else {
        return false;
    };
    let name = format!("{expected_key}.json");
    read_bounded_object(&directory, &name)
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
    fn agent_id_accepts_camel_case_when_snake_case_is_absent() {
        let camel = serde_json::json!({"agentId":"agent-camel"});
        assert_eq!(agent_id(camel.as_object().unwrap()), Some("agent-camel"));
        let snake = serde_json::json!({"agent_id":"agent-snake", "agentId":"agent-camel"});
        assert_eq!(agent_id(snake.as_object().unwrap()), Some("agent-snake"));
        let empty = serde_json::json!({"agent_id":"", "agentId":"agent-camel"});
        assert_eq!(agent_id(empty.as_object().unwrap()), None);
        let typed = serde_json::json!({"agentType":"claudex-grok"});
        assert_eq!(agent_type(typed.as_object().unwrap()), Some("claudex-grok"));
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

    #[test]
    fn short_ttl_is_rejected_even_before_expiry() {
        let state = serde_json::json!({
            "version": 2,
            "session_key": "key",
            "updated_at": 1_000.0,
            "expires_at": 1_000.0 + STATE_TTL_SECONDS - 1.0,
            "base_delegation_required": true,
            "prompt_opt_out": false,
            "delegation_required": true,
            "selected_workers_count": 1,
            "direct_main_execution": "fallback-only"
        });
        assert!(validated_requirement(&state, "key", 1_001.0).is_none());
    }
}
