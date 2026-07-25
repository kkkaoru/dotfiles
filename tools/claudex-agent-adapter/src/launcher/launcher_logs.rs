use std::{fs, io::Write, net::SocketAddr, path::PathBuf, path::Path, process};

use anyhow::{Context, Result};

const EPOCH_FLOOR: &str = "system clock before UNIX_EPOCH";

pub(crate) fn archive_previous_log(log_path: &Path) -> Result<()> {
    if !log_path.exists() {
        return Ok(());
    }
    let metadata = fs::metadata(log_path).context("read previous adapter log metadata")?;
    if !metadata.is_file() {
        return Ok(());
    }
    let archived = archived_log_path(log_path)?;
    fs::rename(log_path, archived).context("archive previous adapter log")?;
    Ok(())
}

fn archived_log_path(log_path: &Path) -> Result<std::path::PathBuf> {
    let stem = log_path
        .file_stem()
        .and_then(|name| name.to_str())
        .context("adapter log file has no stem")?;
    let extension = log_path.extension().and_then(|extension| extension.to_str());
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

pub(crate) fn adapter_log_path(cache: &Path, listen: &SocketAddr) -> PathBuf {
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
        return cache.join("adapter.unknown-listen.log");
    }
    cache.join(format!("adapter.{token}.log"))
}
