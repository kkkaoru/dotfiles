//! Kernel-owned single-flight routing snapshot refresh.

mod capability;

use crate::{trusted, util};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const WORKER_TIMEOUT: Duration = Duration::from_secs(120);
const DUPLICATE_FD_MINIMUM: RawFd = 64;
const SAFE_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
const SNAPSHOT_ENV: &[&str] = &[
    "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS",
    "CLAUDEX_DISABLED_SUBAGENT_MODELS",
    "CLAUDEX_OLLAMA_BASE_URL",
    "CLAUDEX_SUBAGENT_MAX_PARALLEL",
    "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS",
    "CLAUDEX_SUBAGENT_MIN_PARALLEL",
    "CLAUDEX_SUBAGENT_ACTIVE_FLOOR",
    "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES",
    "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION",
    "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS",
    "CLAUDEX_SUBAGENT_REUSE",
    "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT",
    "CLAUDEX_SUBAGENT_FIRST",
    "CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS",
    "CLAUDEX_CUSTOM_ADVISOR",
];

pub struct SpawnRequest<'a> {
    pub cache_path: &'a Path,
    pub home: &'a Path,
    pub config_path: &'a Path,
    pub disabled_path: &'a Path,
    pub codexbar_program: &'a str,
    pub curl_program: &'a str,
    pub configuration_key: &'a str,
}

#[derive(Debug)]
pub struct WorkerGuard {
    lock: File,
    configuration_key: String,
    baseline_generation: u64,
    baseline_cache_key: Option<String>,
}

impl WorkerGuard {
    pub fn owns_configuration(&self, key: &str) -> bool {
        self.configuration_key == key
    }

    pub fn baseline_generation(&self) -> u64 {
        self.baseline_generation
    }
}

pub fn lock_path(cache_path: &Path) -> PathBuf {
    cache_path.with_file_name("usage-routing.refresh.lock")
}

/// Start a detached refresh only when this hook owns the kernel lock.
pub fn schedule(request: &SpawnRequest<'_>, cache_is_fresh: bool) -> Result<bool> {
    schedule_with(request, cache_is_fresh, spawn_worker)
}

fn schedule_with<F>(request: &SpawnRequest<'_>, cache_is_fresh: bool, spawn: F) -> Result<bool>
where
    F: FnOnce(&SpawnRequest<'_>, &File, &File) -> Result<()>,
{
    if cache_is_fresh {
        return Ok(false);
    }
    let Some(parent) = request.cache_path.parent() else {
        bail!("routing cache path must have a parent directory");
    };
    fs::create_dir_all(parent)?;
    let mut lock = private_lock_file(&lock_path(request.cache_path))?;
    if !lock_file(&lock, true)? {
        return Ok(false);
    }
    let baseline = util::cache_head(request.cache_path);
    let generation = baseline.as_ref().map_or(0, |(generation, _)| *generation);
    let secret = random_128()?;
    write_metadata(
        &mut lock,
        request.configuration_key,
        generation,
        baseline.as_ref().map(|(_, key)| key.as_str()),
        &secret,
    )?;
    let (ticket_reader, mut ticket_writer) = ticket_pipe()?;
    ticket_writer.write_all(&secret)?;
    drop(ticket_writer);
    spawn(request, &lock, &ticket_reader)?;
    Ok(true)
}

fn private_lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    use std::os::unix::fs::OpenOptionsExt as _;
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(path).context("open routing refresh lock")?;
    validate_lock(&file)?;
    Ok(file)
}

fn validate_lock(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    use std::os::unix::fs::MetadataExt as _;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        bail!("routing refresh lock must be an owner-controlled regular file");
    }
    Ok(())
}

fn lock_file(file: &File, nonblocking: bool) -> Result<bool> {
    let operation = libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 };
    let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if nonblocking
        && matches!(error.raw_os_error(), Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN)
    {
        Ok(false)
    } else {
        Err(error).context("lock routing refresh single-flight file")
    }
}

fn write_metadata(
    lock: &mut File,
    key: &str,
    generation: u64,
    baseline_key: Option<&str>,
    secret: &[u8; 16],
) -> Result<()> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "version": 3,
        "configuration_key": key,
        "baseline_generation": generation,
        "baseline_cache_key": baseline_key,
        "ticket_digest": hex::encode(Sha256::digest(secret)),
    }))?;
    lock.seek(SeekFrom::Start(0))?;
    lock.set_len(0)?;
    lock.write_all(&payload)?;
    lock.sync_all()?;
    Ok(())
}

fn random_128() -> Result<[u8; 16]> {
    let mut random = File::open("/dev/urandom").context("open OS random source")?;
    let mut token = [0_u8; 16];
    random.read_exact(&mut token)?;
    Ok(token)
}

fn ticket_pipe() -> Result<(File, File)> {
    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error()).context("create refresh ticket pipe");
    }
    let reader = unsafe { File::from_raw_fd(descriptors[0]) };
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };
    set_close_on_exec(&reader)?;
    set_close_on_exec(&writer)?;
    Ok((reader, writer))
}

fn set_close_on_exec(file: &File) -> Result<()> {
    let descriptor = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags == -1
        || unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1
    {
        return Err(std::io::Error::last_os_error())
            .context("secure refresh capability descriptor");
    }
    Ok(())
}

