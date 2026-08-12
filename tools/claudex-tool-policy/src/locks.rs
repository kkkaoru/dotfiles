use crate::deny;
use crate::env::nonempty_str;
use crate::policy::PolicyContext;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub(crate) const LOCK_TTL_SECONDS: f64 = 45.0 * 60.0;
const MAX_LOCK_BYTES: u64 = 16 * 1024;
const GUARD_WAIT: Duration = Duration::from_millis(100);
static CLAIM_COUNTER: AtomicU64 = AtomicU64::new(1);

struct RecordPaths {
    record: PathBuf,
    guard: PathBuf,
}

struct RecordGuard(File);

struct AcquiredClaim {
    paths: RecordPaths,
    claim_id: String,
}

fn io_error(kind: ErrorKind, message: &'static str) -> std::io::Error {
    std::io::Error::new(kind, message)
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn owner_controlled_directory(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o022 != 0
    {
        return Err(io_error(
            ErrorKind::PermissionDenied,
            "directory is not owner-controlled",
        ));
    }
    Ok(())
}

fn private_file(file: &File, maximum_bytes: u64) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o022 != 0
        || metadata.len() > maximum_bytes
    {
        return Err(io_error(
            ErrorKind::PermissionDenied,
            "file is not an owner-controlled regular file",
        ));
    }
    Ok(())
}

fn ensure_lock_dir(context: &PolicyContext) -> std::io::Result<PathBuf> {
    owner_controlled_directory(context.cache_dir())?;
    let path = context.cache_dir().join("file-locks");
    match fs::symlink_metadata(&path) {
        Ok(_) => owner_controlled_directory(&path)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            if let Err(create_error) = builder.mode(0o700).create(&path)
                && create_error.kind() != ErrorKind::AlreadyExists
            {
                return Err(create_error);
            }
            owner_controlled_directory(&path)?;
        }
        Err(error) => return Err(error),
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.mode() & 0o777 != 0o700 {
        return Err(io_error(
            ErrorKind::PermissionDenied,
            "file lock directory is not private",
        ));
    }
    Ok(path)
}

fn try_guard_lock(file: &File) -> std::io::Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    let blocked = error
        .raw_os_error()
        .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN);
    if blocked { Ok(false) } else { Err(error) }
}

fn open_or_create_guard(path: &Path) -> std::io::Result<File> {
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
    Err(io_error(ErrorKind::WouldBlock, "file lock guard raced"))
}

fn wait_for_guard(file: &File) -> std::io::Result<()> {
    let deadline = Instant::now() + GUARD_WAIT;
    while !try_guard_lock(file)? {
        if Instant::now() >= deadline {
            return Err(io_error(ErrorKind::WouldBlock, "file lock guard is busy"));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

impl RecordGuard {
    fn acquire(path: &Path) -> std::io::Result<Self> {
        let file = open_or_create_guard(path)?;
        private_file(&file, MAX_LOCK_BYTES)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        wait_for_guard(&file)?;
        Ok(Self(file))
    }
}

impl Drop for RecordGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn path_from_edit(edit: &Value) -> Option<String> {
    let object = edit.as_object()?;
    nonempty_str(object.get("file_path"))
        .or_else(|| nonempty_str(object.get("path")))
        .map(str::to_owned)
}

fn collect_edit_paths(tool_input: &Map<String, Value>, paths: &mut Vec<String>) {
    let Some(edits) = tool_input.get("edits").and_then(Value::as_array) else {
        return;
    };
    paths.extend(edits.iter().filter_map(path_from_edit));
}

pub(crate) fn tool_file_paths(_tool_name: &str, tool_input: &Map<String, Value>) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["file_path", "path", "notebook_path"] {
        paths.extend(nonempty_str(tool_input.get(key)).map(str::to_owned));
    }
    collect_edit_paths(tool_input, &mut paths);
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

fn resolve_absolute(path: &str, home: &Path) -> String {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if path == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(path)
    };
    fs::canonicalize(&expanded)
        .unwrap_or(expanded)
        .to_string_lossy()
        .into_owned()
}

fn record_paths(directory: &Path, absolute: &str) -> RecordPaths {
    let digest = hex::encode(Sha256::digest(absolute.as_bytes()));
    RecordPaths {
        record: directory.join(format!("{digest}.lock.json")),
        guard: directory.join(format!("{digest}.guard")),
    }
}

fn read_record(path: &Path) -> std::io::Result<Option<Value>> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    private_file(&file, MAX_LOCK_BYTES)?;
    let length = file.metadata()?.len();
    let mut payload = Vec::with_capacity(length as usize);
    file.take(MAX_LOCK_BYTES + 1).read_to_end(&mut payload)?;
    if payload.len() as u64 > MAX_LOCK_BYTES {
        return Err(io_error(ErrorKind::InvalidData, "file lock is too large"));
    }
    let value = serde_json::from_slice::<Value>(&payload)
        .map_err(|_| io_error(ErrorKind::InvalidData, "file lock is malformed"))?;
    if !value.is_object() {
        return Err(io_error(
            ErrorKind::InvalidData,
            "file lock is not an object",
        ));
    }
    Ok(Some(value))
}

fn atomic_write_record(path: &Path, record: &Value) -> std::io::Result<()> {
    let payload = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    if payload.len() as u64 > MAX_LOCK_BYTES {
        return Err(io_error(ErrorKind::InvalidData, "file lock is too large"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io_error(ErrorKind::InvalidInput, "file lock has no parent"))?;
    owner_controlled_directory(parent)?;
    let mut temporary = tempfile::Builder::new().tempfile_in(parent)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(&payload)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn sync_remove(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io_error(ErrorKind::InvalidInput, "file lock has no parent"))?;
    fs::remove_file(path)?;
    File::open(parent)?.sync_all()
}

fn lock_is_stale(record: &Value, now: f64) -> bool {
    record
        .get("acquired_at")
        .and_then(Value::as_f64)
        .is_some_and(|acquired| {
            acquired.is_finite()
                && acquired >= 0.0
                && now.is_finite()
                && now >= acquired
                && now - acquired > LOCK_TTL_SECONDS
        })
}

fn record_holder(record: &Value) -> Option<&str> {
    nonempty_str(record.get("agent_id"))
}

fn record_session(record: &Value) -> Option<&str> {
    nonempty_str(record.get("session_id"))
}

fn owner_matches(record: &Value, agent_id: &str, session_id: Option<&str>) -> bool {
    record_holder(record) == Some(agent_id)
        && match (record_session(record), session_id) {
            (Some(recorded), Some(current)) => recorded == current,
            _ => true,
        }
}

fn claim_id(agent_id: &str, absolute: &str, now: f64) -> String {
    let counter = CLAIM_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!(
        "{}:{}:{}:{agent_id}:{absolute}",
        process::id(),
        now.to_bits(),
        counter
    );
    hex::encode(Sha256::digest(seed.as_bytes()))
}

fn lock_record(
    absolute: &str,
    agent_id: &str,
    session_id: Option<&str>,
    claim_id: &str,
    now: f64,
) -> Value {
    serde_json::json!({
        "path": absolute,
        "agent_id": agent_id,
        "session_id": session_id,
        "claim_id": claim_id,
        "pid": process::id(),
        "acquired_at": now,
    })
}

fn deny_locked(absolute: &str, holder: Option<&str>) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "File `{absolute}` is locked by SubAgent `{}`. Partition write scopes so parallel \
             workers do not edit the same path, or wait for that worker to finish before retrying.",
            holder.unwrap_or("another agent")
        ),
    )
}

