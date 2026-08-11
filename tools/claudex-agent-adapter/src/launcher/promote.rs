use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use super::{
    ServiceConfig, daemon_process, daemon_start, fallback,
    health::{self, Health},
    live,
};

mod rebind;
use rebind::{
    request_bind_listen, request_ephemeral_rebind, restore_old_canonical,
    wait_until_canonical_released,
};
use rebind::listen_is_free;

#[cfg(not(test))]
pub(super) const HANDOVER_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
pub(super) const HANDOVER_TIMEOUT: Duration = Duration::from_secs(2);
// llvm-cov parallel load delays dummy warm-start HTTP; keep the gate from
// treating a slow Python listener as a live-update failure.
#[cfg(not(test))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, coverage_nightly))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(45);
#[cfg(all(test, not(coverage_nightly)))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const HANDOVER_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Deserialize)]
pub(super) struct RebindResponse {
    pub(super) listen: String,
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
    // Quiet between turns still needs the previous generation retained so
    // sticky proxy / prompt-cache can resume warm SubAgents after cutover.
    // Match release_idle_retained: honor sticky idle grace on active sessions.
    if health.has_active_work() || health.within_sticky_idle_grace() {
        health.active_claude_session_ids.clone()
    } else {
        Vec::new()
    }
}

pub(super) fn warm_agent_ages(health: &Health) -> std::collections::BTreeMap<String, u64> {
    let mut ages = std::collections::BTreeMap::new();
    for id in &health.active_subagent_agent_ids {
        if !id.is_empty() {
            ages.insert(id.clone(), 0);
        }
    }
    for (id, age) in &health.recent_subagent_agent_ids {
        if id.is_empty() {
            continue;
        }
        // Prefer in-flight age 0 over a stale published age for the same id.
        ages.entry(id.clone()).or_insert(*age);
    }
    ages
}

#[cfg(test)]
pub(super) fn warm_agent_ids(health: &Health) -> Vec<String> {
    warm_agent_ages(health).into_keys().collect()
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
    let agent_ages = warm_agent_ages(health);
    let advertised = advertised_listen(config, health);
    let warm_listen = fallback::reserve_loopback_listen(config.options.listen)?;
    let warm = config.with_listen(warm_listen);
    let retained_path = live::write_retained_with_agents(
        config,
        advertised,
        old_pid,
        &health.build_id,
        session_ids.clone(),
        agent_ages.clone(),
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
    live::write_retained_with_agents(
        config,
        retained_listen,
        old_pid,
        &health.build_id,
        session_ids.clone(),
        agent_ages,
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
        // Cutover wrote retained.json before we knew the generation was idle.
        // Drop the empty snapshot so sticky middleware does not keep probing a
        // released ephemeral listen after reboot / zero-session promotes.
        if let Some((path, _)) = live::load_retained(config) {
            let _ = live::clear_retained(&path);
        }
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

/// Live-update may keep the previous daemon on an ephemeral port for sticky
/// sessions. Once that generation is idle (or already gone), release it so
/// orphan ACP children cannot accumulate across installs.
pub(super) async fn release_idle_retained(client: &reqwest::Client, config: &ServiceConfig) {
    let Some((path, generation)) = live::load_retained(config) else {
        return;
    };
    let retained = config.with_listen(generation.listen);
    let live_pid = health::fetch_health(client, config)
        .await
        .and_then(|health| health.pid);
    if live_pid == Some(generation.pid) {
        return;
    }
    // Empty sticky maps still need a health probe: forgetting the last owner
    // while retained is busy elsewhere must not kill/lose the daemon. Idle or
    // unreachable empty snapshots are released so zero-session cutovers do not
    // leave a dead listen forever.
    match health::fetch_health(client, &retained).await {
        Some(health) if health.pid == Some(generation.pid) && health.has_active_work() => {}
        Some(health)
            if health.pid == Some(generation.pid) && health.within_sticky_idle_grace() => {}
        Some(health) if health.pid == Some(generation.pid) => {
            release_previous(config, generation.pid);
            let _ = live::clear_retained(&path);
            eprintln!(
                "claudex: released {} retained adapter pid {} on {}",
                if generation.session_ids.is_empty() {
                    "empty"
                } else {
                    "idle"
                },
                generation.pid,
                generation.listen
            );
        }
        _ => {
            release_previous(config, generation.pid);
            let _ = live::clear_retained(&path);
        }
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
        // Spawn-path / preflight crashes exit before /health is up. Do not burn
        // the full warm-start budget polling a dead pid.
        if expected_pid.is_some_and(|pid| !daemon_process::is_alive(pid)) {
            return false;
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "promote_tests.rs"]
mod tests;
