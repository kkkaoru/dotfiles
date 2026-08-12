//! Resolution and validation for executables used by the refresh worker.

use anyhow::{Context, Result, bail};
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

const TRUSTED_SEARCH_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];

/// Resolve a configured executable without consulting the caller's `PATH`.
pub fn executable(program: &str) -> Result<PathBuf> {
    let configured = Path::new(program);
    let candidate = if configured.is_absolute() {
        configured.to_path_buf()
    } else if configured.components().count() == 1 {
        TRUSTED_SEARCH_DIRS
            .iter()
            .map(|directory| Path::new(directory).join(configured))
            .find(|path| path.is_file())
            .with_context(|| format!("trusted executable not found: {program}"))?
    } else {
        bail!("configured executable must be absolute or a bare program name");
    };
    validate_canonical_executable(&candidate)
}

/// The OAuth helper is selected at compile time, never from cwd or runtime env.
pub fn data_file(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = canonical_owned_path(path, label)?;
    if !fs::symlink_metadata(&canonical)?.file_type().is_file() {
        bail!("{label} is not a regular file");
    }
    Ok(canonical)
}

pub fn data_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = canonical_owned_path(path, label)?;
    if !fs::symlink_metadata(&canonical)?.file_type().is_dir() {
        bail!("{label} is not a directory");
    }
    Ok(canonical)
}

fn canonical_owned_path(path: &Path, label: &str) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{label} path is not absolute: {}", path.display());
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("canonicalize {label} {}", path.display()))?;
    validate_chain(&canonical)?;
    Ok(canonical)
}

fn validate_canonical_executable(path: &Path) -> Result<PathBuf> {
    let canonical = canonical_owned_path(path, "trusted executable")?;
    let metadata = fs::symlink_metadata(&canonical)?;
    if !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        bail!("trusted executable is not a regular executable file");
    }
    Ok(canonical)
}

fn validate_chain(path: &Path) -> Result<()> {
    let effective_user = unsafe { libc::geteuid() };
    let mut current = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::Normal(part) => current.push(part),
            _ => bail!("trusted executable contains an unsafe path component"),
        }
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("inspect trusted path component {}", current.display()))?;
        if metadata.file_type().is_symlink() {
            bail!("trusted executable path contains a symlink");
        }
        if metadata.uid() != 0 && metadata.uid() != effective_user {
            bail!("trusted executable path has an untrusted owner");
        }
        let writable = metadata.permissions().mode() & 0o022;
        if writable & 0o002 != 0 || (writable & 0o020 != 0 && metadata.uid() != 0) {
            bail!("trusted path has an unsafe writable ancestor");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn rejects_relative_paths_and_accepts_an_owned_absolute_executable() {
        assert!(executable("nested/tool").is_err());
        let root = tempfile::tempdir().unwrap();
        let tool = root.path().join("tool");
        fs::File::create(&tool)
            .unwrap()
            .write_all(b"#!/bin/sh\n")
            .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            executable(tool.to_str().unwrap()).unwrap(),
            tool.canonicalize().unwrap()
        );
    }

    #[test]
    fn canonicalizes_a_configured_executable_symlink_to_a_trusted_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::write(&target, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            executable(link.to_str().unwrap()).unwrap(),
            target.canonicalize().unwrap()
        );
    }
}
