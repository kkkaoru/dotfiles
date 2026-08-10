use std::{
    env, fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, bail};

use super::RECOVERY_MANIFEST_ENV;

/// Override for tests and explicit pinning. When set, ensure/hot-swap/spawn use
/// this path instead of the unified cargo-bin install.
pub(crate) const ADAPTER_EXECUTABLE_ENV: &str = "CLAUDEX_ADAPTER_EXECUTABLE";

/// Child notify processes set this so they run dedupe in-process instead of
/// re-execing the install path again.
pub(crate) const NOTIFY_IN_PROCESS_ENV: &str = "CLAUDEX_NOTIFY_IN_PROCESS";

const EXECUTABLE_NAME: &str = "claudex-agent-adapter";

/// Canonical install location: `$CARGO_HOME/bin/claudex-agent-adapter`
/// (defaults to `~/.cargo/bin/...`). `~/.local/bin` must be a symlink here.
pub(crate) fn canonical_install_path() -> Option<PathBuf> {
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    Some(cargo_home.join("bin").join(EXECUTABLE_NAME))
}

pub(crate) fn local_link_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin").join(EXECUTABLE_NAME))
}

/// Resolve which binary ensure/hot-swap/daemon spawn should exec.
pub(crate) fn resolve_service_executable(current: PathBuf) -> PathBuf {
    if let Some(path) = env::var_os(ADAPTER_EXECUTABLE_ENV) {
        return PathBuf::from(path);
    }
    if keeps_current_image(&current) {
        return current;
    }
    match unify_install_paths() {
        Ok(path) => path,
        Err(_) => current,
    }
}

/// On-disk binary that owns macOS notify dedupe.
pub(crate) fn notify_delegate_executable() -> Option<PathBuf> {
    if env::var_os(NOTIFY_IN_PROCESS_ENV).is_some() {
        return None;
    }
    if let Some(path) = env::var_os(ADAPTER_EXECUTABLE_ENV) {
        let path = PathBuf::from(path);
        return is_executable(&path).then_some(path);
    }
    let path = unify_install_paths().ok()?;
    is_executable(&path).then_some(path)
}

fn keeps_current_image(current: &Path) -> bool {
    if env::var_os(RECOVERY_MANIFEST_ENV).is_some() {
        return true;
    }
    if current.components().any(|part| part.as_os_str() == "recovery") {
        return true;
    }
    current
        .file_name()
        .is_none_or(|name| name != EXECUTABLE_NAME)
}

/// Copy a fresher real `~/.local/bin` binary into cargo-bin when needed, then
/// make `~/.local/bin` a symlink to that canonical path.
pub(crate) fn unify_install_paths() -> Result<PathBuf> {
    let canonical = canonical_install_path().context("CARGO_HOME or HOME is required")?;
    let local = local_link_path().context("HOME is required")?;
    promote_local_file_into_canonical(&local, &canonical)?;
    if !is_executable(&canonical) {
        bail!(
            "canonical adapter is missing or not executable: {}",
            canonical.display()
        );
    }
    relink_local_to_canonical(&local, &canonical)?;
    Ok(canonical)
}

fn promote_local_file_into_canonical(local: &Path, canonical: &Path) -> Result<()> {
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

fn relink_local_to_canonical(local: &Path, canonical: &Path) -> Result<()> {
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
        std::os::unix::fs::symlink(canonical, local).with_context(|| {
            format!("symlink {} -> {}", local.display(), canonical.display())
        })?;
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

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => left == right,
    }
}

fn is_newer(left: &Path, right: &Path) -> bool {
    modified(left)
        .and_then(|left_time| modified(right).map(|right_time| left_time > right_time))
        .unwrap_or(true)
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

pub(crate) fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "installed_adapter_tests.rs"]
mod tests;
