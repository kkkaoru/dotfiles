use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, launcher_logs};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
std::thread_local! {
    static RETAINED_WRITE_FAILURE_AFTER: Cell<Option<u32>> = const { Cell::new(None) };
}

pub(crate) const RETAINED_STATE_ENV: &str = "CLAUDEX_RETAINED_STATE";
pub(crate) use crate::listen_handover::SERVICE_LISTEN_ENV;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(super) struct LiveState {
    pub listen: SocketAddr,
    pub build_id: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct RetainedGeneration {
    pub listen: SocketAddr,
    pub pid: u32,
    pub build_id: String,
    #[serde(default)]
    pub session_ids: Vec<String>,
}

pub(super) fn publish_listen(
    config: &ServiceConfig,
    listen: SocketAddr,
    pid: Option<u32>,
) -> Result<()> {
    write_live(
        &state_path(config)?,
        &LiveState {
            listen,
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            pid,
        },
    )
}

pub(super) fn publish_url(config: &ServiceConfig, url: &str) -> Result<()> {
    publish_listen(config, parse_listen_url(url)?, None)
}

pub(super) fn read(config: &ServiceConfig) -> Result<Option<LiveState>> {
    read_live(&state_path(config)?)
}

pub(crate) fn load_retained_from_env() -> Option<(PathBuf, RetainedGeneration)> {
    let path = PathBuf::from(std::env::var_os(RETAINED_STATE_ENV)?);
    let generation = read_retained(&path).ok().flatten()?;
    Some((path, generation))
}

pub(super) fn publish_canonical_rebind(
    config: &ServiceConfig,
    listen: SocketAddr,
    pid: u32,
) -> Result<()> {
    write_json(
        &crate::listen_handover::rebind_state_path(cache_dir(config)?, &config.options.listen),
        &crate::listen_handover::RebindState { listen, pid },
    )
}

pub(super) fn write_retained(
    config: &ServiceConfig,
    listen: SocketAddr,
    pid: u32,
    build_id: &str,
    session_ids: Vec<String>,
) -> Result<PathBuf> {
    #[cfg(test)]
    if RETAINED_WRITE_FAILURE_AFTER.with(|cell| match cell.get() {
        Some(0) => {
            cell.set(None);
            true
        }
        Some(remaining) => {
            cell.set(Some(remaining - 1));
            false
        }
        None => false,
    }) {
        bail!("injected retained state write failure");
    }
    let path = retained_path(config)?;
    write_json(
        &path,
        &RetainedGeneration {
            listen,
            pid,
            build_id: build_id.to_owned(),
            session_ids,
        },
    )?;
    Ok(path)
}

#[cfg(test)]
pub(super) struct FailRetainedWriteAfter;

#[cfg(test)]
impl FailRetainedWriteAfter {
    pub(super) fn arm(successes: u32) -> Self {
        RETAINED_WRITE_FAILURE_AFTER.with(|cell| cell.set(Some(successes)));
        Self
    }
}

#[cfg(test)]
impl Drop for FailRetainedWriteAfter {
    fn drop(&mut self) {
        RETAINED_WRITE_FAILURE_AFTER.with(|cell| cell.set(None));
    }
}

pub(super) fn parse_listen_url(url: &str) -> Result<SocketAddr> {
    let trimmed = url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    without_scheme
        .parse()
        .with_context(|| format!("parse live listen URL `{url}`"))
}

fn state_path(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(launcher_logs::live_state_path(
        cache_dir(config)?,
        &config.options.listen,
    ))
}

fn retained_path(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(launcher_logs::retained_state_path(
        cache_dir(config)?,
        &config.options.listen,
    ))
}

fn cache_dir(config: &ServiceConfig) -> Result<&Path> {
    config
        .log_path
        .parent()
        .context("adapter log has no parent")
}

fn read_live(path: &Path) -> Result<Option<LiveState>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: LiveState =
        serde_json::from_slice(&fs::read(path).context("read live adapter state")?)
            .context("decode live adapter state")?;
    if state.listen.port() == 0 || !state.listen.ip().is_loopback() || state.build_id.is_empty() {
        bail!("invalid live adapter state");
    }
    Ok(Some(state))
}

pub(crate) fn read_retained(path: &Path) -> Result<Option<RetainedGeneration>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: RetainedGeneration =
        serde_json::from_slice(&fs::read(path).context("read retained generation state")?)
            .context("decode retained generation state")?;
    if state.listen.port() == 0 || !state.listen.ip().is_loopback() || state.pid == 0 {
        bail!("invalid retained generation state");
    }
    Ok(Some(state))
}

/// Drop one sticky session from a retained snapshot.
///
/// Empty `session_ids` still keep listen/pid so `release_idle_retained` can find
/// a busy orphan after the last sticky owner forgets. Deleting the file here
/// leaked retained daemons that still had active work for other traffic.
pub(crate) fn forget_retained_session(path: &Path, session_id: &str) -> Result<()> {
    let Some(mut generation) = read_retained(path)? else {
        return Ok(());
    };
    let before = generation.session_ids.len();
    generation
        .session_ids
        .retain(|owned| owned != session_id);
    if generation.session_ids.len() == before {
        return Ok(());
    }
    write_json(path, &generation)
}

/// Remove the retained snapshot entirely (idle / dead generation recovery).
pub(crate) fn clear_retained(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).context("remove retained generation state")?;
    }
    Ok(())
}

pub(super) fn load_retained(config: &ServiceConfig) -> Option<(PathBuf, RetainedGeneration)> {
    let path = retained_path(config).ok()?;
    let generation = read_retained(&path).ok().flatten()?;
    Some((path, generation))
}

fn write_live(path: &Path, state: &LiveState) -> Result<()> {
    write_json(path, state)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create live adapter state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure live adapter state")?;
    }
    output
        .write_all(&serde_json::to_vec(value).context("encode live adapter state")?)
        .context("write live adapter state")?;
    output.sync_all().context("sync live adapter state")?;
    fs::rename(&temporary, path).context("publish live adapter state")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "live_tests.rs"]
mod tests;