fn acquire_one(
    directory: &Path,
    file_path: &str,
    agent_id: &str,
    session_id: Option<&str>,
    now: f64,
    home: &Path,
) -> Result<Option<AcquiredClaim>, Value> {
    let absolute = resolve_absolute(file_path, home);
    let paths = record_paths(directory, &absolute);
    let Ok(_guard) = RecordGuard::acquire(&paths.guard) else {
        return Err(deny_locked(&absolute, None));
    };
    let existing = match read_record(&paths.record) {
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
            return atomic_write_record(&paths.record, &refreshed)
                .map(|()| None)
                .map_err(|_| deny_locked(&absolute, None));
        }
        if !lock_is_stale(record, now) {
            return Err(deny_locked(&absolute, record_holder(record)));
        }
    }
    let claim = claim_id(agent_id, &absolute, now);
    let record = lock_record(&absolute, agent_id, session_id, &claim, now);
    atomic_write_record(&paths.record, &record).map_err(|_| deny_locked(&absolute, None))?;
    Ok(Some(AcquiredClaim {
        paths,
        claim_id: claim,
    }))
}

fn rollback_claim(claim: &AcquiredClaim) {
    let Ok(_guard) = RecordGuard::acquire(&claim.paths.guard) else {
        return;
    };
    let Ok(Some(record)) = read_record(&claim.paths.record) else {
        return;
    };
    if record.get("claim_id").and_then(Value::as_str) == Some(claim.claim_id.as_str()) {
        let _ = sync_remove(&claim.paths.record);
    }
}

fn rollback(claims: &[AcquiredClaim]) {
    for claim in claims.iter().rev() {
        rollback_claim(claim);
    }
}

/// Acquire locks for `paths`. Returns `Some(deny)` on conflict or unsafe state.
pub(crate) fn acquire_locks(
    payload: &Map<String, Value>,
    paths: &[String],
    context: &PolicyContext,
) -> Option<Value> {
    let agent_id = nonempty_str(payload.get("agent_id"))?;
    let session_id = crate::state::session_id(payload);
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

fn release_record(
    paths: &RecordPaths,
    agent_id: &str,
    session_id: Option<&str>,
) -> std::io::Result<()> {
    let _guard = RecordGuard::acquire(&paths.guard)?;
    let Some(record) = read_record(&paths.record)? else {
        return Ok(());
    };
    if owner_matches(&record, agent_id, session_id) {
        sync_remove(&paths.record)?;
    }
    Ok(())
}

pub(crate) fn release_paths(
    payload: &Map<String, Value>,
    paths: &[String],
    context: &PolicyContext,
) {
    let Some(agent_id) = nonempty_str(payload.get("agent_id")) else {
        return;
    };
    let session_id = crate::state::session_id(payload);
    let Ok(directory) = ensure_lock_dir(context) else {
        return;
    };
    for file_path in paths {
        let absolute = resolve_absolute(file_path, context.home_dir());
        let paths = record_paths(&directory, &absolute);
        let _ = release_record(&paths, agent_id, session_id);
    }
}

fn digest_from_record_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let digest = name.strip_suffix(".lock.json")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_owned())
}

pub(crate) fn release_agent_locks(payload: &Map<String, Value>, context: &PolicyContext) {
    let Some(agent_id) = nonempty_str(payload.get("agent_id")) else {
        return;
    };
    let session_id = crate::state::session_id(payload);
    let Ok(directory) = ensure_lock_dir(context) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&directory) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(digest) = digest_from_record_name(&path) else {
            continue;
        };
        let paths = RecordPaths {
            record: path,
            guard: directory.join(format!("{digest}.guard")),
        };
        let _ = release_record(&paths, agent_id, session_id);
    }
}
