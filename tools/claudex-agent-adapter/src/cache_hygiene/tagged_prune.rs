use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};

use super::{
    COVERAGE_TARGET_KEEP, COVERAGE_TARGET_RETENTION, TAGGED_TARGET_RETENTION, has_cachedir_tag,
    live_coverage_pid,
};

pub(crate) fn prune_tagged_dirs(root: &Path, now: SystemTime) -> Result<usize> {
    let tagged = find_tagged_dirs(root)?;
    let mut removed = 0;
    for path in tagged {
        if keep_tagged(&path, now)? {
            continue;
        }
        fs::remove_dir_all(&path)
            .with_context(|| format!("remove tagged cache {}", path.display()))?;
        removed += 1;
    }
    Ok(removed)
}

fn keep_tagged(path: &Path, now: SystemTime) -> Result<bool> {
    if contains_live_coverage(path) {
        return Ok(true);
    }
    if is_kept_coverage_target(path, now)? {
        return Ok(true);
    }
    if is_coverage_target_name(path) {
        return Ok(false);
    }
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .with_context(|| format!("inspect tagged cache {}", path.display()))?;
    Ok(now
        .duration_since(modified)
        .is_ok_and(|age| age < TAGGED_TARGET_RETENTION))
}

fn is_coverage_target_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("llvm-cov-"))
}

fn is_kept_coverage_target(path: &Path, now: SystemTime) -> Result<bool> {
    if !is_coverage_target_name(path) {
        return Ok(false);
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(live_coverage_pid)
        .is_some()
    {
        return Ok(true);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    Ok(rank_among_failed_coverage(parent, path, now)? < COVERAGE_TARGET_KEEP)
}

fn rank_among_failed_coverage(parent: &Path, path: &Path, now: SystemTime) -> Result<usize> {
    let mut siblings = failed_coverage_siblings(parent)?;
    siblings.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let rank = siblings
        .iter()
        .position(|(_, candidate)| candidate == path)
        .unwrap_or(usize::MAX);
    let Some((modified, _)) = siblings.get(rank) else {
        return Ok(usize::MAX);
    };
    if now
        .duration_since(*modified)
        .is_ok_and(|age| age >= COVERAGE_TARGET_RETENTION)
    {
        return Ok(usize::MAX);
    }
    Ok(rank)
}

fn failed_coverage_siblings(parent: &Path) -> Result<Vec<(SystemTime, PathBuf)>> {
    let mut siblings = Vec::new();
    for entry in fs::read_dir(parent).with_context(|| format!("read {}", parent.display()))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with("llvm-cov-") || live_coverage_pid(&name).is_some() {
            continue;
        }
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        siblings.push((modified, entry.path()));
    }
    Ok(siblings)
}

fn contains_live_coverage(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(live_coverage_pid)
        .is_some()
    {
        return true;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .and_then(live_coverage_pid)
            .is_some()
    })
}

fn find_tagged_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut tagged = Vec::new();
    while let Some(dir) = pending.pop() {
        collect_tagged(root, &dir, &mut pending, &mut tagged)?;
    }
    Ok(tagged)
}

fn collect_tagged(
    walk_root: &Path,
    dir: &Path,
    pending: &mut Vec<PathBuf>,
    tagged: &mut Vec<PathBuf>,
) -> Result<()> {
    if has_cachedir_tag(dir) {
        if dir != walk_root {
            tagged.push(dir.to_path_buf());
        }
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("walk {}", dir.display())),
    };
    for entry in entries.flatten() {
        push_walk_dir(entry, pending);
    }
    Ok(())
}

fn push_walk_dir(entry: fs::DirEntry, pending: &mut Vec<PathBuf>) {
    let Ok(metadata) = entry.metadata() else {
        return;
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        pending.push(entry.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_helpers_report_missing_metadata_and_file_walk_errors() {
        let root = tempfile::tempdir().expect("private helper fixture");
        let missing = root.path().join("missing-target");
        let metadata_error = keep_tagged(&missing, SystemTime::now())
            .expect_err("missing tagged cache metadata must fail");
        assert!(metadata_error.to_string().contains("inspect tagged cache"));

        let file = root.path().join("not-a-directory");
        fs::write(&file, b"file").expect("helper walk file");
        assert!(failed_coverage_siblings(&file).is_err());
        assert!(!contains_live_coverage(&file));
    }

    #[test]
    fn rank_reports_an_unlisted_coverage_target_as_unranked() {
        let root = tempfile::tempdir().expect("rank fixture");
        let candidate = root.path().join("llvm-cov-not-listed");
        assert_eq!(
            rank_among_failed_coverage(root.path(), &candidate, SystemTime::now())
                .expect("empty sibling rank"),
            usize::MAX
        );
    }
}
