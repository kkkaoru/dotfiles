use std::time::Duration;

use anyhow::Result;

use super::super::{
    ServiceConfig, handover, handover::ServiceState, launcher_lock, pending_hot_swap,
};
use super::{Mode, apply_inspected_state, should_retry_idle_replace};

/// Production recheck cadence while a busy listener is `Defer`ing idle hot-swap.
/// Keep far under 1s so installs promote promptly after in-flight work drains.
pub(crate) const WAIT_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(not(test))]
const WAIT_IDLE_POLL: Duration = WAIT_IDLE_POLL_INTERVAL;
#[cfg(test)]
const WAIT_IDLE_POLL: Duration = Duration::from_millis(0);
/// Production waiters keep retrying Replace after a failed handover. Tests
/// allow one retry so the sleep/continue arm stays measurable under llvm-cov,
/// then fail closed.
#[cfg(not(test))]
const WAIT_IDLE_REPLACE_RETRIES: Option<u32> = None;
#[cfg(test)]
const WAIT_IDLE_REPLACE_RETRIES: Option<u32> = Some(1);
#[cfg(test)]
std::thread_local! {
    // Lets fixtures bind a current-build listener between the outer Start
    // observation and wait_for_hot_swap_idle's re-inspect.
    static WAIT_IDLE_INSPECT_PAUSE: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
}

pub(super) async fn wait_until_idle_then_replace(config: &ServiceConfig) -> Result<String> {
    let client = reqwest::Client::new();
    let mut replace_failures: u32 = 0;
    loop {
        let state = handover::inspect_service(&client, config).await;
        if let Some(url) =
            handle_wait_idle_state(config, &client, state, &mut replace_failures).await?
        {
            return Ok(url);
        }
    }
}

async fn handle_wait_idle_state(
    config: &ServiceConfig,
    client: &reqwest::Client,
    state: ServiceState,
    replace_failures: &mut u32,
) -> Result<Option<String>> {
    match state {
        ServiceState::Reuse => {
            pending_hot_swap::clear_if_current(config);
            Ok(Some(config.base_url()))
        }
        ServiceState::Defer { .. } => {
            tokio::time::sleep(WAIT_IDLE_POLL).await;
            Ok(None)
        }
        ServiceState::Start => wait_idle_after_start(config, client).await,
        ServiceState::Replace { .. } => {
            wait_idle_after_replace(config, client, replace_failures).await
        }
    }
}

async fn wait_idle_after_start(
    config: &ServiceConfig,
    client: &reqwest::Client,
) -> Result<Option<String>> {
    let _lock = launcher_lock::acquire(&config.lock_path)?;
    wait_idle_inspect_pause().await;
    let state = handover::wait_for_hot_swap_idle(client, config).await?;
    match state {
        ServiceState::Defer { .. } => Ok(None),
        ServiceState::Reuse => {
            pending_hot_swap::clear_if_current(config);
            Ok(Some(config.base_url()))
        }
        state => {
            let url = apply_inspected_state(config, client, Mode::HotSwap, state).await?;
            pending_hot_swap::clear_if_current(config);
            Ok(Some(url))
        }
    }
}

async fn wait_idle_after_replace(
    config: &ServiceConfig,
    client: &reqwest::Client,
    replace_failures: &mut u32,
) -> Result<Option<String>> {
    let outcome = {
        let _lock = launcher_lock::acquire(&config.lock_path)?;
        wait_idle_inspect_pause().await;
        let state = handover::wait_for_hot_swap_idle(client, config).await?;
        match state {
            ServiceState::Defer { .. } => None,
            ServiceState::Reuse => {
                pending_hot_swap::clear_if_current(config);
                return Ok(Some(config.base_url()));
            }
            state => Some(apply_inspected_state(config, client, Mode::HotSwap, state).await),
        }
    };
    finish_wait_idle_replace(config, outcome, replace_failures).await
}

async fn finish_wait_idle_replace(
    config: &ServiceConfig,
    outcome: Option<Result<String>>,
    replace_failures: &mut u32,
) -> Result<Option<String>> {
    match outcome {
        None => Ok(None),
        Some(Ok(url)) => {
            pending_hot_swap::clear_if_current(config);
            Ok(Some(url))
        }
        Some(Err(error)) => {
            *replace_failures = replace_failures.saturating_add(1);
            eprintln!("claudex: idle hot-swap replace failed ({error:#}); waiting to retry");
            if !should_retry_idle_replace(*replace_failures, WAIT_IDLE_REPLACE_RETRIES) {
                return Err(error);
            }
            tokio::time::sleep(WAIT_IDLE_POLL).await;
            Ok(None)
        }
    }
}


async fn wait_idle_inspect_pause() {
    #[cfg(test)]
    {
        let pause = WAIT_IDLE_INSPECT_PAUSE.with(|cell| cell.get());
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
        }
    }
}

#[cfg(test)]
pub(crate) struct WaitIdleInspectPause;

#[cfg(test)]
impl WaitIdleInspectPause {
    pub(crate) fn arm(pause: Duration) -> Self {
        WAIT_IDLE_INSPECT_PAUSE.with(|cell| cell.set(pause));
        Self
    }
}

#[cfg(test)]
impl Drop for WaitIdleInspectPause {
    fn drop(&mut self) {
        WAIT_IDLE_INSPECT_PAUSE.with(|cell| cell.set(Duration::ZERO));
    }
}
