use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::daemon_process::{matches as process_matches, terminate};
use super::{ServiceConfig, authenticates, fetch_health};

const LISTENER_RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ServiceState {
    Reuse,
    Replace(Option<u32>),
    Start,
}

pub(super) async fn inspect_service(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> ServiceState {
    let Some(health) = fetch_health(client, config).await else {
        return ServiceState::Start;
    };
    if config.matches(&health)
        && health.build_id == env!("CLAUDEX_BUILD_ID")
        && authenticates(client, config).await
    {
        ServiceState::Reuse
    } else {
        ServiceState::Replace(health.pid)
    }
}

pub(super) async fn release_stale_listener(
    client: &reqwest::Client,
    config: &ServiceConfig,
    pid: Option<u32>,
) -> Result<()> {
    release_stale_listener_with(client, config, pid, process_matches, terminate).await
}

pub(super) async fn release_stale_listener_with(
    client: &reqwest::Client,
    config: &ServiceConfig,
    pid: Option<u32>,
    process_matches: impl Fn(u32, &std::path::Path) -> bool,
    terminate: impl Fn(u32),
) -> Result<()> {
    let Some(pid) = pid else {
        return Ok(());
    };
    if pid == std::process::id() || !process_matches(pid, &config.executable) {
        return Ok(());
    }
    eprintln!("claudex: draining active requests on stale adapter pid {pid} during handover");
    terminate(pid);
    wait_until_listener_released(client, config, pid).await?;
    Ok(())
}

async fn wait_until_listener_released(
    client: &reqwest::Client,
    config: &ServiceConfig,
    stale_pid: u32,
) -> Result<()> {
    wait_until_listener_released_by(
        client,
        config,
        stale_pid,
        Instant::now() + super::START_TIMEOUT,
    )
    .await
}

pub(super) async fn wait_until_listener_released_by(
    client: &reqwest::Client,
    config: &ServiceConfig,
    stale_pid: u32,
    deadline: Instant,
) -> Result<()> {
    loop {
        let stale_health_is_gone = fetch_health(client, config)
            .await
            .is_none_or(|health| health.pid != Some(stale_pid));
        if stale_health_is_gone
            && tokio::net::TcpListener::bind(config.options.listen)
                .await
                .is_ok()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("stale adapter pid {stale_pid} did not release its listener");
        }
        tokio::time::sleep(LISTENER_RELEASE_POLL_INTERVAL).await;
    }
}
