use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cache_hygiene::process_is_alive;

use super::adapter_log_path;

const ARCHIVE_KEEP: usize = 5;
const ARCHIVE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const ARCHIVE_MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct LiveHint {
    listen: std::net::SocketAddr,
    #[serde(default)]
    pid: Option<u32>,
}

pub(crate) fn prune_adapter_logs(cache: &Path) -> Result<usize> {
    let protected = live_log_paths(cache);
    let entries = match fs::read_dir(cache) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).with_context(|| format!("read {}", cache.display())),
    };
    let mut archived = Vec::new();
    for entry in entries.flatten() {
        collect_archive(&entry.path(), &protected, &mut archived)?;
    }
    prune_archived_logs(archived)
}

fn collect_archive(
    path: &Path,
    protected: &[PathBuf],
    archived: &mut Vec<ArchivedLog>,
) -> Result<()> {
    if protected.iter().any(|live| live == path) || is_canonical_adapter_log(path) {
        return Ok(());
    }
    if let Some(record) = archived_log(path)? {
        archived.push(record);
    }
    Ok(())
}

struct ArchivedLog {
    path: PathBuf,
    stem: String,
    modified: SystemTime,
    bytes: u64,
}

fn archived_log(path: &Path) -> Result<Option<ArchivedLog>> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    if !name.starts_with("adapter.") || !name.contains(".pid") {
        return Ok(None);
    }
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !metadata.is_file() {
        return Ok(None);
    }
    Ok(Some(ArchivedLog {
        stem: archive_stem(name).to_owned(),
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        bytes: metadata.len(),
        path: path.to_path_buf(),
    }))
}

fn archive_stem(name: &str) -> &str {
    name.split(".pid")
        .next()
        .and_then(|prefix| prefix.rsplit_once('.'))
        .map(|(stem, _nanos)| {
            stem.rsplit_once('.')
                .map(|(stem, _secs)| stem)
                .unwrap_or(stem)
        })
        .unwrap_or(name)
}

fn prune_archived_logs(mut archived: Vec<ArchivedLog>) -> Result<usize> {
    archived.sort_by(|left, right| {
        left.stem
            .cmp(&right.stem)
            .then(right.modified.cmp(&left.modified))
    });
    let now = SystemTime::now();
    let mut kept = 0;
    let mut current_stem = String::new();
    let mut removed = 0;
    for log in archived {
        if log.stem != current_stem {
            current_stem.clone_from(&log.stem);
            kept = 0;
        }
        if should_delete_archive(&log, kept, now) {
            fs::remove_file(&log.path)
                .with_context(|| format!("remove archived adapter log {}", log.path.display()))?;
            removed += 1;
            continue;
        }
        kept += 1;
    }
    Ok(removed)
}

fn should_delete_archive(log: &ArchivedLog, kept: usize, now: SystemTime) -> bool {
    kept >= ARCHIVE_KEEP
        || log.bytes > ARCHIVE_MAX_BYTES
        || now
            .duration_since(log.modified)
            .is_ok_and(|age| age >= ARCHIVE_MAX_AGE)
}

fn is_canonical_adapter_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("adapter.") && name.ends_with(".log") && !name.contains(".pid")
}

fn live_log_paths(cache: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cache) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| live_log_from_state(cache, &entry.path()))
        .collect()
}

fn live_log_from_state(cache: &Path, path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if !name.starts_with("live.") || !name.ends_with(".json") {
        return None;
    }
    let hint: LiveHint = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let pid = i32::try_from(hint.pid?).ok()?;
    if pid <= 0 || !process_is_alive(pid) {
        return None;
    }
    Some(adapter_log_path(cache, &hint.listen))
}
