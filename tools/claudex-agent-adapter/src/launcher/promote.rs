use std::{
    net::{SocketAddr, TcpListener as StdTcpListener},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use super::{
    ServiceConfig, daemon_process, daemon_start, fallback,
    health::{self, Health},
    live,
};

#[cfg(not(test))]
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(2);
// llvm-cov parallel load delays dummy warm-start HTTP; keep the gate from
// treating a slow Python listener as a live-update failure.
#[cfg(not(test))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, coverage_nightly))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(45);
#[cfg(all(test, not(coverage_nightly)))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(2);
const HANDOVER_POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Deserialize)]
struct RebindResponse {
    listen: String,
}

pub(super) fn handover_supported(health: &Health) -> bool {
    health.listener_handover && health.pid.is_some_and(|pid| pid != 0)
}

/// Warm-start cutover is only for a new adapter binary on the same service
/// config. Fingerprint changes still need isolated preflight and recovery.
pub(super) fn live_update_eligible(health: &Health, config: &ServiceConfig) -> bool {
    handover_supported(health)
        && health.codex_config_fingerprint == config.codex_config_fingerprint
        && health.service_config_fingerprint == config.service_config_fingerprint
}

pub(super) fn retained_session_ids(health: &Health) -> Vec<String> {
    if !health.busy_claude_session_ids.is_empty() {
        return health.busy_claude_session_ids.clone();
    }
    if health.has_active_work() {
        health.active_claude_session_ids.clone()
    } else {
        Vec::new()
    }
}

pub(super) async fn try_canonical(
    client: &reqwest::Client,
    config: &ServiceConfig,
    health: &Health,
) -> Result<Option<String>> {
    let Some(old_pid) = health.pid.filter(|&pid| pid != 0) else {
        return Ok(None);
    };
    if !health.listener_handover {
        return Ok(None);
    }
    super::pending_hot_swap::disarm(config);
    let session_ids = retained_session_ids(health);
    let advertised = advertised_listen(config, health);
    let warm_listen = fallback::reserve_loopback_listen(config.options.listen)?;
    let warm = config.with_listen(warm_listen);
    let retained_path = live::write_retained(
        config,
        advertised,
        old_pid,
        &health.build_id,
        session_ids.clone(),
    )?;
    let started = daemon_start::start_adapter_with_retained(&warm, &retained_path, config)
        .context("warm-start current-build listener before canonical cutover")?;
    if !wait_until_current_build(client, &warm, Some(started)).await {
        terminate_started(started, &warm);
        bail!(
            "wait for warm-start listener; see {}",
            warm.log_path.display()
        );
    }
    let Some(rebind) = request_ephemeral_rebind(client, config).await? else {
        terminate_started(started, &warm);
        return Ok(None);
    };
    let retained_listen = live::parse_listen_url(&format!("http://{}", rebind.listen))?;
    live::write_retained(
        config,
        retained_listen,
        old_pid,
        &health.build_id,
        session_ids.clone(),
    )?;
    wait_until_canonical_released(config).await?;
    let probe = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .context("build live-update probe client")?;
    let _ = request_bind_listen(&probe, &warm, config.options.listen).await;
    if !wait_until_current_build(&probe, config, None).await {
        restore_old_canonical(&probe, config, retained_listen).await;
    }
    if canonical_serves_current_build(&probe, config, None).await {
        return Ok(Some(publish_promoted(
            config,
            started,
            old_pid,
            retained_listen,
            session_ids.len(),
        )));
    }
    terminate_started(started, &warm);
    if listen_is_free(config.options.listen) {
        let pid = daemon_start::start_adapter(config)
            .context("start current-build listener after empty canonical port")?;
        if wait_until_current_build(&probe, config, None).await {
            return Ok(Some(publish_promoted(
                config,
                pid,
                old_pid,
                retained_listen,
                session_ids.len(),
            )));
        }
    }
    bail!(
        "wait for promoted canonical listener; see {}",
        config.log_path.display()
    );
}

