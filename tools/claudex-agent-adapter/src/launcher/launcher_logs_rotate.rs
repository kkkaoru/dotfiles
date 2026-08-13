use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};

use super::{ARCHIVE_MAX_BYTES, prune_adapter_logs};

pub(crate) const LOG_ROTATION_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) async fn watch_canonical_log_size<F>(log_path: PathBuf, interval: Duration, rotate: F)
where
    F: Fn(&Path) -> Result<bool> + Send + 'static,
{
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let _ = rotate(&log_path);
    }
}

pub(crate) fn rotate_live_daemon_log(log_path: &Path) -> Result<bool> {
    if !stdio_owns_path(log_path) {
        return Ok(false);
    }
    let Some(file) = rotate_canonical_log(log_path)? else {
        return Ok(false);
    };
    redirect_stdio(&file)?;
    if let Some(parent) = log_path.parent() {
        let _ = prune_adapter_logs(parent);
    }
    Ok(true)
}

pub(crate) fn rotate_canonical_log(log_path: &Path) -> Result<Option<File>> {
    if !needs_rotation(log_path)? {
        return Ok(None);
    }
    let archived = super::archive_previous_log(log_path)?;
    match OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(log_path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) => {
            restore_canonical_log(log_path, archived);
            Err(error).context("open rotated adapter log")
        }
    }
}

fn restore_canonical_log(log_path: &Path, archived: Option<PathBuf>) {
    if let Some(archived) = archived {
        let _ = fs::rename(&archived, log_path);
    }
}

fn needs_rotation(log_path: &Path) -> Result<bool> {
    match fs::metadata(log_path) {
        Ok(metadata) if metadata.is_file() => Ok(metadata.len() > ARCHIVE_MAX_BYTES),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("stat {}", log_path.display())),
    }
}

fn stdio_owns_path(path: &Path) -> bool {
    let Ok(path_meta) = fs::metadata(path) else {
        return false;
    };
    fd_metadata(2).is_some_and(|stderr_meta| same_inode(&path_meta, &stderr_meta))
}

#[cfg(unix)]
fn fd_metadata(fd: i32) -> Option<fs::Metadata> {
    use std::os::unix::io::{FromRawFd, IntoRawFd};

    // SAFETY: the raw fd stays open; IntoRawFd prevents File::drop from closing it.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().ok();
    let _ = file.into_raw_fd();
    metadata
}

#[cfg(unix)]
fn same_inode(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
fn redirect_stdio(file: &File) -> Result<()> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    redirect_fd(fd, 1).context("redirect stdout to rotated adapter log")?;
    redirect_fd(fd, 2).context("redirect stderr to rotated adapter log")
}

#[cfg(unix)]
fn redirect_fd(from: i32, to: i32) -> io::Result<()> {
    if unsafe { libc::dup2(from, to) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn fd_metadata(_fd: i32) -> Option<fs::Metadata> {
    None
}

#[cfg(not(unix))]
fn same_inode(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

#[cfg(not(unix))]
fn redirect_stdio(_file: &File) -> Result<()> {
    Ok(())
}
