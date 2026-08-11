use std::{net::SocketAddr, path::PathBuf};

#[cfg(test)]
use anyhow::bail;
use anyhow::{Context, Result};

#[cfg(test)]
use super::RETAINED_WRITE_FAILURE_AFTER;
use super::{RetainedGeneration, ServiceConfig, retained_path, write_json};

#[cfg(test)]
pub(in crate::launcher) fn write_retained(
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

pub(in crate::launcher) fn write_retained_with_agents(
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
pub(in crate::launcher) struct FailRetainedWriteAfter;

#[cfg(test)]
impl FailRetainedWriteAfter {
    pub(in crate::launcher) fn arm(successes: u32) -> Self {
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

pub(in crate::launcher) fn parse_listen_url(url: &str) -> Result<SocketAddr> {
    let trimmed = url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    without_scheme
        .parse()
        .with_context(|| format!("parse live listen URL `{url}`"))
}
