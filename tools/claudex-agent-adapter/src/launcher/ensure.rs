use std::time::Duration;

use anyhow::{Context, Result};

use super::{
    ServiceConfig, daemon_process, daemon_start, fallback, handover, handover::ServiceState,
    health::wait_until_ready, launcher_lock, pending_hot_swap, preflight, recovery,
};

const WAIT_IDLE_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Ensure,
    HotSwap,
    WaitIdle,
}

pub(super) async fn run(config: &ServiceConfig, mode: Mode) -> Result<String> {
    if mode == Mode::WaitIdle {
        return wait_until_idle_then_replace(config).await;
    }
    let _lock = launcher_lock::acquire(&config.lock_path)?;
    let client = reqwest::Client::new();
    let state = if mode == Mode::HotSwap {
        handover::wait_for_hot_swap_idle(&client, config).await?
    } else {
        handover::inspect_service(&client, config).await
    };
    apply_inspected_state(config, &client, mode, state).await
}

async fn wait_until_idle_then_replace(config: &ServiceConfig) -> Result<String> {
    let client = reqwest::Client::new();
    loop {
        match handover::inspect_service(&client, config).await {
            ServiceState::Reuse => {
                pending_hot_swap::clear_if_current(config);
                return Ok(config.base_url());
            }
            ServiceState::Defer { .. } => {
                tokio::time::sleep(WAIT_IDLE_POLL).await;
            }
            ServiceState::Replace { .. } | ServiceState::Start => {
                let _lock = launcher_lock::acquire(&config.lock_path)?;
                let state = handover::wait_for_hot_swap_idle(&client, config).await?;
                match &state {
                    ServiceState::Defer { .. } => {}
                    ServiceState::Reuse => {
                        pending_hot_swap::clear_if_current(config);
                        return Ok(config.base_url());
                    }
                    _ => {
                        let url =
                            apply_inspected_state(config, &client, Mode::HotSwap, state).await?;
                        pending_hot_swap::clear_if_current(config);
                        return Ok(url);
                    }
                }
            }
        }
    }
}

async fn apply_inspected_state(
    config: &ServiceConfig,
    client: &reqwest::Client,
    mode: Mode,
    state: ServiceState,
) -> Result<String> {
    let recovery_manifest = match state {
        ServiceState::Reuse => {
            pending_hot_swap::clear_if_current(config);
            return Ok(config.base_url());
        }
        ServiceState::Defer {
            pid,
            active_http_requests,
            active_provider_turns,
            active_subagents,
        } => {
            return defer_busy_listener(
                config,
                client,
                mode,
                pid,
                active_http_requests,
                active_provider_turns,
                active_subagents,
            )
            .await;
        }
        ServiceState::Replace {
            pid,
            recovery_generation,
        } => {
            if let Some(generation) = recovery_generation.as_deref() {
                daemon_start::validate_recovery(config, generation)
                    .context("validate current adapter recovery generation before handover")?;
            } else {
                eprintln!(
                    "claudex: current adapter predates recovery generations; performing a one-time preflight-only migration"
                );
            }
            let attached =
                super::session_process::any_launch_is_active(config.options.listen.port());
            eprintln!(
                "claudex: replacing adapter pid {pid:?} on {} with build {}{}{}",
                config.base_url(),
                env!("CLAUDEX_BUILD_ID"),
                if mode == Mode::HotSwap {
                    " (hot-swap)"
                } else {
                    ""
                },
                if attached {
                    "; launch TUI kept on this port"
                } else {
                    ""
                }
            );
            preflight::verify(client, config).await?;
            handover::release_stale_listener(client, config, pid).await?;
            recovery_generation
        }
        ServiceState::Start => None,
    };
    let started_pid = match daemon_start::start_adapter(config) {
        Ok(pid) => pid,
        Err(error) => {
            return recovery::after_update_failure(
                client,
                config,
                recovery_manifest.as_deref(),
                error.context("start new adapter generation"),
            )
            .await;
        }
    };
    if let Err(error) = wait_until_ready(client, config).await {
        if daemon_process::matches(started_pid, &config.executable) {
            daemon_process::terminate(started_pid);
        }
        return recovery::after_update_failure(client, config, recovery_manifest.as_deref(), error)
            .await;
    }
    pending_hot_swap::clear_if_current(config);
    Ok(config.base_url())
}

async fn defer_busy_listener(
    config: &ServiceConfig,
    client: &reqwest::Client,
    mode: Mode,
    pid: Option<u32>,
    active_http_requests: usize,
    active_provider_turns: usize,
    active_subagents: usize,
) -> Result<String> {
    let outcome = pending_hot_swap::arm(config)?;
    match mode {
        Mode::WaitIdle => unreachable!("wait-idle polls Defer without arming"),
        Mode::HotSwap => {
            eprintln!(
                "claudex: adapter pid {pid:?} still has active work ({active_http_requests} HTTP request(s), {active_provider_turns} provider turn(s), {active_subagents} SubAgent(s)); armed idle hot-swap waiter pid {} for build {}",
                outcome.pid(),
                env!("CLAUDEX_BUILD_ID"),
            );
            Ok(config.base_url())
        }
        Mode::Ensure => {
            eprintln!(
                "claudex: retaining active adapter pid {pid:?}; routing this new session to a current-build listener ({active_http_requests} HTTP request(s), {active_provider_turns} provider turn(s), {active_subagents} SubAgent(s); live launch sessions kept; idle hot-swap waiter pid {} for build {})",
                outcome.pid(),
                env!("CLAUDEX_BUILD_ID"),
            );
            fallback::ensure_current_generation(client, config)
                .await
                .context("start current-build listener while stale adapter is active")
        }
    }
}
