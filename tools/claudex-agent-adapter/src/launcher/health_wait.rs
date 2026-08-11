use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::super::{
    START_INITIAL_POLL_DELAY, START_MAX_POLL_DELAY, START_TIMEOUT, ServiceConfig,
    daemon_start::RecoveryProcess,
};
use super::{authenticates, fetch_health};

pub(in crate::launcher) async fn wait_until_recovery_ready(
    client: &reqwest::Client,
    config: &ServiceConfig,
    recovery: &RecoveryProcess,
) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if fetch_health(client, config).await.is_some_and(|health| {
            health.status == "ok"
                && health.pid == Some(recovery.pid)
                && health.protocol_version == recovery.protocol_version
                && health.build_id == recovery.build_id
                && health.model == recovery.model
                && health.codex_config_fingerprint == recovery.codex_config_fingerprint
                && health.service_config_fingerprint == recovery.service_config_fingerprint
                && health.recovery_generation.as_deref() == Some(&recovery.generation)
        }) && authenticates(client, config).await
        {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("previous adapter generation failed to recover");
        }
        tokio::time::sleep(START_INITIAL_POLL_DELAY.min(remaining)).await;
    }
}

pub(in crate::launcher) async fn wait_until_ready(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<()> {
    wait_until_ready_with(
        client,
        config,
        START_TIMEOUT,
        START_INITIAL_POLL_DELAY,
        START_MAX_POLL_DELAY,
    )
    .await
}

pub(in crate::launcher) async fn wait_until_ready_with(
    client: &reqwest::Client,
    config: &ServiceConfig,
    timeout: Duration,
    initial_delay: Duration,
    max_delay: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut delay = initial_delay;
    loop {
        let ready = match fetch_health(client, config).await {
            Some(health)
                if config.matches(&health) && health.build_id == env!("CLAUDEX_BUILD_ID") =>
            {
                authenticates(client, config).await
            }
            _ => false,
        };
        if ready {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(delay.min(remaining)).await;
        delay = delay.saturating_mul(2).min(max_delay);
    }
    bail!(
        "agent adapter failed to start; see {}",
        config.log_path.display()
    )
}
