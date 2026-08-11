use std::{
    fs,
    path::Path,
    time::SystemTime,
};

use anyhow::{Context, Result};

use super::is_executable;

pub(super) fn promote_local_file_into_canonical(local: &Path, canonical: &Path) -> Result<()> {
    if !local.exists() || local.is_symlink() || !is_executable(local) {
        return Ok(());
    }
    if let Some(parent) = canonical.parent() {
        fs::create_dir_all(parent).context("create cargo bin directory")?;
    }
    if !is_executable(canonical) || is_newer(local, canonical) {
        fs::copy(local, canonical).with_context(|| {
            format!(
                "copy fresher adapter {} -> {}",
                local.display(),
                canonical.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(canonical)?.permissions();
            permissions.set_mode(permissions.mode() | 0o111);
            fs::set_permissions(canonical, permissions)?;
        }
    }
    Ok(())
}

pub(super) fn relink_local_to_canonical(local: &Path, canonical: &Path) -> Result<()> {
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent).context("create ~/.local/bin")?;
    }
    if local.is_symlink()
        && fs::read_link(local)
            .ok()
            .is_some_and(|target| paths_match(&target, canonical, local.parent()))
    {
        return Ok(());
    }
    if local.exists() || local.is_symlink() {
        fs::remove_file(local).with_context(|| format!("replace {}", local.display()))?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(canonical, local)
            .with_context(|| format!("symlink {} -> {}", local.display(), canonical.display()))?;
    }
    #[cfg(not(unix))]
    {
        fs::copy(canonical, local).context("copy adapter into ~/.local/bin")?;
    }
    Ok(())
}

fn paths_match(link_target: &Path, canonical: &Path, local_parent: Option<&Path>) -> bool {
    if link_target == canonical {
        return true;
    }
    let resolved = local_parent
        .map(|parent| parent.join(link_target))
        .unwrap_or_else(|| link_target.to_path_buf());
    same_file(&resolved, canonical)
}

pub(in crate::launcher) fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => left == right,
    }
}

pub(in crate::launcher) fn is_newer(left: &Path, right: &Path) -> bool {
    modified(left)
        .and_then(|left_time| modified(right).map(|right_time| left_time > right_time))
        .unwrap_or(true)
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
