use std::{
    env,
    path::{Path, PathBuf},
};
#[cfg(test)]
use std::{fs, time::SystemTime};

use anyhow::{Context, Result, bail};

use super::RECOVERY_MANIFEST_ENV;

#[path = "installed_adapter_paths.rs"]
mod paths;
#[cfg(test)]
pub(in crate::launcher) use paths::{is_newer, same_file};
use paths::{promote_local_file_into_canonical, relink_local_to_canonical};

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
    if current
        .components()
        .any(|part| part.as_os_str() == "recovery")
    {
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
