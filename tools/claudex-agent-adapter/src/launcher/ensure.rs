use anyhow::Result;

use super::{ServiceConfig, handover, handover::ServiceState, launcher_lock, pending_hot_swap};

mod notify;
#[path = "ensure_wait_idle.rs"]
mod wait_idle;
#[cfg(test)]
use notify::recovery_snapshot_is_missing;
pub(super) use notify::{
    listener_was_replaced, log_live_listener, notify_live_listener, notify_swap_if_replaced,
    should_retry_idle_replace, usable_recovery_generation,
};

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
        } => {
            match prepare_replace_recovery(config, client, mode, pid, recovery_generation).await? {
                ReplacePrep::Finished(url) => return Ok(url),
                ReplacePrep::Continue(manifest) => manifest,
            }
        }
        ServiceState::Start => None,
    };
    start_and_wait_for_adapter(config, client, recovery_manifest, replaced).await
}

#[path = "ensure_replace.rs"]
mod replace;
#[cfg(test)]
use replace::try_defer_live_update;
use replace::{
    ReplacePrep, defer_busy_listener, prepare_replace_recovery, start_and_wait_for_adapter,
};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "ensure_tests.rs"]
mod tests;
