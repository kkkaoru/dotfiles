use std::time::Duration;

use anyhow::{Context, Result};

use super::{
    ServiceConfig, daemon_process, daemon_start, fallback, handover, handover::ServiceState,
    health::wait_until_ready, launcher_lock, macos_notify, pending_hot_swap, preflight, recovery,
};

#[cfg(not(test))]
const WAIT_IDLE_POLL: Duration = Duration::from_secs(1);
#[cfg(test)]
const WAIT_IDLE_POLL: Duration = Duration::from_millis(0);
/// Production waiters keep retrying Replace after a failed handover. Tests
/// fail immediately so dummy-start fixtures do not spin forever.
#[cfg(not(test))]
const WAIT_IDLE_REPLACE_RETRIES: Option<u32> = None;
#[cfg(test)]
const WAIT_IDLE_REPLACE_RETRIES: Option<u32> = Some(0);

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
    let state = handover::inspect_service(&client, config).await;
    apply_inspected_state(config, &client, mode, state).await
}

async fn wait_until_idle_then_replace(config: &ServiceConfig) -> Result<String> {
    let client = reqwest::Client::new();
    let mut replace_failures: u32 = 0;
    loop {
        match handover::inspect_service(&client, config).await {
            ServiceState::Reuse => {
                pending_hot_swap::clear_if_current(config);
                return Ok(config.base_url());
            }
            ServiceState::Defer { .. } => {
                tokio::time::sleep(WAIT_IDLE_POLL).await;
            }
            ServiceState::Start => {
                let _lock = launcher_lock::acquire(&config.lock_path)?;
                let state = handover::wait_for_hot_swap_idle(&client, config).await?;
                match state {
                    ServiceState::Defer { .. } => {}
                    ServiceState::Reuse => {
                        pending_hot_swap::clear_if_current(config);
                        return Ok(config.base_url());
                    }
                    state => {
                        let url =
                            apply_inspected_state(config, &client, Mode::HotSwap, state).await?;
                        pending_hot_swap::clear_if_current(config);
                        return Ok(url);
                    }
                }
            }
            ServiceState::Replace { .. } => {
                let outcome = {
                    let _lock = launcher_lock::acquire(&config.lock_path)?;
                    let state = handover::wait_for_hot_swap_idle(&client, config).await?;
                    match state {
                        ServiceState::Defer { .. } => None,
                        ServiceState::Reuse => {
                            pending_hot_swap::clear_if_current(config);
                            return Ok(config.base_url());
                        }
                        state => {
                            Some(apply_inspected_state(config, &client, Mode::HotSwap, state).await)
                        }
                    }
                };
                match outcome {
                    None => {}
                    Some(Ok(url)) => {
                        pending_hot_swap::clear_if_current(config);
                        return Ok(url);
                    }
                    Some(Err(error)) => {
                        replace_failures = replace_failures.saturating_add(1);
                        eprintln!(
                            "claudex: idle hot-swap replace failed ({error:#}); waiting to retry"
                        );
                        if !should_retry_idle_replace(replace_failures, WAIT_IDLE_REPLACE_RETRIES) {
                            return Err(error);
                        }
                        tokio::time::sleep(WAIT_IDLE_POLL).await;
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
    let replaced = listener_was_replaced(&state);
    let recovery_manifest = match state {
        ServiceState::Reuse => {
            pending_hot_swap::clear_if_current(config);
            let _ = super::live::publish_listen(config, config.options.listen, None);
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
            let recovery_generation =
                usable_recovery_generation(config, recovery_generation.as_deref())?;
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
    notify_swap_if_replaced(replaced, config);
    let _ = super::live::publish_listen(config, config.options.listen, Some(started_pid));
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
        Mode::HotSwap | Mode::Ensure => {
            eprintln!(
                "claudex: retaining active adapter pid {pid:?}; routing new sessions to a current-build listener ({active_http_requests} HTTP request(s), {active_provider_turns} provider turn(s), {active_subagents} SubAgent(s); live launch sessions kept; idle hot-swap waiter pid {} for build {})",
                outcome.pid(),
                env!("CLAUDEX_BUILD_ID"),
            );
            if let Some(health) = super::health::fetch_health(client, config).await
                && let Some(url) = super::promote::try_canonical(client, config, &health).await?
            {
                pending_hot_swap::clear_if_current(config);
                notify_swap_if_replaced(true, config);
                return Ok(url);
            }
            let url = fallback::ensure_current_generation(client, config)
                .await
                .context("start current-build listener while stale adapter is active")?;
            let _ = super::live::publish_url(config, &url);
            if let Ok(live_listen) = super::live::parse_listen_url(&url) {
                macos_notify::live_ready(config, live_listen);
            }
            if let Ok(Some(live)) = super::live::read(config) {
                eprintln!(
                    "claudex: live generation {} on {}",
                    live.build_id, live.listen
                );
            }
            Ok(url)
        }
    }
}

pub(super) fn should_retry_idle_replace(failures: u32, limit: Option<u32>) -> bool {
    limit.is_none_or(|limit| failures <= limit)
}

pub(super) fn listener_was_replaced(state: &ServiceState) -> bool {
    matches!(state, ServiceState::Replace { .. })
}

pub(super) fn notify_swap_if_replaced(replaced: bool, config: &ServiceConfig) {
    if replaced {
        macos_notify::swap_complete(config);
    }
}

pub(super) fn usable_recovery_generation(
    config: &ServiceConfig,
    generation: Option<&str>,
) -> Result<Option<String>> {
    let Some(generation) = generation else {
        eprintln!(
            "claudex: current adapter predates recovery generations; performing a one-time preflight-only migration"
        );
        return Ok(None);
    };
    match daemon_start::validate_recovery(config, generation) {
        Ok(_) => Ok(Some(generation.to_owned())),
        Err(error) if recovery_snapshot_is_missing(&error) => {
            eprintln!(
                "claudex: recovery snapshot `{generation}` is unavailable ({error:#}); performing a preflight-only migration"
            );
            Ok(None)
        }
        Err(error) => {
            Err(error).context("validate current adapter recovery generation before handover")
        }
    }
}

fn recovery_snapshot_is_missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
    })
}
