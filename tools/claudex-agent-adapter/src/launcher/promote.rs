use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use super::{
    ServiceConfig, daemon_process,
    health::{self, Health},
    live,
};
use serde::Deserialize;

mod rebind;
mod warm;
#[allow(unused_imports)]
use rebind::{
    listen_is_free, request_bind_listen, request_ephemeral_rebind, restore_old_canonical,
    wait_until_canonical_released,
};
#[cfg(test)]
pub(in crate::launcher) use warm::warm_agent_ids;
pub(in crate::launcher) use warm::{retained_session_ids, warm_agent_ages};

#[cfg(not(test))]
pub(super) const HANDOVER_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, not(coverage_nightly)))]
pub(super) const HANDOVER_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(all(test, coverage_nightly))]
pub(super) const HANDOVER_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(not(test))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, not(coverage_nightly)))]
const WARM_START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, coverage_nightly))]
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

mod canonical;
pub(crate) use canonical::try_canonical;
#[allow(unused_imports)]
use canonical::{publish_promoted, release_previous};

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
        Some(health) if health.pid == Some(generation.pid) && health.within_sticky_idle_grace() => {
        }
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

pub(super) async fn canonical_serves_current_build(
    client: &reqwest::Client,
    config: &ServiceConfig,
    expected_pid: Option<u32>,
) -> bool {
    health::fetch_health(client, config)
        .await
        .is_some_and(|health| current_build_ready(&health, expected_pid))
}

pub(super) async fn wait_until_current_build(
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

pub(super) fn advertised_listen(config: &ServiceConfig, health: &Health) -> SocketAddr {
    health
        .listen
        .as_deref()
        .and_then(|listen| live::parse_listen_url(&format!("http://{listen}")).ok())
        .unwrap_or(config.options.listen)
}

#[cfg(test)]
pub(super) fn terminate_started(pid: u32, config: &ServiceConfig) {
    if daemon_process::matches(pid, &config.executable) {
        daemon_process::terminate(pid);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "promote_tests.rs"]
mod tests;
