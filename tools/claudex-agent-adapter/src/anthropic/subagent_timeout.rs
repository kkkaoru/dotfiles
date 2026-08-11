use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
#[cfg(test)]
use anyhow::anyhow;
use axum::{body::Body, http::Response};

use super::{ActiveTurn, Bridge, MessagesRequest, model_concurrency::ModelPermit};

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
        let _active_subagent = self.track_active_subagent(is_subagent, &request);
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
    pub(super) async fn non_streaming_subagent_response_with_timeout(
        self: &Arc<Self>,
        mut turn: ActiveTurn,
        _permit: Option<ModelPermit>,
        timeout: Option<Duration>,
    ) -> Result<Response<Body>> {
        loop {
            match self.next_subagent_segment(&turn, timeout).await? {
                Ok(segment) => return Ok(completion::finish(self, turn, segment).await),
                Err(error) => turn = self.retry_subagent_context(&mut turn, error).await?,
            }
        }
    }

    async fn next_subagent_segment(
        self: &Arc<Self>,
        turn: &ActiveTurn,
        timeout: Option<Duration>,
    ) -> Result<Result<super::Segment, anyhow::Error>> {
        let Some(segment) = completes_within(
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
        .await
        else {
            let timeout = timeout.expect("elapsed wait has a configured timeout");
            return Err(self.expire_subagent_turn(turn, timeout).await);
        };
        Ok(segment)
    }

    pub(super) fn track_active_subagent(
        &self,
        is_subagent: bool,
        request: &super::MessagesRequest,
    ) -> Option<super::active_subagent_models::ActiveSubagentGuard> {
        if !is_subagent {
            return None;
        }
        let agent_id = super::request_identity::request_agent_id(request);
        Some(
            self.active_subagent_models
                .acquire(&request.model, agent_id.as_deref()),
        )
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
}

mod expire;
#[cfg(test)]
use expire::provider_cancellation_within;

#[cfg(test)]
#[path = "subagent_timeout_tests.rs"]
mod tests;
