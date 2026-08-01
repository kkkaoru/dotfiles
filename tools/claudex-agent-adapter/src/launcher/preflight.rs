use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener},
    path::Path,
    pin::Pin,
};

use anyhow::{Context, Result};

use super::{ServiceConfig, daemon_process, daemon_start::start_ephemeral_adapter, handover};

pub(super) async fn verify(client: &reqwest::Client, config: &ServiceConfig) -> Result<()> {
    verify_with_hooks(
        client,
        config,
        start_ephemeral_adapter,
        |client, config| Box::pin(super::wait_until_ready(client, config)),
        |client, config, pid| {
            Box::pin(async move {
                handover::release_stale_listener(client, config, Some(pid))
                    .await
                    .context("stop isolated adapter preflight")
            })
        },
        daemon_process::matches,
        daemon_process::terminate,
    )
    .await
}

async fn verify_with_hooks<Start, Wait, Release, Matches, Terminate>(
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
    if let Err(error) = wait(client, &preflight).await {
        if matches(pid, &preflight.executable) {
            terminate(pid);
        }
        return Err(error.context("isolated adapter preflight failed"));
    }
    finish_preflight_shutdown(pid, release(client, &preflight, pid).await, terminate)
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
mod tests {
    use super::*;

    fn test_config() -> ServiceConfig {
        ServiceConfig::new(super::super::AdapterOptions {
            routes: Vec::new(),
            model: "test-model".to_owned(),
            listen: "127.0.0.1:0".parse().unwrap(),
            subscription_max_processes: 1,
            subscription_timeout_minutes: 1,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        })
        .unwrap()
    }

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

    #[tokio::test]
    async fn verify_runs_the_full_successful_preflight_lifecycle() {
        let config = test_config();
        let result = verify_with_hooks(
            &reqwest::Client::new(),
            &config,
            |_| Ok(77),
            |_, _| Box::pin(async { Ok(()) }),
            |_, _, _| Box::pin(async { Ok(()) }),
            |_, _| true,
            |_| panic!("successful preflight does not force terminate"),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn verify_terminates_a_matching_preflight_when_readiness_fails() {
        let config = test_config();
        let terminated = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let observed = std::sync::Arc::clone(&terminated);
        let result = verify_with_hooks(
            &reqwest::Client::new(),
            &config,
            |_| Ok(78),
            |_, _| Box::pin(async { Err(anyhow::anyhow!("not ready")) }),
            |_, _, _| Box::pin(async { Ok(()) }),
            |_, _| true,
            move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 78);
    }

    #[tokio::test]
    async fn verify_keeps_an_unmatched_process_when_readiness_fails() {
        let config = test_config();
        let terminated = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let observed = std::sync::Arc::clone(&terminated);
        let result = verify_with_hooks(
            &reqwest::Client::new(),
            &config,
            |_| Ok(79),
            |_, _| Box::pin(async { Err(anyhow::anyhow!("not ready")) }),
            |_, _, _| Box::pin(async { Ok(()) }),
            |_, _| false,
            move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn verify_force_terminates_when_graceful_release_fails() {
        let config = test_config();
        let terminated = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let observed = std::sync::Arc::clone(&terminated);
        let result = verify_with_hooks(
            &reqwest::Client::new(),
            &config,
            |_| Ok(80),
            |_, _| Box::pin(async { Ok(()) }),
            |_, _, _| Box::pin(async { Err(anyhow::anyhow!("release failed")) }),
            |_, _| true,
            move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 80);
    }
}
