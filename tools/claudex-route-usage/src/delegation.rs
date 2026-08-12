//! Session-scoped delegation policy snapshots shared with the PreToolUse hook.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const STATE_VERSION: u64 = 2;
pub const STATE_DIRECTORY: &str = "delegation-state-v2";
pub const MAX_SESSION_ID_BYTES: usize = 256;
pub const STATE_TTL_SECONDS: f64 = 86_400.0;
const MAX_STATE_BYTES: u64 = 16 * 1024;
const MAX_SELECTED_WORKERS: u64 = 256;
const MAX_FUTURE_SKEW_SECONDS: f64 = 300.0;
const LEGACY_STATE_FILE: &str = "delegation-state.json";
const LOCK_WAIT: Duration = Duration::from_millis(100);
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

struct ExclusiveFileLock(File);

fn open_or_create_lock(path: &Path) -> std::io::Result<File> {
    for _ in 0..3 {
        let mut existing = OpenOptions::new();
        existing
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        match existing.open(path) {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut create = OpenOptions::new();
        create
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC);
        match create.open(path) {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::from(ErrorKind::WouldBlock))
}

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
        let file = open_or_create_lock(path)
            .with_context(|| format!("open delegation lock {}", path.display()))?;
        validate_private_file(&file, MAX_STATE_BYTES)
            .with_context(|| format!("validate delegation lock {}", path.display()))?;
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

fn normalized_prompt(prompt: &str) -> String {
    prompt
        .chars()
        .map(|character| match character {
            '\u{2018}' | '\u{2019}' | '\u{02bc}' | '\u{ff07}' => '\'',
            _ if character.is_ascii() => character.to_ascii_lowercase(),
            _ => character,
        })
        .collect()
}

fn word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn contains_bounded_literal(text: &str, literal: &str) -> bool {
    text.match_indices(literal).any(|(offset, matched)| {
        let before = text[..offset].chars().next_back();
        let after = text[offset + matched.len()..].chars().next();
        before.is_none_or(|character| !word_character(character))
            && after.is_none_or(|character| !word_character(character))
    })
}

pub fn current_prompt_opts_out(payload: &Value) -> bool {
    let Some(prompt) = crate::lifecycle::prompt(payload) else {
        return false;
    };
    let normalized = normalized_prompt(prompt);
    let english = [
        "do not delegate",
        "don't delegate",
        "dont delegate",
        "no delegation",
        "without delegation",
        "do not use subagents",
        "don't use subagents",
        "dont use subagents",
        "no subagents",
    ];
    english
        .into_iter()
        .any(|phrase| contains_bounded_literal(&normalized, phrase))
        || ["委譲しないで", "委譲しない", "サブエージェントを使わない"]
            .into_iter()
            .any(|phrase| normalized.contains(phrase))
}

/// Make the routing metadata reflect an explicit opt-out in this prompt. The
/// unmodified branch deliberately leaves genuine worker selections intact.
pub fn effective_summary(mut summary: Value, payload: Option<&Value>) -> Value {
    let opted_out = payload.is_some_and(current_prompt_opts_out);
    let Some(object) = summary.as_object_mut() else {
        return summary;
    };
    let base = object
        .get("delegation_required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let required = base && !opted_out;
    object.insert("base_delegation_required".into(), Value::Bool(base));
    object.insert("prompt_delegation_opt_out".into(), Value::Bool(opted_out));
    object.insert("delegation_required".into(), Value::Bool(required));
    object.insert(
        "direct_main_execution".into(),
        Value::from(if required { "fallback-only" } else { "allowed" }),
    );
    summary
}

#[cfg(test)]
fn cache_dir(home: &Path) -> PathBuf {
    home.join(".cache/claudex")
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn validate_owned_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{} is not a real directory", path.display());
    }
    if metadata.uid() != current_uid() || metadata.mode() & 0o022 != 0 {
        bail!("{} is not an owner-controlled directory", path.display());
    }
    Ok(())
}

fn ensure_owned_directory(path: &Path, private: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("delegation state directory is not a real directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            if let Err(create_error) = builder.mode(0o700).create(path)
                && create_error.kind() != ErrorKind::AlreadyExists
            {
                return Err(create_error.into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    validate_owned_directory(path)?;
    if private {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        validate_owned_directory(path)?;
    }
    Ok(())
}

fn ensure_cache_chain(home: &Path) -> Result<PathBuf> {
    validate_owned_directory(home)?;
    let dot_cache = home.join(".cache");
    ensure_owned_directory(&dot_cache, false)?;
    let cache = dot_cache.join("claudex");
    ensure_owned_directory(&cache, true)?;
    Ok(cache)
}

fn validate_private_file(file: &File, maximum_bytes: u64) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o022 != 0
        || metadata.len() > maximum_bytes
    {
        bail!("file is not a safe owner-controlled regular file");
    }
    Ok(())
}

fn read_bounded_object(path: &Path) -> Result<Option<Value>> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = match options.open(path) {
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

fn exact_state_keys(object: &Map<String, Value>) -> bool {
    object.len() == STATE_KEYS.len() && STATE_KEYS.iter().all(|key| object.contains_key(*key))
}

fn validated_record<'a>(value: &'a Value, key: &str, now: f64) -> Option<&'a Map<String, Value>> {
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
        || expires - updated > STATE_TTL_SECONDS
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

fn existing_is_newer(path: &Path, key: &str, now: f64) -> Result<bool> {
    let Some(existing) = read_bounded_object(path)? else {
        return Ok(false);
    };
    Ok(validated_record(&existing, key, now)
        .and_then(|record| record.get("updated_at"))
        .and_then(Value::as_f64)
        .is_some_and(|stored| stored > now))
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
    if existing_is_newer(path, key, now)? {
        return Ok(());
    }
    crate::util::write_private_json(path, value)
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

fn write_legacy_tombstone(cache: &Path, now: f64) -> Result<()> {
    let path = cache.join(LEGACY_STATE_FILE);
    let lock_path = cache.join("delegation-state.migration.lock");
    let _guard = ExclusiveFileLock::acquire(&lock_path)?;
    let already_neutral = read_bounded_object(&path)
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
    crate::util::write_private_json(&path, &migration_tombstone(now))
}

fn session_record(summary: &Value, key: &str, now: f64) -> Value {
    let workers = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let base = summary
        .get("base_delegation_required")
        .and_then(Value::as_bool)
        .or_else(|| summary.get("delegation_required").and_then(Value::as_bool))
        .unwrap_or(false);
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
pub fn write_delegation_state(
    home: &Path,
    id: Option<&str>,
    summary: &Value,
    now: f64,
) -> Result<()> {
    if !now.is_finite() || now < 0.0 {
        bail!("delegation state timestamp must be finite and non-negative");
    }
    let cache = ensure_cache_chain(home)?;
    write_legacy_tombstone(&cache, now)?;
    let Some(id) = id else {
        return Ok(());
    };
    let directory = cache.join(STATE_DIRECTORY);
    ensure_owned_directory(&directory, true)?;
    let key = session_key(id);
    let path = state_path(&cache, id);
    let record = session_record(summary, &key, now);
    if validated_record(&record, &key, now).is_none() {
        bail!("refusing to publish inconsistent delegation state");
    }
    write_session_state(&path, &key, &record, now)?;
    // Re-neutralize after v2 publication to narrow the compatibility race
    // with a still-running v1 route hook that does not honor our lock.
    write_legacy_tombstone(&cache, now)
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
        assert_eq!(first["base_delegation_required"], true);
        assert_eq!(first["prompt_opt_out"], false);
        assert_eq!(first["expires_at"], 12.0 + STATE_TTL_SECONDS);
        assert_eq!(second["delegation_required"], false);
        assert_eq!(second["selected_workers_count"], 1);
        assert_eq!(second["base_delegation_required"], true);
        assert_eq!(second["prompt_opt_out"], true);
        assert_eq!(second.as_object().unwrap().len(), STATE_KEYS.len());
        assert_ne!(first["session_key"], second["session_key"]);
        let legacy: Value = serde_json::from_slice(
            &fs::read(cache_dir(root.path()).join(LEGACY_STATE_FILE)).unwrap(),
        )
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
        assert_eq!(legacy["version"], 1);
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
}
