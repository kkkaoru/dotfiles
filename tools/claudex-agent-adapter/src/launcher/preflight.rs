use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

use anyhow::{Context, Result};

use super::{ServiceConfig, daemon_process, daemon_start::start_ephemeral_adapter, handover};

pub(super) async fn verify(client: &reqwest::Client, config: &ServiceConfig) -> Result<()> {
    super::program_identity::validate(&config.options.routes)?;
    let listen = isolated_listen(config.options.listen)?;
    let preflight = config.with_listen(listen);
    let pid = start_ephemeral_adapter(&preflight).context("start isolated adapter preflight")?;
    if let Err(error) = super::wait_until_ready(client, &preflight).await {
        if daemon_process::matches(pid, &preflight.executable) {
            daemon_process::terminate(pid);
        }
        return Err(error.context("isolated adapter preflight failed"));
    }
    finish_preflight_shutdown(
        pid,
        handover::release_stale_listener(client, &preflight, Some(pid))
            .await
            .context("stop isolated adapter preflight"),
        daemon_process::terminate,
    )
}

fn finish_preflight_shutdown(
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

fn isolated_listen(configured: SocketAddr) -> Result<SocketAddr> {
    let ip = match configured.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    let listener = TcpListener::bind(SocketAddr::new(ip, 0))
        .context("reserve isolated adapter preflight listener")?;
    listener.local_addr().context("read preflight listener")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_matching_address_families_for_preflight() {
        assert!(
            isolated_listen("0.0.0.0:8318".parse().unwrap())
                .unwrap()
                .is_ipv4()
        );
        assert!(
            isolated_listen("[::]:8318".parse().unwrap())
                .unwrap()
                .is_ipv6()
        );
    }

    #[test]
    fn force_cleans_a_preflight_that_refuses_graceful_shutdown() {
        let terminated = std::sync::atomic::AtomicU32::new(0);
        let result = finish_preflight_shutdown(77, Err(anyhow::Error::msg("deadline")), |pid| {
            terminated.store(pid, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(result.is_err());
        assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 77);
    }
}
