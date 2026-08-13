use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    net::SocketAddr,
    path::Path,
    path::PathBuf,
    process,
};

use anyhow::{Context, Result};

const EPOCH_FLOOR: &str = "system clock before UNIX_EPOCH";

pub(crate) fn archive_previous_log(log_path: &Path) -> Result<Option<PathBuf>> {
    if !log_path.exists() {
        return Ok(None);
    }
    let metadata = fs::metadata(log_path).context("read previous adapter log metadata")?;
    if !metadata.is_file() {
        return Ok(None);
    }
    let archived = archived_log_path(log_path)?;
    fs::rename(log_path, &archived).context("archive previous adapter log")?;
    Ok(Some(archived))
}

fn archived_log_path(log_path: &Path) -> Result<std::path::PathBuf> {
    let stem = log_path
        .file_stem()
        .and_then(|name| name.to_str())
        .context("adapter log file has no stem")?;
    let extension = log_path
        .extension()
        .and_then(|extension| extension.to_str());
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context(EPOCH_FLOOR)?;
    let suffix = format!(
        "{}.{:09}.pid{}",
        timestamp.as_secs(),
        timestamp.subsec_nanos(),
        process::id()
    );
    let file_name = extension
        .map(|extension| format!("{stem}.{suffix}.{extension}"))
        .unwrap_or_else(|| format!("{stem}.{suffix}"));
    Ok(log_path.with_file_name(file_name))
}

pub(crate) fn write_adapter_log_header(
    log: &mut impl Write,
    model: &str,
    listen: &SocketAddr,
    token_len: usize,
) -> Result<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context(EPOCH_FLOOR)?;
    let started_utc = format!("{}.{}", timestamp.as_secs(), timestamp.subsec_nanos());
    writeln!(
        log,
        "=== claudex-agent-adapter daemon start === model={} listen={} build_id={} pid={} token_len={} started_at_utc={}",
        model,
        listen,
        env!("CLAUDEX_BUILD_ID"),
        process::id(),
        token_len,
        started_utc
    )?;
    log.flush()?;
    Ok(())
}

#[path = "launcher_logs_prune.rs"]
mod prune;
pub(crate) use prune::{ARCHIVE_MAX_BYTES, prune_adapter_logs};

#[path = "launcher_logs_rotate.rs"]
mod rotate;
#[cfg(test)]
pub(crate) use rotate::rotate_canonical_log;
pub(crate) use rotate::{LOG_ROTATION_INTERVAL, rotate_live_daemon_log, watch_canonical_log_size};

pub(crate) fn prune_spawn_caches(log_dir: &Path) {
    crate::cache_hygiene::prune_tagged_cache(&crate::cache_hygiene::tagged_prune_root(log_dir));
    let _ = prune_adapter_logs(log_dir);
}

pub(crate) fn adapter_log_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
    cache.join(format!("adapter.{}.log", listen_token(listen)))
}

pub(crate) fn adapter_lock_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
    cache.join(format!("adapter.port-{}.lock", listen.port()))
}

pub(crate) fn pending_hot_swap_state_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
    cache.join(format!("pending-hot-swap.{}.json", listen_token(listen)))
}

pub(crate) fn pending_hot_swap_log_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
    cache.join(format!("pending-hot-swap.{}.log", listen_token(listen)))
}

pub(crate) fn hot_swap_notify_path(cache: &Path, _listen: &SocketAddr) -> PathBuf {
    // One shared dedup file per cache so the same build_id cannot notify once
    // per listen port when cargo install / ensure races across adapters.
    cache.join("hot-swap-notify.json")
}

pub(crate) fn hot_swap_notify_lock_path(cache: &Path) -> PathBuf {
    cache.join("hot-swap-notify.lock")
}

pub(crate) fn live_state_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
    cache.join(format!("live.{}.json", listen.port()))
}

pub(crate) fn retained_state_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
    cache.join(format!("retained.{}.json", listen.port()))
}

pub(crate) fn session_lock_path(cache: &Path, session_id: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    cache.join(format!("session.{:016x}.lock", hasher.finish()))
}

fn listen_token(listen: &SocketAddr) -> String {
    let token: String = listen
        .to_string()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if token.is_empty() {
        return "unknown-listen".to_owned();
    }
    token
}
