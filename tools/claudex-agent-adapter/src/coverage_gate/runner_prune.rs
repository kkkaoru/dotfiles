use std::{fs, path::Path, time::SystemTime};

use anyhow::{Context, Result};

use crate::cache_hygiene::{
    COVERAGE_TARGET_KEEP, COVERAGE_TARGET_RETENTION, live_coverage_pid, write_cachedir_tag,
};

use super::COVERAGE_TARGET_PREFIX;

pub(in crate::coverage_gate) fn prune_stale_coverage_artifacts(
    root: &Path,
    current: &Path,
    now: SystemTime,
) -> Result<()> {
    let mut failed = collect_failed_targets(root, current)?;
    failed.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let mut kept = 0;
    for (modified, path) in failed {
        if keep_failed_target(kept, modified, now) {
            let _ = shrink_failed_coverage_target(&path);
            kept += 1;
            continue;
        }
        fs::remove_dir_all(&path)
            .with_context(|| format!("remove stale coverage artifact {}", path.display()))?;
    }
    Ok(())
}

pub(in crate::coverage_gate) fn shrink_failed_coverage_target(target: &Path) -> Result<()> {
    if !target.is_dir() {
        return Ok(());
    }
    let _ = write_cachedir_tag(target);
    for entry in fs::read_dir(target)
        .with_context(|| format!("read failed coverage target {}", target.display()))?
    {
        let path = entry?.path();
        if keep_diagnosis_file(&path) {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(&path).with_context(|| format!("shrink {}", path.display()))?;
        } else {
            fs::remove_file(&path).with_context(|| format!("shrink {}", path.display()))?;
        }
    }
    Ok(())
}

fn keep_failed_target(kept: usize, modified: SystemTime, now: SystemTime) -> bool {
    kept < COVERAGE_TARGET_KEEP
        && now
            .duration_since(modified)
            .is_ok_and(|age| age < COVERAGE_TARGET_RETENTION)
}

fn keep_diagnosis_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name == crate::cache_hygiene::CACHEDIR_TAG_NAME
        || name == "branch-coverage.json"
        || path
            .extension()
            .is_some_and(|ext| ext == "profraw" || ext == "profdata" || ext == "json")
}

fn collect_failed_targets(
    root: &Path,
    current: &Path,
) -> Result<Vec<(SystemTime, std::path::PathBuf)>> {
    let target_root = root.join("target");
    let entries = match fs::read_dir(&target_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", target_root.display())),
    };
    let mut failed = Vec::new();
    for entry in entries.flatten() {
        if let Some(record) = failed_target(&entry, current)? {
            failed.push(record);
        }
    }
    Ok(failed)
}

fn failed_target(
    entry: &fs::DirEntry,
    current: &Path,
) -> Result<Option<(SystemTime, std::path::PathBuf)>> {
    let path = entry.path();
    if path == current || !is_coverage_target(entry) || live_coverage_target(entry) {
        return Ok(None);
    }
    let modified = entry
        .metadata()
        .with_context(|| format!("inspect coverage artifact {}", path.display()))?
        .modified()
        .with_context(|| format!("read modification time for {}", path.display()))?;
    Ok(Some((modified, path)))
}

fn is_coverage_target(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(COVERAGE_TARGET_PREFIX))
}

fn live_coverage_target(entry: &fs::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .and_then(live_coverage_pid)
        .is_some()
}
