use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, launcher_logs};

pub(crate) const RETAINED_STATE_ENV: &str = "CLAUDEX_RETAINED_STATE";

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
