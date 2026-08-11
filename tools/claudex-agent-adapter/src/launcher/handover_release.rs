use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::{
    LISTENER_RELEASE_POLL_INTERVAL, ServiceConfig, fetch_health, force_terminate, process_matches,
    request_graceful_shutdown,
};

pub(in crate::launcher) async fn release_stale_listener(
    client: &reqwest::Client,
    config: &ServiceConfig,
    pid: Option<u32>,
) -> Result<()> {
    release_stale_listener_with(
        client,
        config,
        pid,
        process_matches,
        request_graceful_shutdown,
        force_terminate,
        Instant::now() + super::super::START_TIMEOUT,
    )
    .await
}

pub(in crate::launcher) async fn release_stale_listener_with(
    client: &reqwest::Client,
    config: &ServiceConfig,
    pid: Option<u32>,
    process_matches: impl Fn(u32, &std::path::Path) -> bool,
    request_graceful_shutdown: impl Fn(u32),
    force_terminate: impl Fn(u32),
    deadline: Instant,
) -> Result<()> {
    let Some(pid) = pid else {
        return Ok(());
    };
    if pid == std::process::id() || !process_matches(pid, &config.executable) {
        return Ok(());
    }
    eprintln!("claudex: draining active requests on stale adapter pid {pid} during handover");
    request_graceful_shutdown(pid);
    if wait_until_listener_released_by(client, config, pid, deadline)
        .await
        .is_ok()
    {
        return Ok(());
    }
    eprintln!("claudex: forcing terminate of stale adapter pid {pid} after drain timeout");
    force_terminate(pid);
    if let Err(error) = wait_until_listener_released_by(
        client,
        config,
        pid,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    {
        eprintln!(
            "claudex: handover aborted because stale adapter pid {pid} retained its listener"
        );
        return Err(error);
    }
    Ok(())
}

async fn wait_until_listener_released_by(
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
