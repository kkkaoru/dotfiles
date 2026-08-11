use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};

pub(super) fn ensure_private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        reject_symlink_and_wrong_owner(path, "recovery directory")?;
        ensure!(path.is_dir(), "recovery path is not a directory");
    } else {
        fs::create_dir_all(path).context("create adapter recovery directory")?;
    }
    set_private_permissions(path, 0o700)?;
    validate_private_directory(path, "recovery directory")
}

pub(super) fn validate_private_directory(path: &Path, label: &str) -> Result<()> {
    reject_symlink_and_wrong_owner(path, label)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_dir(), "{label} is not a directory");
    validate_mode(&metadata, 0o700, label)
}

pub(super) fn validate_private_file(path: &Path, mode: u32, label: &str) -> Result<()> {
    reject_symlink_and_wrong_owner(path, label)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "{label} is not a regular file");
    validate_mode(&metadata, mode, label)
}

fn reject_symlink_and_wrong_owner(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{label} must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "{label} is not owned by the current user"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn validate_mode(metadata: &fs::Metadata, expected: u32, label: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        metadata.permissions().mode() & 0o777 == expected,
        "{label} permissions must be {expected:o}"
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_mode(_metadata: &fs::Metadata, _expected: u32, _label: &str) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_private_permissions(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("secure adapter recovery path {}", path.display()))
}

#[cfg(not(unix))]
pub(super) fn set_private_permissions(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

pub(super) fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
