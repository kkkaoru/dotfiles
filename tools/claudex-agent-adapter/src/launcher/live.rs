use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
#[cfg(test)]
use anyhow::bail;
use serde::{Deserialize, Serialize};

use super::ServiceConfig;

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
std::thread_local! {
    static RETAINED_WRITE_FAILURE_AFTER: Cell<Option<u32>> = const { Cell::new(None) };
}

pub(crate) const RETAINED_STATE_ENV: &str = "CLAUDEX_RETAINED_STATE";
pub(crate) use crate::listen_handover::SERVICE_LISTEN_ENV;

mod io;
use io::{cache_dir, read_live, retained_path, state_path, write_json, write_live};
pub(crate) use io::{clear_retained, forget_retained_session, load_retained, read_retained};

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
    /// SubAgent agentIds in-flight / warm at promote time. Kept for older
    /// readers; prefer `agent_ages` when present.
    #[serde(default)]
    pub agent_ids: Vec<String>,
    /// Warm SubAgent agentIds → seconds since last observation at promote.
    /// Empty on legacy snapshots; sticky then seeds `agent_ids` at `now`.
    #[serde(default)]
    pub agent_ages: std::collections::BTreeMap<String, u64>,
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

#[cfg(test)]
pub(super) fn write_retained(
    config: &ServiceConfig,
    listen: SocketAddr,
    pid: u32,
    build_id: &str,
    session_ids: Vec<String>,
) -> Result<PathBuf> {
    write_retained_with_agents(
        config,
        listen,
        pid,
        build_id,
        session_ids,
        std::collections::BTreeMap::new(),
    )
}

pub(super) fn write_retained_with_agents(
    config: &ServiceConfig,
    listen: SocketAddr,
    pid: u32,
    build_id: &str,
    session_ids: Vec<String>,
    agent_ages: std::collections::BTreeMap<String, u64>,
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
    let agent_ids: Vec<String> = agent_ages.keys().cloned().collect();
    write_json(
        &path,
        &RetainedGeneration {
            listen,
            pid,
            build_id: build_id.to_owned(),
            session_ids,
            agent_ids,
            agent_ages,
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


#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "live_tests.rs"]
mod tests;
