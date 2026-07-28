use std::{future::Future, time::Duration};

use anyhow::{Result, anyhow};
use tokio::sync::oneshot;

use super::{
    connection::AcpProvider,
    turns::{ActiveTurns, cancel_turn},
};

const CONFIGURED_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn acquire<T, F>(provider: AcpProvider, operation: &str, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    acquire_with_timeout(provider, operation, CONFIGURED_WAIT_TIMEOUT, future).await
}

async fn acquire_with_timeout<T, F>(
    provider: AcpProvider,
    operation: &str,
    timeout: Duration,
    future: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if !provider.is_session_scoped_configured() {
        return future.await;
    }
    tokio::time::timeout(timeout, future).await.map_err(|_| {
        anyhow!(
            "{} ACP {operation} queue wait timed out after {timeout:?}",
            provider.label()
        )
    })?
}

pub(super) fn finish_start_turn(
    active_turns: &ActiveTurns,
    session_id: &str,
    response: oneshot::Sender<Result<()>>,
    result: Result<()>,
) {
    let Err(unsent) = response.send(result) else {
        return;
    };
    if unsent.is_ok() {
        let (cancelled, _result) = oneshot::channel();
        cancel_turn(active_turns, session_id, cancelled);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn configured_wait_is_bounded_without_changing_launch_scoped_providers() {
        let stalled = acquire_with_timeout(
            AcpProvider::Configured,
            "turn/start",
            Duration::from_millis(1),
            std::future::pending::<Result<()>>(),
        )
        .await
        .unwrap_err();
        assert!(stalled.to_string().contains("queue wait timed out"));

        assert!(
            acquire_with_timeout(
                AcpProvider::ConfiguredLaunchScoped,
                "turn/start",
                Duration::from_millis(1),
                std::future::ready(Ok(())),
            )
            .await
            .is_ok()
        );
    }
}
