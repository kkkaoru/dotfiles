use std::{
    net::TcpStream,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use super::{
    ServiceConfig, daemon_process, daemon_start,
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

pub(super) async fn try_canonical(
    client: &reqwest::Client,
    config: &ServiceConfig,
    health: &Health,
) -> Result<Option<String>> {
    if !handover_supported(health) {
        return Ok(None);
    }
    let Some(pid) = health.pid else {
        return Ok(None);
    };
    let Some(rebind) = request_ephemeral_rebind(client, config).await? else {
        return Ok(None);
    };
    let retained_listen = live::parse_listen_url(&format!("http://{}", rebind.listen))?;
    wait_until_canonical_released(config).await?;
    let retained_path = live::write_retained(
        config,
        retained_listen,
        pid,
        &health.build_id,
        health.active_claude_session_ids.clone(),
    )?;
    let started = daemon_start::start_adapter_with_retained(config, &retained_path)
        .context("start current-build listener on the canonical port")?;
    if let Err(error) = health::wait_until_ready(client, config).await {
        if daemon_process::matches(started, &config.executable) {
            daemon_process::terminate(started);
        }
        return Err(error.context("wait for promoted canonical listener"));
    }
    live::publish_listen(config, config.options.listen, Some(started))?;
    eprintln!(
        "claudex: promoted build {} to {} (previous pid {pid} listen {} retained on {} for {} session(s))",
        env!("CLAUDEX_BUILD_ID"),
        config.base_url(),
        health.listen.as_deref().unwrap_or("unknown"),
        retained_listen,
        health.active_claude_session_ids.len()
    );
    Ok(Some(config.base_url()))
}

async fn request_ephemeral_rebind(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<Option<RebindResponse>> {
    let response = match client
        .post(format!("{}/admin/rebind-listener", config.base_url()))
        .bearer_auth(&config.token)
        .json(&json!({ "ephemeral": true }))
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
