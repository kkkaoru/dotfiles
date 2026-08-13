use std::{
    ffi::CString,
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub(crate) fn available_bytes(path: &Path) -> Result<u64> {
    let probe = existing_ancestor(path);
    let c_path = CString::new(probe.as_os_str().as_bytes())
        .with_context(|| format!("statvfs path {}", probe.display()))?;
    let mut vfs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `probe` is a NUL-terminated filesystem path; `vfs` is written on success.
    let code = unsafe { libc::statvfs(c_path.as_ptr(), vfs.as_mut_ptr()) };
    if code != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("statvfs {}", probe.display()));
    }
    // SAFETY: `statvfs` succeeded and initialized `vfs`.
    let vfs = unsafe { vfs.assume_init() };
    Ok((vfs.f_frsize as u64).saturating_mul(vfs.f_bavail as u64))
}

pub(crate) fn require_disk_free(path: &Path, minimum: u64) -> Result<()> {
    let available = available_bytes(path)?;
    if available >= minimum {
        return Ok(());
    }
    bail!(
        "only {available} bytes free at {} (need {minimum})",
        existing_ancestor(path).display()
    )
}

fn existing_ancestor(path: &Path) -> PathBuf {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) if parent != probe => probe = parent.to_path_buf(),
            _ => break,
        }
    }
    probe
}