fn duplicate_high(file: &File) -> Result<File> {
    let descriptor = unsafe {
        libc::fcntl(
            file.as_raw_fd(),
            libc::F_DUPFD_CLOEXEC,
            DUPLICATE_FD_MINIMUM,
        )
    };
    if descriptor == -1 {
        return Err(std::io::Error::last_os_error()).context("duplicate refresh capability");
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn spawn_worker(request: &SpawnRequest<'_>, lock: &File, ticket: &File) -> Result<()> {
    let executable = trusted::executable(
        std::env::current_exe()
            .context("resolve refresh worker executable")?
            .to_str()
            .context("refresh worker executable is not UTF-8")?,
    )?;
    let codexbar = trusted::executable(request.codexbar_program)?;
    let curl = trusted::executable(request.curl_program)?;
    let home = trusted::data_directory(request.home, "refresh HOME")?;
    let config = trusted::data_file(request.config_path, "provider config")?;
    let disabled = trusted::data_file(request.disabled_path, "disabled-model config")?;
    let lock = duplicate_high(lock)?;
    let ticket = duplicate_high(ticket)?;
    let mut command = Command::new(executable);
    command
        .arg("--refresh-cache-worker")
        .arg("--refresh-lock-fd")
        .arg(lock.as_raw_fd().to_string())
        .arg("--refresh-ticket-fd")
        .arg(ticket.as_raw_fd().to_string())
        .arg("--config")
        .arg(config)
        .arg("--disabled-models-config")
        .arg(disabled)
        .arg("--codexbar-program")
        .arg(codexbar)
        .arg("--curl-program")
        .arg(curl)
        .env_clear()
        .env("HOME", home)
        .env("PATH", SAFE_PATH)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    copy_snapshot_environment(&mut command);
    configure_worker(&mut command, lock.as_raw_fd(), ticket.as_raw_fd());
    command
        .spawn()
        .context("spawn routing cache refresh worker")?;
    Ok(())
}

fn copy_snapshot_environment(command: &mut Command) {
    for name in SNAPSHOT_ENV {
        if let Some(value) = std::env::var_os(name).filter(|value| safe_environment(value)) {
            command.env(name, value);
        }
    }
}

fn safe_environment(value: &OsStr) -> bool {
    let bytes = value.as_encoded_bytes();
    bytes.len() <= 4096 && !bytes.contains(&0) && !bytes.contains(&b'\n') && !bytes.contains(&b'\r')
}

fn configure_worker(command: &mut Command, lock: RawFd, ticket: RawFd) {
    use std::os::unix::process::CommandExt as _;
    unsafe {
        command.pre_exec(move || {
            clear_close_on_exec(lock)?;
            clear_close_on_exec(ticket)?;
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

unsafe fn clear_close_on_exec(descriptor: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags == -1
        || unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Adopt inherited descriptors; failure means this process is not an owner.
pub fn claim_worker(lock_descriptor: RawFd, ticket_descriptor: RawFd) -> Option<WorkerGuard> {
    capability::claim(lock_descriptor, ticket_descriptor)
}

pub fn publish_sync(cache_path: &Path, summary: &Value, now: f64, key: &str) -> Result<bool> {
    if !now.is_finite() || now < 0.0 {
        bail!("routing cache timestamp must be finite and non-negative");
    }
    let mut lock = private_lock_file(&lock_path(cache_path))?;
    if !lock_file(&lock, true)? {
        return Ok(false);
    }
    let baseline = util::cache_head(cache_path);
    let generation = baseline
        .as_ref()
        .map_or(0, |(generation, _)| *generation)
        .checked_add(1)
        .context("routing cache generation overflow")?;
    write_metadata(
        &mut lock,
        key,
        generation - 1,
        baseline.as_ref().map(|(_, cache_key)| cache_key.as_str()),
        &[1; 16],
    )?;
    if !lock_path_matches(&lock, &lock_path(cache_path))? {
        return Ok(false);
    }
    util::write_routing_cache(cache_path, summary, now, key, generation)?;
    Ok(true)
}

pub fn publish_worker(
    guard: &WorkerGuard,
    cache_path: &Path,
    summary: &Value,
    now: f64,
    key: &str,
) -> Result<bool> {
    if !now.is_finite() || now < 0.0 {
        bail!("routing cache timestamp must be finite and non-negative");
    }
    if !guard.owns_configuration(key)
        || util::cache_head(cache_path)
            != guard
                .baseline_cache_key
                .as_ref()
                .map(|key| (guard.baseline_generation(), key.clone()))
        || !lock_path_matches(&guard.lock, &lock_path(cache_path))?
    {
        return Ok(false);
    }
    let generation = guard
        .baseline_generation()
        .checked_add(1)
        .context("routing cache generation overflow")?;
    util::write_routing_cache(cache_path, summary, now, key, generation)?;
    Ok(true)
}

fn lock_path_matches(lock: &File, path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    let current = match private_lock_file(path) {
        Ok(file) => file,
        Err(_) => return Ok(false),
    };
    let owned = lock.metadata()?;
    let named = current.metadata()?;
    Ok(owned.dev() == named.dev() && owned.ino() == named.ino())
}

#[cfg(test)]
#[path = "refresh/tests.rs"]
mod tests;