fn publish_promoted(
    config: &ServiceConfig,
    pid: u32,
    old_pid: u32,
    retained_listen: SocketAddr,
    retained_sessions: usize,
) -> String {
    let _ = live::publish_listen(config, config.options.listen, Some(pid));
    let _ = live::publish_canonical_rebind(config, config.options.listen, pid);
    if retained_sessions == 0 {
        release_previous(config, old_pid);
        eprintln!(
            "claudex: promoted build {} to {} (previous pid {old_pid} released; launch TUI kept)",
            env!("CLAUDEX_BUILD_ID"),
            config.base_url(),
        );
    } else {
        eprintln!(
            "claudex: promoted build {} to {} (previous pid {old_pid} retained on {} for {retained_sessions} in-flight session(s); launch TUI kept)",
            env!("CLAUDEX_BUILD_ID"),
            config.base_url(),
            retained_listen
        );
    }
    config.base_url()
}

fn release_previous(config: &ServiceConfig, old_pid: u32) {
    if daemon_process::matches(old_pid, &config.executable) {
        daemon_process::terminate(old_pid);
    }
}

pub(super) fn current_build_ready(health: &Health, expected_pid: Option<u32>) -> bool {
    health.status == "ok"
        && health.build_id == env!("CLAUDEX_BUILD_ID")
        && expected_pid.is_none_or(|pid| health.pid.is_none_or(|actual| actual == pid))
}

async fn canonical_serves_current_build(
    client: &reqwest::Client,
    config: &ServiceConfig,
    expected_pid: Option<u32>,
) -> bool {
    health::fetch_health(client, config)
        .await
        .is_some_and(|health| current_build_ready(&health, expected_pid))
}

async fn wait_until_current_build(
    client: &reqwest::Client,
    config: &ServiceConfig,
    expected_pid: Option<u32>,
) -> bool {
    let deadline = Instant::now() + WARM_START_TIMEOUT;
    loop {
        if canonical_serves_current_build(client, config, expected_pid).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(HANDOVER_POLL).await;
    }
}

fn advertised_listen(config: &ServiceConfig, health: &Health) -> SocketAddr {
    health
        .listen
        .as_deref()
        .and_then(|listen| live::parse_listen_url(&format!("http://{listen}")).ok())
        .unwrap_or(config.options.listen)
}

fn terminate_started(pid: u32, config: &ServiceConfig) {
    if daemon_process::matches(pid, &config.executable) {
        daemon_process::terminate(pid);
    }
}

async fn restore_old_canonical(
    client: &reqwest::Client,
    config: &ServiceConfig,
    retained_listen: SocketAddr,
) {
    let _ = request_bind_listen(
        client,
        &config.with_listen(retained_listen),
        config.options.listen,
    )
    .await;
}

async fn request_ephemeral_rebind(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<Option<RebindResponse>> {
    request_rebind(client, config, json!({ "ephemeral": true })).await
}

async fn request_bind_listen(
    client: &reqwest::Client,
    target: &ServiceConfig,
    listen: SocketAddr,
) -> Result<Option<RebindResponse>> {
    request_rebind(client, target, json!({ "listen": listen.to_string() })).await
}

async fn request_rebind(
    client: &reqwest::Client,
    target: &ServiceConfig,
    body: serde_json::Value,
) -> Result<Option<RebindResponse>> {
    let response = match client
        .post(format!("{}/admin/rebind-listener", target.base_url()))
        .bearer_auth(&target.token)
        .json(&body)
        .timeout(HANDOVER_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    Ok(response.json().await.ok())
}

fn listen_is_free(listen: SocketAddr) -> bool {
    StdTcpListener::bind(listen).is_ok()
}

async fn wait_until_canonical_released(config: &ServiceConfig) -> Result<()> {
    let deadline = Instant::now() + HANDOVER_TIMEOUT;
    loop {
        if listen_is_free(config.options.listen) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "canonical listener {} did not release after handover",
                config.options.listen
            );
        }
        tokio::time::sleep(HANDOVER_POLL).await;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "promote_tests.rs"]
mod tests;
