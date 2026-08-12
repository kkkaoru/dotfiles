use anyhow::{Context, Result};

use super::super::health::wait_until_ready;
use super::super::{daemon_start, fallback, handover, pending_hot_swap, preflight, recovery};
use super::{
    Mode, ServiceConfig, log_live_listener, notify_live_listener, notify_swap_if_replaced,
    usable_recovery_generation,
};

pub(super) enum ReplacePrep {
    Finished(String),
    Continue(Option<String>),
}

pub(super) async fn prepare_replace_recovery(
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
    let attached =
        super::super::session_process::any_launch_is_active(config.options.listen.port());
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

pub(super) async fn try_live_replace_update(
    config: &ServiceConfig,
    client: &reqwest::Client,
    pid: Option<u32>,
) -> Result<Option<String>> {
    let Some(health) = super::super::health::fetch_health(client, config).await else {
        return Ok(None);
    };
    if !super::super::promote::live_update_eligible(&health, config) {
        return Ok(None);
    }
    if let Some(url) = super::super::promote::try_canonical(client, config, &health).await? {
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

pub(super) async fn start_and_wait_for_adapter(
    config: &ServiceConfig,
    client: &reqwest::Client,
    recovery_manifest: Option<String>,
    replaced: bool,
) -> Result<String> {
    let started = match daemon_start::start_adapter(config) {
        Ok(pid) => daemon_start::StartedDaemon::new(pid),
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
        drop(started);
        return recovery::after_update_failure(client, config, recovery_manifest.as_deref(), error)
            .await;
    }
    if let Err(error) =
        super::super::live::publish_listen(config, config.options.listen, Some(started.pid()))
    {
        drop(started);
        return recovery::after_update_failure(
            client,
            config,
            recovery_manifest.as_deref(),
            error.context("publish new adapter generation"),
        )
        .await;
    }
    started.disarm();
    pending_hot_swap::clear_if_current(config);
    notify_swap_if_replaced(replaced, config);
    Ok(config.base_url())
}

pub(super) async fn defer_busy_listener(
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
    let _ = super::super::live::publish_url(config, &url);
    notify_live_listener(config, &url);
    log_live_listener(config);
    Ok(url)
}

pub(super) async fn try_defer_live_update(
    config: &ServiceConfig,
    client: &reqwest::Client,
    pid: Option<u32>,
) -> Result<Option<String>> {
    let Some(health) = super::super::health::fetch_health(client, config).await else {
        return Ok(None);
    };
    if !super::super::promote::live_update_eligible(&health, config) {
        return Ok(None);
    }
    match super::super::promote::try_canonical(client, config, &health).await {
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
