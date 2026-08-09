use anyhow::{Context, Result, bail};

use super::{
    ServiceConfig, daemon_process, daemon_start, fallback, handover, handover::ServiceState,
    health::wait_until_ready, launcher_lock, preflight, recovery,
};

pub(super) async fn run(config: &ServiceConfig, hot_swap: bool) -> Result<String> {
    let _lock = launcher_lock::acquire(&config.lock_path)?;
    let client = reqwest::Client::new();
    let state = if hot_swap {
        handover::wait_for_hot_swap_idle(&client, config).await?
    } else {
        handover::inspect_service(&client, config).await
    };
    let recovery_manifest = match state {
        ServiceState::Reuse => return Ok(config.base_url()),
        ServiceState::Defer {
            pid,
            active_http_requests,
            active_provider_turns,
        } if hot_swap => {
            bail!(
                "hot-swap timed out: adapter pid {pid:?} still has active work ({active_http_requests} HTTP request(s), {active_provider_turns} provider turn(s))"
            );
        }
        ServiceState::Defer {
            pid,
            active_http_requests,
            active_provider_turns,
        } => {
            eprintln!(
                "claudex: retaining active adapter pid {:?}; routing this new session to a current-build listener ({} HTTP request(s), {} provider turn(s); live launch sessions kept)",
                pid, active_http_requests, active_provider_turns
            );
            return fallback::ensure_current_generation(&client, config)
                .await
                .context("start current-build listener while stale adapter is active");
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
                if hot_swap { " (hot-swap)" } else { "" },
                if attached {
                    "; launch TUI kept on this port"
                } else {
                    ""
                }
            );
            preflight::verify(&client, config).await?;
            handover::release_stale_listener(&client, config, pid).await?;
            recovery_generation
        }
        ServiceState::Start => None,
    };
    let started_pid = match daemon_start::start_adapter(config) {
        Ok(pid) => pid,
        Err(error) => {
            return recovery::after_update_failure(
                &client,
                config,
                recovery_manifest.as_deref(),
                error.context("start new adapter generation"),
            )
            .await;
        }
    };
    if let Err(error) = wait_until_ready(&client, config).await {
        if daemon_process::matches(started_pid, &config.executable) {
            daemon_process::terminate(started_pid);
        }
        return recovery::after_update_failure(
            &client,
            config,
            recovery_manifest.as_deref(),
            error,
        )
        .await;
    }
    Ok(config.base_url())
}
