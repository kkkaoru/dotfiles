use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Deserialize;

use super::{
    START_INITIAL_POLL_DELAY, START_MAX_POLL_DELAY, START_TIMEOUT, ServiceConfig,
    daemon_start::RecoveryProcess, route_descriptions, search_worker_route_descriptions,
    worker_route_descriptions,
};
use crate::ADAPTER_PROTOCOL_VERSION;

#[derive(Debug, Deserialize)]
pub(super) struct Health {
    pub(super) status: String,
    pub(super) pid: Option<u32>,
    pub(super) protocol_version: u64,
    #[serde(rename = "build_id")]
    pub(super) build_id: String,
    #[serde(default)]
    pub(super) model: String,
    #[serde(default)]
    pub(super) codex_config_fingerprint: String,
    #[serde(default)]
    pub(super) service_config_fingerprint: String,
    #[serde(default)]
    pub(super) backend_routes: Vec<String>,
    #[serde(default)]
    pub(super) worker_routes: Vec<String>,
    #[serde(default)]
    pub(super) search_worker_routes: Vec<String>,
    pub(super) subscription_max_processes: usize,
    pub(super) subscription_timeout_minutes: u64,
    #[serde(default)]
    pub(super) subagent_hard_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub(super) recovery_generation: Option<String>,
    /// Number of HTTP requests currently served by the adapter.
    /// Older adapters omit this field, so deserialization remains compatible.
    #[serde(default)]
    pub(super) active_http_requests: usize,
    /// Number of provider turns currently in flight.
    /// Older adapters omit this field, so deserialization remains compatible.
    #[serde(default)]
    pub(super) active_provider_turns: usize,
}

impl Health {
    pub(super) fn has_active_work(&self) -> bool {
        self.active_http_requests > 0 || self.active_provider_turns > 0
    }
}

impl ServiceConfig {
    pub(super) fn matches(&self, health: &Health) -> bool {
        // Protocol/config compatibility is separate from build freshness. The handover
        // state machine checks the build ID without interrupting accepted responses.
        health.status == "ok"
            && health.protocol_version == ADAPTER_PROTOCOL_VERSION
            && health.model == self.options.model
            && health.codex_config_fingerprint == self.codex_config_fingerprint
            && health.service_config_fingerprint == self.service_config_fingerprint
            && health.backend_routes == route_descriptions(&self.options.routes)
            && health.worker_routes == worker_route_descriptions(&self.options.model_catalog)
            && health.search_worker_routes
                == search_worker_route_descriptions(&self.options.model_catalog)
            && health.subscription_max_processes == self.options.subscription_max_processes
            && health.subscription_timeout_minutes == self.options.subscription_timeout_minutes
            && health.subagent_hard_timeout_seconds
                == self
                    .options
                    .subagent_hard_timeout_seconds
                    .map(std::num::NonZeroU64::get)
    }
}

pub(super) async fn fetch_health(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Option<Health> {
    client
        .get(format!("{}/health", config.base_url()))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()
}

pub(super) async fn authenticates(client: &reqwest::Client, config: &ServiceConfig) -> bool {
    client
        .get(format!("{}/v1/models", config.base_url()))
        .bearer_auth(&config.token)
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

pub(super) async fn wait_until_recovery_ready(
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

pub(super) async fn wait_until_ready(
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

pub(super) async fn wait_until_ready_with(
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
