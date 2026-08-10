use std::{
    net::{SocketAddr, TcpStream},
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
const HANDOVER_TIMEOUT: Duration = Duration::from_millis(50);
const HANDOVER_POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Deserialize)]
struct RebindResponse {
    listen: String,
}

pub(super) fn handover_supported(health: &Health) -> bool {
    health.listener_handover && health.pid.is_some_and(|pid| pid != 0)
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
    if !handover_supported(health) {
        return Ok(None);
    }
    let Some(old_pid) = health.pid else {
        return Ok(None);
    };
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
    let started = daemon_start::start_adapter_with_retained(&warm, &retained_path)
        .context("warm-start current-build listener before canonical cutover")?;
    if let Err(error) = health::wait_until_ready(client, &warm).await {
        terminate_started(started, &warm);
        return Err(error.context("wait for warm-start listener"));
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
    if request_bind_listen(client, &warm, config.options.listen)
        .await?
        .is_none()
    {
        restore_old_canonical(client, config, retained_listen).await;
        terminate_started(started, &warm);
        return Ok(None);
    }
    if let Err(error) = health::wait_until_ready(client, config).await {
        restore_old_canonical(client, config, retained_listen).await;
        terminate_started(started, &warm);
        return Err(error.context("wait for promoted canonical listener"));
    }
    let _ = live::publish_listen(config, config.options.listen, Some(started));
    let _ = live::publish_canonical_rebind(config, config.options.listen, started);
    eprintln!(
        "claudex: promoted build {} to {} (previous pid {old_pid} retained on {} for {} in-flight session(s); launch TUI kept)",
        env!("CLAUDEX_BUILD_ID"),
        config.base_url(),
        retained_listen,
        session_ids.len()
    );
    Ok(Some(config.base_url()))
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

async fn wait_until_canonical_released(config: &ServiceConfig) -> Result<()> {
    let deadline = Instant::now() + HANDOVER_TIMEOUT;
    loop {
        if TcpStream::connect_timeout(&config.options.listen, Duration::from_millis(50)).is_err() {
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
