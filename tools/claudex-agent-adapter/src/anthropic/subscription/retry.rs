use std::{future::Future, time::Duration};

use anyhow::{Error, Result};

pub(in crate::anthropic) const MAX_TRANSIENT_RETRIES: usize = 2;
const BASE_DELAY: Duration = Duration::from_millis(250);
const STREAM_OUTPUT_MARKER: &str = "subscription stream already emitted frames";

pub(in crate::anthropic) fn should_retry_subscription(error: &Error) -> bool {
    let message = error
        .chain()
        .map(|cause| cause.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    !message.contains(STREAM_OUTPUT_MARKER)
        && (is_empty_subscription_exit(&message)
            || [
                "502",
                "bad gateway",
                "gateway timeout",
                "service unavailable",
                "temporarily unavailable",
            ]
            .iter()
            .any(|marker| message.contains(marker)))
}

fn is_empty_subscription_exit(message: &str) -> bool {
    message.contains("claude subscription")
        && message.contains("exited with")
        && message
            .rsplit_once(':')
            .is_some_and(|(_, stderr)| stderr.trim().is_empty())
}

pub(in crate::anthropic) fn transient_retry_delay(retry: usize) -> Duration {
    BASE_DELAY.saturating_mul(2u32.saturating_pow(retry.saturating_sub(1).min(4) as u32))
}

pub(in crate::anthropic) async fn with_transient_retries<T, F, Fut>(
    model: &str,
    mut operation: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut retry = 0;
    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(error) if retry < MAX_TRANSIENT_RETRIES && should_retry_subscription(&error) => {
                retry += 1;
                let delay = transient_retry_delay(retry);
                tracing::warn!(
                    %model,
                    retry,
                    max_retries = MAX_TRANSIENT_RETRIES,
                    delay_ms = delay.as_millis(),
                    error = ?error,
                    "retrying transient Claude subscription failure"
                );
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
}
