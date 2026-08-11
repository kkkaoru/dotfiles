use std::{future::Future, time::Duration};

#[cfg(test)]
use anyhow::{Result, anyhow};
#[cfg(test)]
use axum::{body::Body, http::Response};

use super::{ActiveTurn, Bridge};

mod completion;
#[path = "subagent_timeout_run.rs"]
mod run;

pub(crate) const SUBAGENT_HARD_TIMEOUT_ENV: &str = "CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS";
pub(crate) const LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV: &str =
    "CLAUDEX_SUBAGENT_RESPONSE_TIMEOUT_SECONDS";

// ACP cancellation can spend up to two settlement windows: one for the
// session/cancel request and one for the in-flight prompt. Bound the complete
// adapter-side request as well so a wedged command queue cannot turn a hard
// timeout into an unbounded wait.
pub(super) const PROVIDER_CANCEL_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn subagent_hard_timeout() -> Option<Duration> {
    subagent_hard_timeout_from(|name| std::env::var(name).ok())
}

pub(crate) fn subagent_hard_timeout_from(get: impl Fn(&str) -> Option<String>) -> Option<Duration> {
    get(SUBAGENT_HARD_TIMEOUT_ENV)
        .or_else(|| get(LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV))
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
}

pub(super) async fn completes_within<T>(
    timeout: Option<Duration>,
    future: impl Future<Output = T>,
) -> Option<T> {
    match timeout {
        Some(timeout) => tokio::time::timeout(timeout, future).await.ok(),
        None => Some(future.await),
    }
}


mod expire;
#[cfg(test)]
use expire::provider_cancellation_within;

#[cfg(test)]
#[path = "subagent_timeout_tests.rs"]
mod tests;
