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
// Coverage excludes test implementation; production behavior remains measured.
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

    #[tokio::test]
    async fn queue_wait_returns_ready_values_and_errors_without_a_timeout() {
        assert_eq!(
            acquire(
                AcpProvider::Grok,
                "turn/start",
                std::future::ready(Ok::<_, anyhow::Error>("ready")),
            )
            .await
            .unwrap(),
            "ready"
        );
        assert!(
            acquire_with_timeout(
                AcpProvider::Grok,
                "turn/start",
                Duration::from_millis(1),
                std::future::ready(Err::<(), _>(anyhow!("unavailable"))),
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable")
        );
    }

    #[tokio::test]
    async fn disconnected_start_response_cancels_only_a_successful_queued_turn() {
        let active_turns = ActiveTurns::default();
        let (cancel, cancelled) = oneshot::channel();
        active_turns
            .borrow_mut()
            .insert("session".to_owned(), Some(cancel));

        let (response, requester) = oneshot::channel();
        drop(requester);
        finish_start_turn(&active_turns, "session", response, Ok(()));
        assert!(cancelled.await.is_ok());

        let (response, requester) = oneshot::channel();
        drop(requester);
        finish_start_turn(&active_turns, "session", response, Err(anyhow!("rejected")));
        assert!(active_turns.borrow().contains_key("session"));
    }

    #[tokio::test]
    async fn delivered_start_response_leaves_the_queued_turn_alone() {
        let active_turns = ActiveTurns::default();
        let (cancel, mut cancelled) = oneshot::channel();
        active_turns
            .borrow_mut()
            .insert("session".to_owned(), Some(cancel));
        let (response, result) = oneshot::channel();

        finish_start_turn(&active_turns, "session", response, Ok(()));

        assert!(result.await.unwrap().is_ok());
        assert!(cancelled.try_recv().is_err());
    }
}
