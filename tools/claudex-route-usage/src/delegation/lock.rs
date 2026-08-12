use super::MAX_STATE_BYTES;
use super::fs::{c_name, io_error, open_at, validate_private_file};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::time::{Duration, Instant};

const LOCK_WAIT: Duration = Duration::from_millis(100);

pub(super) struct ExclusiveFileLock(File);

fn open_or_create_lock(directory: &File, name: &str) -> std::io::Result<File> {
    let flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    for _ in 0..3 {
        match open_at(directory, name, flags, 0) {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let name_c = c_name(name)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name_c.as_ptr(),
                flags | libc::O_CREAT | libc::O_EXCL,
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
    Err(io_error(ErrorKind::WouldBlock, "delegation lock raced"))
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
    pub(super) fn acquire(directory: &File, name: &str) -> Result<Self> {
        let file = open_or_create_lock(directory, name)
            .with_context(|| format!("open delegation lock {name}"))?;
        validate_private_file(&file, MAX_STATE_BYTES)
            .with_context(|| format!("validate delegation lock {name}"))?;
        wait_for_exclusive_lock(&file).with_context(|| format!("lock delegation state {name}"))?;
        Ok(Self(file))
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
