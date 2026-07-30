use std::future::Future;

use anyhow::Result;
use tokio::sync::oneshot;

use super::{
    connection::AcpProvider,
    turns::{ActiveTurns, cancel_turn},
};

pub(super) async fn acquire<T, F>(provider: AcpProvider, operation: &str, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if !provider.is_session_scoped_configured() {
        return future.await;
    }
    // A configured ACP may legitimately have a long-running SubAgent turn.
    // Keep queue pressure cancellable by the caller, but never turn it into a
    // synthetic 30-second failure. User turns retain a reserved permit in
    // `acquire_turn_permit`, so background work cannot starve the main session.
    tracing::debug!(
        provider = provider.label(),
        operation,
        "waiting for ACP queue capacity"
    );
    future.await
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
    use anyhow::anyhow;

    use super::*;

    #[tokio::test]
    async fn configured_wait_is_caller_cancellable_without_a_synthetic_timeout() {
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(5),
            acquire(
                AcpProvider::Configured,
                "turn/start",
                std::future::pending::<Result<()>>(),
            ),
        )
        .await;
        assert!(result.is_err(), "the caller-owned cancellation should win");

        assert!(
            acquire(
                AcpProvider::ConfiguredLaunchScoped,
                "turn/start",
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
            acquire(
                AcpProvider::Grok,
                "turn/start",
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
