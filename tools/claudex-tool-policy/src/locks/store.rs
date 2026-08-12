use crate::env::nonempty_str;
use crate::policy::PolicyContext;
use crate::state::current_uid;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::ffi::CStr;
use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::fs::MetadataExt as _;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const LOCK_TTL_SECONDS: f64 = 45.0 * 60.0;
const MAX_LOCK_BYTES: u64 = 16 * 1024;
const GUARD_WAIT: Duration = Duration::from_millis(100);
static CLAIM_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) struct RecordPaths {
    pub(super) directory: File,
    pub(super) record_name: String,
    pub(super) guard_name: String,
}

pub(super) struct RecordGuard(File);

fn io_error(kind: ErrorKind, message: &'static str) -> std::io::Error {
    std::io::Error::new(kind, message)
}

fn private_file(file: &File, maximum_bytes: u64) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > maximum_bytes
    {
        return Err(io_error(
            ErrorKind::PermissionDenied,
            "file is not an owner-controlled regular file",
        ));
    }
    Ok(())
}

pub(super) fn ensure_lock_dir(context: &PolicyContext) -> std::io::Result<File> {
    let cache = crate::state::open_cache_directory(context)?;
    crate::state::open_child_directory(&cache, "file-locks", true)
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

fn open_or_create_guard(directory: &File, name: &str) -> std::io::Result<File> {
    for _ in 0..3 {
        match crate::state::open_at(
            directory,
            name,
            libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        ) {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let name = std::ffi::CString::new(name)
            .map_err(|_| io_error(ErrorKind::InvalidInput, "NUL in lock name"))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        if fd >= 0 {
            // SAFETY: openat returned a new owned descriptor.
            return Ok(unsafe { File::from_raw_fd(fd) });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::AlreadyExists {
            return Err(error);
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
    pub(super) fn acquire(directory: &File, name: &str) -> std::io::Result<Self> {
        let file = open_or_create_guard(directory, name)?;
        private_file(&file, MAX_LOCK_BYTES)?;
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        wait_for_guard(&file)?;
        Ok(Self(file))
    }
}

impl Drop for RecordGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub(super) fn record_paths(directory: &File, absolute: &str) -> std::io::Result<RecordPaths> {
    let digest = hex::encode(Sha256::digest(absolute.as_bytes()));
    Ok(RecordPaths {
        directory: directory.try_clone()?,
        record_name: format!("{digest}.lock.json"),
        guard_name: format!("{digest}.guard"),
    })
}

pub(super) fn read_record(directory: &File, name: &str) -> std::io::Result<Option<Value>> {
    let file = match crate::state::open_at(
        directory,
        name,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    private_file(&file, MAX_LOCK_BYTES)?;
    let length = file.metadata()?.len();
    let mut payload = Vec::with_capacity(length as usize);
    file.take(MAX_LOCK_BYTES + 1).read_to_end(&mut payload)?;
    // The lock record is newline terminated; enforce the limit on the bytes
    // that are actually written, not only on the JSON payload.
    if payload.len() as u64 + 1 > MAX_LOCK_BYTES {
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

pub(super) fn atomic_write_record(
    directory: &File,
    name: &str,
    record: &Value,
) -> std::io::Result<()> {
    let payload = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    if payload.len() as u64 + 1 > MAX_LOCK_BYTES {
        return Err(io_error(ErrorKind::InvalidData, "file lock is too large"));
    }
    let temporary_name = format!(
        ".claudex-lock-{}-{}",
        process::id(),
        CLAIM_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut temporary = crate::state::open_at(
        directory,
        &temporary_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0o600,
    )?;
    temporary.write_all(&payload)?;
    temporary.write_all(b"\n")?;
    temporary.sync_all()?;
    let old_name = std::ffi::CString::new(name)
        .map_err(|_| io_error(ErrorKind::InvalidInput, "NUL in lock name"))?;
    let temporary_name_c = std::ffi::CString::new(temporary_name)
        .map_err(|_| io_error(ErrorKind::InvalidInput, "NUL in temporary lock name"))?;
    let renamed = unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            temporary_name_c.as_ptr(),
            directory.as_raw_fd(),
            old_name.as_ptr(),
        )
    };
    if renamed != 0 {
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temporary_name_c.as_ptr(), 0);
        }
        return Err(std::io::Error::last_os_error());
    }
    directory.sync_all()
}

pub(super) fn sync_remove(directory: &File, name: &str) -> std::io::Result<()> {
    let name = std::ffi::CString::new(name)
        .map_err(|_| io_error(ErrorKind::InvalidInput, "NUL in lock name"))?;
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    directory.sync_all()
}

pub(super) fn directory_entries(directory: &File) -> Option<Vec<String>> {
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return None;
    }
    // SAFETY: duplicate is an owned directory descriptor transferred to
    // fdopendir; closedir closes it on every return path below.
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe { libc::close(duplicate) };
        return None;
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: stream remains valid until closedir below.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        // SAFETY: d_name is NUL-terminated by readdir.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let Ok(name) = name.to_str() else {
            continue;
        };
        if name != "." && name != ".." {
            names.push(name.to_owned());
        }
    }
    unsafe { libc::closedir(stream) };
    Some(names)
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

pub(super) fn owner_matches(record: &Value, agent_id: &str, session_id: &str) -> bool {
    record_holder(record) == Some(agent_id) && record_session(record) == Some(session_id)
}

pub(super) fn claim_id(agent_id: &str, absolute: &str, now: f64) -> String {
    let counter = CLAIM_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!(
        "{}:{}:{counter}:{agent_id}:{absolute}",
        process::id(),
        now.to_bits()
    );
    hex::encode(Sha256::digest(seed.as_bytes()))
}

pub(super) fn lock_record(
    absolute: &str,
    agent_id: &str,
    session_id: &str,
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

pub(super) fn is_stale(record: &Value, now: f64) -> bool {
    lock_is_stale(record, now)
}

pub(super) fn holder_of(record: &Value) -> Option<&str> {
    record_holder(record)
}
