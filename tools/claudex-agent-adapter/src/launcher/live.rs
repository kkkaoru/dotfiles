use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
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

#[path = "live_retained.rs"]
mod retained;
#[cfg(test)]
pub(super) use retained::{FailRetainedWriteAfter, write_retained};
pub(super) use retained::{parse_listen_url, write_retained_with_agents};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "live_tests.rs"]
mod tests;
