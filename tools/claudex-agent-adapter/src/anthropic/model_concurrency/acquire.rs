use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use super::{DEFAULT_WAIT_TIMEOUT, LimitedModel, MODEL_CONCURRENCY_WAIT_TIMEOUT_ENV};

pub(super) async fn acquire_interactive_permit(
    entry: &LimitedModel,
    wait_timeout: Duration,
    model: &str,
) -> Result<OwnedSemaphorePermit> {
    if wait_timeout.is_zero() {
        return entry
            .interactive
            .clone()
            .try_acquire_owned()
            .or_else(|_| entry.slots.clone().try_acquire_owned())
            .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"));
    }
    timeout(wait_timeout, async {
        tokio::select! {
            permit = entry.interactive.clone().acquire_owned() => permit,
            permit = entry.slots.clone().acquire_owned() => permit,
        }
    })
    .await
    .map_err(|_| {
        anyhow!("model `{model}` concurrency model admission timed out after {wait_timeout:?}")
    })?
    .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"))
}

pub(super) async fn acquire_permit(
    semaphore: Arc<Semaphore>,
    wait_timeout: Duration,
    stage: &str,
    model: &str,
) -> Result<OwnedSemaphorePermit> {
    if wait_timeout.is_zero() {
        semaphore
            .try_acquire_owned()
            .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"))
    } else {
        timeout(wait_timeout, semaphore.acquire_owned())
            .await
            .map_err(|_| {
                anyhow!(
                    "model `{model}` concurrency {stage} admission timed out after {wait_timeout:?}"
                )
            })?
            .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"))
    }
}

pub(super) fn admission_capacity(limit: usize) -> usize {
    limit
        .saturating_mul(3)
        .min(Semaphore::MAX_PERMITS)
        .max(limit)
}

pub(super) fn model_concurrency_wait_timeout() -> Duration {
    parse_wait_timeout(
        std::env::var(MODEL_CONCURRENCY_WAIT_TIMEOUT_ENV)
            .ok()
            .as_deref(),
    )
}

pub(super) fn parse_wait_timeout(value: Option<&str>) -> Duration {
    let Some(value) = value else {
        return DEFAULT_WAIT_TIMEOUT;
    };
    let Ok(milliseconds) = value.parse::<u64>() else {
        return DEFAULT_WAIT_TIMEOUT;
    };
    Duration::from_millis(milliseconds)
}
