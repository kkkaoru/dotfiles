use std::future::Future;
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::{ActiveTurn, Bridge, PROVIDER_CANCEL_SETTLEMENT_TIMEOUT};
use crate::agent_backend::TurnCancellation;

impl Bridge {
    pub(in crate::anthropic) async fn expire_subagent_turn(
        &self,
        turn: &ActiveTurn,
        timeout: Duration,
    ) -> anyhow::Error {
        #[cfg(test)]
        self.subagent_hard_timeout_cancel_attempts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cancellation = provider_cancellation_within(
            self.app.cancel_turn(&turn.session.thread_id),
            PROVIDER_CANCEL_SETTLEMENT_TIMEOUT,
        )
        .await;
        self.settle_expired_provider(turn, cancellation).await;
        anyhow!(
            "SubAgent provider turn exceeded the configured hard timeout of {} seconds",
            timeout.as_secs()
        )
    }

    pub(super) async fn settle_expired_provider(
        &self,
        turn: &ActiveTurn,
        cancellation: Result<TurnCancellation>,
    ) {
        self.settle_expired_provider_with(turn, cancellation, || {
            self.app.abort_turn_provider(&turn.session.thread_id)
        })
        .await;
    }

    pub(super) async fn settle_expired_provider_with<Abort, AbortFuture>(
        &self,
        turn: &ActiveTurn,
        cancellation: Result<TurnCancellation>,
        abort_provider: Abort,
    ) where
        Abort: FnOnce() -> AbortFuture,
        AbortFuture: Future<Output = Result<()>>,
    {
        // Retire the session before a potentially slow provider abort. The
        // ActiveTurn still owns its slot until this method returns.
        self.remove_session(&turn.session).await;
        let needs_abort = match cancellation {
            Ok(TurnCancellation::Settled) => {
                tracing::debug!(
                    thread_id = %turn.session.thread_id,
                    "cancelled SubAgent provider turn at configured hard timeout"
                );
                false
            }
            Ok(TurnCancellation::Unsupported) => true,
            Err(error) => {
                tracing::warn!(
                    %error,
                    thread_id = %turn.session.thread_id,
                    "failed to settle expired SubAgent cancellation; aborting its provider"
                );
                true
            }
        };
        if needs_abort && let Err(error) = abort_provider().await {
            tracing::warn!(
                %error,
                thread_id = %turn.session.thread_id,
                "targeted provider abort failed; shutting down all providers before releasing the expired SubAgent turn"
            );
            // A routed leaf may already have been retired by an overlapping
            // failure path. Await the shared backend lifecycle as a final
            // cleanup join rather than releasing permits on an unverified
            // provider state.
            self.app.shutdown().await;
        }
    }
}

pub(super) async fn provider_cancellation_within(
    cancellation: impl Future<Output = Result<TurnCancellation>>,
    timeout: Duration,
) -> Result<TurnCancellation> {
    tokio::time::timeout(timeout, cancellation)
        .await
        .map_err(|_| {
            anyhow!(
                "SubAgent provider cancellation did not settle within {} seconds",
                timeout.as_secs()
            )
        })?
}

