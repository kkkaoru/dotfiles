//! Session-scoped delegation policy snapshots shared with the PreToolUse hook.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const STATE_VERSION: u64 = 2;
pub const STATE_DIRECTORY: &str = "delegation-state-v2";
pub const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_STATE_BYTES: u64 = 4 * 1024;
const MAX_CLOCK_REORDER_SECONDS: f64 = 24.0 * 60.0 * 60.0;
const LEGACY_STATE_FILE: &str = "delegation-state.json";
const LOCK_WAIT: Duration = Duration::from_millis(100);

struct ExclusiveFileLock(File);

fn try_exclusive_lock(file: &File) -> std::io::Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    let would_block = error
        .raw_os_error()
        .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN);
    if would_block { Ok(false) } else { Err(error) }
}

fn wait_for_exclusive_lock(file: &File) -> std::io::Result<()> {
    let deadline = Instant::now() + LOCK_WAIT;
    while !try_exclusive_lock(file)? {
        if Instant::now() >= deadline {
            return Err(std::io::Error::from(ErrorKind::WouldBlock));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .with_context(|| format!("open delegation lock {}", path.display()))?;
        wait_for_exclusive_lock(&file)
            .with_context(|| format!("lock delegation state {}", path.display()))?;
        Ok(Self(file))
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Prefer Claude Code's current spelling. A present but invalid current field
/// is not rescued by the legacy spelling.
pub fn session_id(payload: &Value) -> Option<&str> {
    let value = match payload.get("session_id") {
        Some(value) => value,
        None => payload.get("sessionId")?,
    };
    valid_session_id(value)
}

fn valid_session_id(value: &Value) -> Option<&str> {
    let id = value.as_str()?.trim();
    (!id.is_empty() && id.len() <= MAX_SESSION_ID_BYTES && !id.chars().any(char::is_control))
        .then_some(id)
}

pub fn session_key(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}

pub fn state_path(cache_dir: &Path, id: &str) -> PathBuf {
    cache_dir
        .join(STATE_DIRECTORY)
        .join(format!("{}.json", session_key(id)))
}

fn contains_ascii_case_insensitive(text: &str, needle: &str) -> bool {
    text.as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

pub fn current_prompt_opts_out(payload: &Value) -> bool {
    let Some(prompt) = crate::lifecycle::prompt(payload) else {
        return false;
    };
    ["do not delegate", "don't delegate", "dont delegate"]
        .into_iter()
        .any(|phrase| contains_ascii_case_insensitive(prompt, phrase))
}

/// Make the routing metadata reflect an explicit opt-out in this prompt. The
/// unmodified branch deliberately leaves genuine worker selections intact.
pub fn effective_summary(mut summary: Value, payload: Option<&Value>) -> Value {
    let opted_out = payload.is_some_and(current_prompt_opts_out);
    if !opted_out {
        return summary;
    }
    let Some(object) = summary.as_object_mut() else {
        return summary;
    };
    object.insert("selected_agents".into(), Value::Array(Vec::new()));
    object.insert("selected_workers".into(), Value::Array(Vec::new()));
    object.insert("preferred_worker".into(), Value::Null);
    object.insert("delegation_required".into(), Value::Bool(false));
    object.insert("direct_main_execution".into(), Value::from("allowed"));
    object.insert("delegation_opt_out".into(), Value::Bool(true));
    summary
}

fn cache_dir(home: &Path) -> PathBuf {
    home.join(".cache/claudex")
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("delegation state directory is not a real directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("delegation state directory is not a real directory");
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn read_bounded_object(path: &Path) -> Option<Value> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
        return None;
    }
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

fn existing_is_newer(path: &Path, key: &str, now: f64) -> bool {
    let Some(existing) = read_bounded_object(path) else {
        return false;
    };
    if existing.get("version").and_then(Value::as_u64) != Some(STATE_VERSION)
        || existing.get("session_key").and_then(Value::as_str) != Some(key)
    {
        return false;
    }
    existing
        .get("updated_at")
        .and_then(Value::as_f64)
        .is_some_and(|stored| {
            stored.is_finite()
                && stored >= 0.0
                && stored > now
                && stored - now <= MAX_CLOCK_REORDER_SECONDS
        })
}

fn lock_path_for(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("delegation state path has no UTF-8 file name")?;
    Ok(path.with_file_name(format!("{name}.lock")))
}

fn write_session_state(path: &Path, key: &str, value: &Value, now: f64) -> Result<()> {
    let lock_path = lock_path_for(path)?;
    let _guard = ExclusiveFileLock::acquire(&lock_path)?;
    if existing_is_newer(path, key, now) {
        return Ok(());
    }
    crate::util::write_private_json(path, value)
}

fn migration_tombstone(now: f64) -> Value {
    serde_json::json!({
        "version": STATE_VERSION,
        "migration": "session-scoped-v2",
        "updated_at": now,
        "delegation_required": false,
        "selected_workers_count": 0,
        "direct_main_execution": "allowed"
    })
}

fn write_legacy_tombstone(cache: &Path, now: f64) -> Result<()> {
    let path = cache.join(LEGACY_STATE_FILE);
    let lock_path = cache.join("delegation-state.migration.lock");
    let _guard = ExclusiveFileLock::acquire(&lock_path)?;
    crate::util::write_private_json(&path, &migration_tombstone(now))
}

fn session_record(summary: &Value, key: &str, now: f64) -> Value {
    let workers = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let required = workers > 0
        && summary
            .get("delegation_required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let mut object = Map::new();
    object.insert("version".into(), Value::from(STATE_VERSION));
    object.insert("session_key".into(), Value::from(key));
    object.insert("updated_at".into(), Value::from(now));
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
pub fn write_delegation_state(
    home: &Path,
    id: Option<&str>,
    summary: &Value,
    now: f64,
) -> Result<()> {
    if !now.is_finite() || now < 0.0 {
        bail!("delegation state timestamp must be finite and non-negative");
    }
    let cache = cache_dir(home);
    ensure_private_directory(&cache)?;
    write_legacy_tombstone(&cache, now)?;
    let Some(id) = id else {
        return Ok(());
    };
    let directory = cache.join(STATE_DIRECTORY);
    ensure_private_directory(&directory)?;
    let key = session_key(id);
    let path = state_path(&cache, id);
    write_session_state(&path, &key, &session_record(summary, &key, now), now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

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
        assert_eq!(second["delegation_required"], false);
        assert_eq!(second["selected_workers_count"], 0);
        assert_ne!(first["session_key"], second["session_key"]);
        let legacy: Value = serde_json::from_slice(
            &fs::read(cache_dir(root.path()).join(LEGACY_STATE_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(legacy["delegation_required"], false);
        assert_eq!(legacy["migration"], "session-scoped-v2");

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
        let legacy: Value =
            serde_json::from_slice(&fs::read(cache.join(LEGACY_STATE_FILE)).unwrap()).unwrap();
        assert_eq!(legacy["delegation_required"], false);
    }

    #[test]
    fn current_prompt_opt_out_does_not_consult_legacy_prompt_or_damage_normal_routes() {
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

        let opted_out = effective_summary(
            summary,
            Some(&serde_json::json!({"prompt":"Please DO NOT DELEGATE this turn"})),
        );
        assert_eq!(opted_out["selected_workers"], serde_json::json!([]));
        assert_eq!(opted_out["delegation_required"], false);
        assert_eq!(opted_out["direct_main_execution"], "allowed");
        assert_eq!(opted_out["delegation_opt_out"], true);
    }
}
