use anyhow::{Context, Result, bail};
use std::time::Duration;
use tokio::sync::Semaphore;

pub(crate) const DEFAULT_MAX_PROCESSES: usize = 20;
pub(crate) const DEFAULT_TIMEOUT_MINUTES: u64 = 120;
const MAX_PROCESSES_ENV: &str = "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES";
const TIMEOUT_MINUTES_ENV: &str = "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES";
pub(in crate::anthropic) struct SubscriptionLimits {
    pub(in crate::anthropic) max_processes: usize,
    pub(in crate::anthropic) timeout: Duration,
}

impl SubscriptionLimits {
    pub(crate) fn new(max_processes: usize, timeout_minutes: u64) -> Result<Self> {
        if max_processes == 0 || max_processes > Semaphore::MAX_PERMITS {
            bail!("subscription process limit is out of range");
        }
        let timeout_seconds = timeout_minutes
            .checked_mul(60)
            .filter(|seconds| *seconds > 0)
            .context("subscription timeout is out of range")?;
        Ok(Self {
            max_processes,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }
}

pub(in crate::anthropic) fn subscription_limits() -> SubscriptionLimits {
    subscription_limits_from(|name| std::env::var(name).ok())
}

pub(in crate::anthropic) fn subscription_limits_from(get: impl Fn(&str) -> Option<String>) -> SubscriptionLimits {
    let max_processes = positive_usize(get(MAX_PROCESSES_ENV)).unwrap_or(DEFAULT_MAX_PROCESSES);
    let timeout_seconds = positive_u64(get(TIMEOUT_MINUTES_ENV))
        .and_then(|minutes| minutes.checked_mul(60))
        .unwrap_or(DEFAULT_TIMEOUT_MINUTES * 60);
    SubscriptionLimits {
        max_processes,
        timeout: Duration::from_secs(timeout_seconds),
    }
}

pub(in crate::anthropic) fn positive_usize(value: Option<String>) -> Option<usize> {
    value?
        .parse()
        .ok()
        .filter(|value| *value > 0 && *value <= Semaphore::MAX_PERMITS)
}

pub(in crate::anthropic) fn positive_u64(value: Option<String>) -> Option<u64> {
    value?.parse().ok().filter(|value| *value > 0)
}

