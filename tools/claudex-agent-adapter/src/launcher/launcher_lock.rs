use std::{
    fs::{self, File, OpenOptions},
    os::fd::AsRawFd,
    path::Path,
};

use anyhow::{Context, Result};

pub(super) struct LauncherLock {
    _file: File,
}

pub(super) fn acquire(path: &Path) -> Result<LauncherLock> {
    let parent = path.parent().context("launcher lock has no parent")?;
    fs::create_dir_all(parent).context("create launcher lock directory")?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .context("open launcher lock")?;
    lock(&file)?;
    Ok(LauncherLock { _file: file })
}

/// Try to acquire an exclusive launcher lock without waiting for the owner.
///
/// The daemon lock is intentionally blocking, but a resume-session lock must
/// fail fast: a second Claude Code process must not attach to the same
/// transcript and make either interactive UI appear to disappear.
pub(super) fn try_acquire(path: &Path) -> Result<Option<LauncherLock>> {
    let parent = path.parent().context("launcher lock has no parent")?;
    fs::create_dir_all(parent).context("create launcher lock directory")?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .context("open launcher lock")?;
    if try_lock(&file)? {
        Ok(Some(LauncherLock { _file: file }))
    } else {
        Ok(None)
    }
}

fn lock(file: &File) -> Result<()> {
    lock_file_descriptor(file.as_raw_fd())
}

fn try_lock(file: &File) -> Result<bool> {
    try_lock_file_descriptor(file.as_raw_fd())
}

pub(super) fn lock_file_descriptor(file_descriptor: std::os::fd::RawFd) -> Result<()> {
    loop {
        let result = unsafe { libc::flock(file_descriptor, libc::LOCK_EX) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("lock launcher state");
        }
    }
}

pub(super) fn try_lock_file_descriptor(file_descriptor: std::os::fd::RawFd) -> Result<bool> {
    loop {
        let result = unsafe { libc::flock(file_descriptor, libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        let raw = error.raw_os_error();
        if raw == Some(libc::EAGAIN) || raw == Some(libc::EWOULDBLOCK) {
            return Ok(false);
        }
        return Err(error).context("try lock launcher state");
    }
}
