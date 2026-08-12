use super::{DUPLICATE_FD_MINIMUM, WorkerGuard, lock_file, validate_lock};
use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};

pub(super) fn claim(lock_descriptor: RawFd, ticket_descriptor: RawFd) -> Option<WorkerGuard> {
    if lock_descriptor < DUPLICATE_FD_MINIMUM
        || ticket_descriptor < DUPLICATE_FD_MINIMUM
        || lock_descriptor == ticket_descriptor
    {
        return None;
    }
    let mut lock = duplicate_inherited(lock_descriptor)?;
    let mut ticket = duplicate_inherited(ticket_descriptor)?;
    unsafe {
        libc::close(lock_descriptor);
        libc::close(ticket_descriptor);
    }
    validate_lock(&lock).ok()?;
    set_close_on_exec(&lock).ok()?;
    set_close_on_exec(&ticket).ok()?;
    set_nonblocking(&ticket).ok()?;
    let mut secret = [0_u8; 16];
    ticket.read_exact(&mut secret).ok()?;
    let mut extra = [0_u8; 1];
    if secret.iter().all(|byte| *byte == 0) || ticket.read(&mut extra).ok()? != 0 {
        return None;
    }
    drop(ticket);
    if !lock_file(&lock, true).ok()? {
        return None;
    }
    lock.seek(SeekFrom::Start(0)).ok()?;
    let metadata: Value = serde_json::from_reader(&lock).ok()?;
    if metadata.get("version")?.as_u64()? != 3 {
        return None;
    }
    let digest = hex::encode(Sha256::digest(secret));
    if metadata.get("ticket_digest")?.as_str()? != digest {
        return None;
    }
    Some(WorkerGuard {
        lock,
        configuration_key: metadata.get("configuration_key")?.as_str()?.to_owned(),
        baseline_generation: metadata.get("baseline_generation")?.as_u64()?,
        baseline_cache_key: metadata
            .get("baseline_cache_key")?
            .as_str()
            .map(str::to_owned),
    })
}

fn duplicate_inherited(descriptor: RawFd) -> Option<File> {
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, DUPLICATE_FD_MINIMUM) };
    (duplicate != -1).then(|| unsafe { File::from_raw_fd(duplicate) })
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

fn set_nonblocking(file: &File) -> Result<()> {
    let descriptor = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(std::io::Error::last_os_error()).context("set refresh ticket nonblocking");
    }
    Ok(())
}
