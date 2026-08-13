use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

mod disk;
mod tag;
mod tagged_prune;

pub(crate) use disk::require_disk_free;
pub(crate) use tag::{has_cachedir_tag, write_cachedir_tag};
pub(crate) use tagged_prune::prune_tagged_dirs;

/// Instrumented coverage targets are multi-gigabyte; refuse to start when the
/// volume cannot hold one PID-scoped directory plus headroom.
pub(crate) const MIN_COVERAGE_FREE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Keep at most this many non-live failed `llvm-cov-*` directories per parent.
pub(crate) const COVERAGE_TARGET_KEEP: usize = 2;
pub(crate) const COVERAGE_TARGET_RETENTION: std::time::Duration =
    std::time::Duration::from_secs(2 * 60 * 60);
pub(crate) const TAGGED_TARGET_RETENTION: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);

pub(crate) const CACHEDIR_TAG_NAME: &str = "CACHEDIR.TAG";
pub(crate) const CACHEDIR_TAG_CONTENTS: &str = "\
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by claudex-agent-adapter.
# For information about cache directory tags, see:
#	https://bford.info/cachedir/
";

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct PruneSummary {
    pub tagged_dirs: usize,
    pub adapter_logs: usize,
}

pub(crate) fn default_prune_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is required")?;
    Ok(PathBuf::from(home).join(".cache"))
}

pub(crate) fn process_is_alive(process: i32) -> bool {
    // SAFETY: signal 0 performs no action; it only asks whether the process exists.
    unsafe { libc::kill(process, 0) == 0 }
}

pub(crate) fn live_coverage_pid(name: &str) -> Option<i32> {
    name.strip_prefix("llvm-cov-")
        .and_then(|process| process.parse().ok())
        .filter(|process| process_is_alive(*process))
}

pub(crate) fn format_prune_summary(summary: &PruneSummary) -> String {
    format!(
        "pruned {} tagged cache dirs, {} adapter logs",
        summary.tagged_dirs, summary.adapter_logs
    )
}

pub(crate) fn require_coverage_disk(path: &Path) -> Result<()> {
    require_disk_free(path, MIN_COVERAGE_FREE_BYTES).with_context(|| {
        format!(
            "need at least {} bytes free for an isolated coverage target",
            MIN_COVERAGE_FREE_BYTES
        )
    })
}

pub(crate) fn prepare_coverage_target(target: &Path) -> Result<()> {
    require_coverage_disk(target)?;
    std::fs::create_dir_all(target)
        .with_context(|| format!("create coverage target {}", target.display()))?;
    write_cachedir_tag(target)
}

pub(crate) fn ensure_prune_root(root: Option<PathBuf>) -> Result<PathBuf> {
    match root {
        Some(path) if path.as_os_str().is_empty() => bail!("prune-cache root must not be empty"),
        Some(path) => Ok(path),
        None => default_prune_root(),
    }
}

/// Walk `$HOME/.cache` when the adapter log lives there so spawn/coverage
/// prune the same tagged cargo trees as `prune-cache`. Test fixtures that
/// are not under the home cache stay scoped to themselves.
pub(crate) fn tagged_prune_root(log_dir: &Path) -> PathBuf {
    default_prune_root()
        .ok()
        .filter(|home_cache| log_dir.starts_with(home_cache))
        .unwrap_or_else(|| log_dir.to_path_buf())
}

pub(crate) fn prune_tagged_cache(root: &Path) {
    let _ = prune_tagged_dirs(root, std::time::SystemTime::now());
}

pub(crate) fn prune_default_tagged_cache() {
    let Ok(root) = default_prune_root() else {
        return;
    };
    prune_tagged_cache(&root);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "cache_hygiene_tests.rs"]
mod tests;
