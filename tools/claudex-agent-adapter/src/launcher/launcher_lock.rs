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

fn lock(file: &File) -> Result<()> {
    lock_file_descriptor(file.as_raw_fd())
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
