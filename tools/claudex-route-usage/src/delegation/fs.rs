use super::STATE_DIRECTORY;
use anyhow::{Context, Result, bail};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path};

pub(super) fn io_error(kind: ErrorKind, message: &'static str) -> std::io::Error {
    std::io::Error::new(kind, message)
}

pub(super) fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

pub(super) fn c_name(name: &str) -> std::io::Result<CString> {
    CString::new(name).map_err(|_| io_error(ErrorKind::InvalidInput, "NUL in directory entry name"))
}

pub(super) fn open_at(
    directory: &File,
    name: &str,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> std::io::Result<File> {
    let name = c_name(name)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY);
    options.open(path)
}

fn open_directory_path(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(io_error(
            ErrorKind::InvalidInput,
            "cache directory must be absolute",
        ));
    }
    let path = if path.starts_with("/var") && Path::new("/var").is_symlink() {
        Path::new("/private").join(path.strip_prefix("/").unwrap_or(path))
    } else {
        path.to_path_buf()
    };
    let root = open_directory(Path::new("/"))?;
    validate_directory(&root, false)?;
    let mut current = root;
    for component in path.components() {
        let Some(segment) = next_path_segment(component)? else {
            continue;
        };
        let segment = segment.to_str().ok_or_else(|| {
            io_error(
                ErrorKind::InvalidInput,
                "cache directory contains non-UTF-8 path components",
            )
        })?;
        current = open_child_directory(&current, segment, false)?;
    }
    Ok(current)
}

fn validate_directory(directory: &File, private: bool) -> std::io::Result<()> {
    let metadata = directory.metadata()?;
    let mode = metadata.mode();
    let trusted_owner = metadata.uid() == current_uid() || (!private && metadata.uid() == 0);
    if !metadata.is_dir()
        || !trusted_owner
        || mode & 0o022 != 0
        || (private && mode & 0o777 != 0o700)
    {
        return Err(io_error(
            ErrorKind::PermissionDenied,
            "directory is not an owner-controlled private directory",
        ));
    }
    Ok(())
}

fn chmod_directory(directory: &File, mode: libc::mode_t) -> std::io::Result<()> {
    if unsafe { libc::fchmod(directory.as_raw_fd(), mode) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn next_path_segment(component: Component<'_>) -> std::io::Result<Option<&std::ffi::OsStr>> {
    match component {
        Component::RootDir => Ok(None),
        Component::Normal(segment) => Ok(Some(segment)),
        _ => Err(io_error(
            ErrorKind::InvalidInput,
            "cache directory contains unsafe path components",
        )),
    }
}

fn mkdir_private(parent: &File, name: &str) -> std::io::Result<()> {
    let name_c = c_name(name)?;
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), 0o700) };
    if result == 0 {
        return Ok(());
    }
    let create_error = std::io::Error::last_os_error();
    if create_error.kind() == ErrorKind::AlreadyExists {
        Ok(())
    } else {
        Err(create_error)
    }
}

fn open_child_directory(parent: &File, name: &str, private: bool) -> std::io::Result<File> {
    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY;
    let directory = match open_at(parent, name, flags, 0) {
        Ok(directory) => directory,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            mkdir_private(parent, name)?;
            open_at(parent, name, flags, 0)?
        }
        Err(error) => return Err(error),
    };
    if private {
        chmod_directory(&directory, 0o700)?;
    }
    validate_directory(&directory, private)?;
    Ok(directory)
}

pub(super) fn open_cache_layout(home: &Path, cache_root: &Path) -> Result<(File, File)> {
    let default_root = home.join(".cache/claudex");
    let cache = if cache_root == default_root {
        let home_directory = open_directory_path(home).context("open HOME directory chain")?;
        validate_directory(&home_directory, false).context("validate HOME directory")?;
        let dot_cache = open_child_directory(&home_directory, ".cache", false)
            .context("open HOME .cache directory")?;
        open_child_directory(&dot_cache, "claudex", true).context("open Claudex cache directory")?
    } else {
        let cache = open_directory_path(cache_root).context("open CLAUDEX_CACHE_DIR")?;
        chmod_directory(&cache, 0o700).context("privatize CLAUDEX_CACHE_DIR")?;
        validate_directory(&cache, true).context("validate CLAUDEX_CACHE_DIR")?;
        cache
    };
    let state = open_child_directory(&cache, STATE_DIRECTORY, true)
        .context("open delegation state directory")?;
    Ok((cache, state))
}

pub(super) fn validate_private_file(file: &File, maximum_bytes: u64) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > maximum_bytes
    {
        bail!("file is not a safe owner-controlled regular file");
    }
    Ok(())
}
