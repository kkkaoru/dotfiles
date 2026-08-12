use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
#[cfg(test)]
use std::{future::Future, path::Path, pin::Pin};

use anyhow::{Context, Result};

use super::{
    ServiceConfig,
    daemon_start::{StartedDaemon, start_ephemeral_adapter},
    handover,
};

pub(super) async fn verify(client: &reqwest::Client, config: &ServiceConfig) -> Result<()> {
    super::program_identity::validate(&config.options.routes)?;
    let listen = isolated_listen(config.options.listen)?;
    let preflight = config.with_listen(listen);
    let started = StartedDaemon::new(
        start_ephemeral_adapter(&preflight).context("start isolated adapter preflight")?,
    );
    super::wait_until_ready(client, &preflight)
        .await
        .context("isolated adapter preflight failed")?;
    handover::release_stale_listener(client, &preflight, Some(started.pid()))
        .await
        .context("stop isolated adapter preflight")?;
    started.disarm();
    Ok(())
}

#[cfg(test)]
pub(super) async fn verify_with_hooks<Start, Wait, Release, Matches, Terminate>(
    client: &reqwest::Client,
    config: &ServiceConfig,
    start: Start,
    wait: Wait,
    release: Release,
    matches: Matches,
    terminate: Terminate,
) -> Result<()>
where
    Start: FnOnce(&ServiceConfig) -> Result<u32>,
    Wait: for<'a> Fn(
        &'a reqwest::Client,
        &'a ServiceConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>,
    Release: for<'a> Fn(
        &'a reqwest::Client,
        &'a ServiceConfig,
        u32,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>,
    Matches: Fn(u32, &Path) -> bool,
    Terminate: Fn(u32),
{
    super::program_identity::validate(&config.options.routes)?;
    let listen = isolated_listen(config.options.listen)?;
    let preflight = config.with_listen(listen);
    let pid = start(&preflight).context("start isolated adapter preflight")?;
    let started = StartedDaemon::with_terminate(pid, terminate);
    if let Err(error) = wait(client, &preflight).await {
        if !matches(pid, &preflight.executable) {
            started.disarm();
        }
        return Err(error.context("isolated adapter preflight failed"));
    }
    release(client, &preflight, pid).await?;
    started.disarm();
    Ok(())
}

#[cfg(test)]
pub(super) fn finish_preflight_shutdown(
    pid: u32,
    result: Result<()>,
    force_terminate: impl FnOnce(u32),
) -> Result<()> {
    if let Err(error) = result {
        force_terminate(pid);
        return Err(error);
    }
    Ok(())
}

pub(super) fn isolated_listen(configured: SocketAddr) -> Result<SocketAddr> {
    let ip = match configured.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    let listener = TcpListener::bind(SocketAddr::new(ip, 0))
        .context("reserve isolated adapter preflight listener")?;
    listener.local_addr().context("read preflight listener")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "preflight_tests.rs"]
mod tests;
