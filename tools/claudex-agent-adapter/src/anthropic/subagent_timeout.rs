use std::{future::Future, sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use axum::{body::Body, http::Response};

use super::{ActiveTurn, Bridge, MessagesRequest, model_concurrency::ModelPermit};
use crate::agent_backend::TurnCancellation;

mod completion;

pub(crate) const SUBAGENT_HARD_TIMEOUT_ENV: &str = "CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS";
pub(crate) const LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV: &str =
    "CLAUDEX_SUBAGENT_RESPONSE_TIMEOUT_SECONDS";

// ACP cancellation can spend up to two settlement windows: one for the
// session/cancel request and one for the in-flight prompt. Bound the complete
// adapter-side request as well so a wedged command queue cannot turn a hard
// timeout into an unbounded wait.
const PROVIDER_CANCEL_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(5);

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

impl Bridge {
    pub(super) async fn provider_messages(
        self: &Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        is_subagent: bool,
        run_in_background: bool,
    ) -> Result<Response<Body>> {
        let concurrency_ticket = self.model_concurrency.ticket(
            &request.model,
            self.app.max_concurrency_for_model(&request.model),
        );
        // Open SSE before prepare_turn so Claude Code receives message_start and
        // keepalives while the provider session starts.
        if request.stream {
            return Ok(self.streaming_messages(
                request,
                input_tokens,
                effort,
                concurrency_ticket,
                is_subagent,
                run_in_background,
            ));
        }
        let permit = match concurrency_ticket {
            Some(ticket) => Some(ticket.acquire_for(!is_subagent).await?),
            None => None,
        };
        let turn = self.prepare_turn(&request, input_tokens, effort).await?;
        if is_subagent && run_in_background {
            self.non_streaming_subagent_response(turn, permit).await
        } else {
            self.non_streaming_response(turn).await
        }
    }

    pub(super) async fn non_streaming_subagent_response(
        self: &Arc<Self>,
        turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) -> Result<Response<Body>> {
        self.non_streaming_subagent_response_with_timeout(turn, permit, self.subagent_hard_timeout)
            .await
    }

    // This bounded retry loop retains ActiveTurn until completion or provider cleanup.
    #[allow(clippy::excessive_nesting)]
    pub(super) async fn non_streaming_subagent_response_with_timeout(
        self: &Arc<Self>,
        mut turn: ActiveTurn,
        _permit: Option<ModelPermit>,
        timeout: Option<Duration>,
    ) -> Result<Response<Body>> {
        loop {
            let segment = completes_within(
                timeout,
                self.wait_for_segment(
                    &turn.session,
                    &turn.events,
                    turn.input_tokens,
                    &turn.extras,
                    &turn.routing_system,
                    None,
                ),
            )
            .await;
            let Some(segment) = segment else {
                let timeout = timeout.expect("elapsed wait has a configured timeout");
                return Err(self.expire_subagent_turn(&turn, timeout).await);
            };
            match segment {
                Ok(segment) => return Ok(completion::finish(self, turn, segment).await),
                Err(error) => turn = self.retry_subagent_context(&mut turn, error).await?,
            }
        }
    }

    async fn retry_subagent_context(
        &self,
        turn: &mut ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ActiveTurn> {
        let error_text = error.to_string();
        let retry = self.context_retry_or_error(turn, error).await?;
        tracing::warn!(
            error = %error_text,
            thread_id = %turn.session.thread_id,
            "retrying completed SubAgent turn after context window exceeded"
        );
        self.retry_after_context_window(retry, &turn.session, turn.input_tokens)
            .await
    }

    pub(super) async fn expire_subagent_turn(
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

    async fn settle_expired_provider(
        &self,
        turn: &ActiveTurn,
        cancellation: Result<TurnCancellation>,
    ) {
        self.settle_expired_provider_with(turn, cancellation, || {
            self.app.abort_turn_provider(&turn.session.thread_id)
        })
        .await;
    }

    async fn settle_expired_provider_with<Abort, AbortFuture>(
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

async fn provider_cancellation_within(
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

#[cfg(test)]
#[path = "subagent_timeout_tests.rs"]
mod tests;
