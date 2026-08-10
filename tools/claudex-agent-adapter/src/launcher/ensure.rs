use anyhow::{Context, Result};

use super::{
    ServiceConfig, daemon_process, daemon_start, fallback, handover, handover::ServiceState,
    health::wait_until_ready, launcher_lock, macos_notify, pending_hot_swap, preflight, recovery,
};

#[path = "ensure_wait_idle.rs"]
mod wait_idle;

#[cfg(test)]
pub(super) use wait_idle::{WAIT_IDLE_POLL_INTERVAL, WaitIdleInspectPause};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Ensure,
    HotSwap,
    WaitIdle,
}

pub(super) async fn run(config: &ServiceConfig, mode: Mode) -> Result<String> {
    if mode == Mode::WaitIdle {
        return wait_idle::wait_until_idle_then_replace(config).await;
    }
    let _lock = launcher_lock::acquire(&config.lock_path)?;
    let client = reqwest::Client::new();
    let state = handover::inspect_service(&client, config).await;
    apply_inspected_state(config, &client, mode, state).await
}

pub(super) async fn apply_inspected_state(
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
            super::promote::release_idle_retained(client, config).await;
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
        } => match prepare_replace_recovery(config, client, mode, pid, recovery_generation).await?
        {
            ReplacePrep::Finished(url) => return Ok(url),
            ReplacePrep::Continue(manifest) => manifest,
        },
        ServiceState::Start => None,
    };
    start_and_wait_for_adapter(config, client, recovery_manifest, replaced).await
}

enum ReplacePrep {
    Finished(String),
    Continue(Option<String>),
}

async fn prepare_replace_recovery(
    config: &ServiceConfig,
    client: &reqwest::Client,
    mode: Mode,
    pid: Option<u32>,
    recovery_generation: Option<String>,
) -> Result<ReplacePrep> {
    if let Some(url) = try_live_replace_update(config, client, pid).await? {
        return Ok(ReplacePrep::Finished(url));
    }
    let recovery_generation = usable_recovery_generation(config, recovery_generation.as_deref())?;
    let attached = super::session_process::any_launch_is_active(config.options.listen.port());
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
    Ok(ReplacePrep::Continue(recovery_generation))
}

async fn try_live_replace_update(
    config: &ServiceConfig,
    client: &reqwest::Client,
    pid: Option<u32>,
) -> Result<Option<String>> {
    let Some(health) = super::health::fetch_health(client, config).await else {
        return Ok(None);
    };
    if !super::promote::live_update_eligible(&health, config) {
        return Ok(None);
    }
    if let Some(url) = super::promote::try_canonical(client, config, &health).await? {
        pending_hot_swap::clear_if_current(config);
        notify_swap_if_replaced(true, config);
        return Ok(Some(url));
    }
    eprintln!(
        "claudex: live update handover failed; keeping pid {pid:?} on {} so Claude Code stays connected",
        config.base_url()
    );
    Ok(Some(config.base_url()))
}

async fn start_and_wait_for_adapter(
    config: &ServiceConfig,
    client: &reqwest::Client,
    recovery_manifest: Option<String>,
    replaced: bool,
) -> Result<String> {
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
    // WaitIdle polls Defer without calling this arm helper.
    debug_assert!(matches!(mode, Mode::HotSwap | Mode::Ensure));
    let _ = mode;
    pending_hot_swap::disarm(config);
    if let Some(url) = try_defer_live_update(config, client, pid).await? {
        return Ok(url);
    }
    let outcome = pending_hot_swap::arm(config)?;
    eprintln!(
        "claudex: retaining active adapter pid {pid:?}; routing new sessions to a current-build listener ({active_http_requests} HTTP request(s), {active_provider_turns} provider turn(s), {active_subagents} SubAgent(s); live launch sessions kept; idle hot-swap waiter pid {} for build {})",
        outcome.pid(),
        env!("CLAUDEX_BUILD_ID"),
    );
    let url = fallback::ensure_current_generation(client, config)
        .await
        .context("start current-build listener while stale adapter is active")?;
    let _ = super::live::publish_url(config, &url);
    notify_live_listener(config, &url);
    log_live_listener(config);
    Ok(url)
}

async fn try_defer_live_update(
    config: &ServiceConfig,
    client: &reqwest::Client,
    pid: Option<u32>,
) -> Result<Option<String>> {
    let Some(health) = super::health::fetch_health(client, config).await else {
        return Ok(None);
    };
    if !super::promote::live_update_eligible(&health, config) {
        return Ok(None);
    }
    match super::promote::try_canonical(client, config, &health).await {
        Ok(Some(url)) => {
            notify_swap_if_replaced(true, config);
            Ok(Some(url))
        }
        Ok(None) => Ok(None),
        Err(error) => {
            eprintln!(
                "claudex: live update handover failed ({error:#}); retaining pid {pid:?} on {} so Claude Code stays connected",
                config.base_url()
            );
            Ok(None)
        }
    }
}

pub(super) fn notify_live_listener(config: &ServiceConfig, url: &str) {
    if let Ok(live_listen) = super::live::parse_listen_url(url) {
        macos_notify::live_ready(config, live_listen);
    }
}

pub(super) fn log_live_listener(config: &ServiceConfig) {
    if let Ok(Some(live)) = super::live::read(config) {
        eprintln!(
            "claudex: live generation {} on {}",
            live.build_id, live.listen
        );
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "ensure_tests.rs"]
mod tests;
