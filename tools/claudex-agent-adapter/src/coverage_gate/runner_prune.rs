use std::{
    fs,
    path::Path,
    time::SystemTime,
};

use anyhow::{Context, Result};

use super::{COVERAGE_ARTIFACT_RETENTION, COVERAGE_TARGET_PREFIX};

pub(in crate::coverage_gate) fn prune_stale_coverage_artifacts(
    root: &Path,
    current: &Path,
    now: SystemTime,
) -> Result<()> {
    let target_root = root.join("target");
    let entries = match fs::read_dir(&target_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("read {}", target_root.display())),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if stale_coverage_artifact(&entry, current, now)? {
            fs::remove_dir_all(&path)
                .with_context(|| format!("remove stale coverage artifact {}", path.display()))?;
        }
    }
    Ok(())
}

fn stale_coverage_artifact(entry: &fs::DirEntry, current: &Path, now: SystemTime) -> Result<bool> {
    let path = entry.path();
    if path == current || !is_coverage_target(entry) || live_coverage_process(entry) {
        return Ok(false);
    }
    let modified = entry
        .metadata()
        .with_context(|| format!("inspect coverage artifact {}", path.display()))?
        .modified()
        .with_context(|| format!("read modification time for {}", path.display()))?;
    Ok(now
        .duration_since(modified)
        .is_ok_and(|age| age >= COVERAGE_ARTIFACT_RETENTION))
}

fn is_coverage_target(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(COVERAGE_TARGET_PREFIX))
}

fn live_coverage_process(entry: &fs::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .and_then(|name| name.strip_prefix(COVERAGE_TARGET_PREFIX))
        .and_then(|process| process.parse::<i32>().ok())
        .is_some_and(process_is_alive)
}

fn process_is_alive(process: i32) -> bool {
    // SAFETY: signal zero performs no action; it only asks the OS whether the
    // process exists, which prevents pruning an active sibling coverage run.
    unsafe { libc::kill(process, 0) == 0 }
}
