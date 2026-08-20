use std::ops::ControlFlow;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::usage_limit_failover::is_usage_limit_exceeded;
use super::super::{ActiveTurn, Bridge, ContextRetry, content::anthropic_response};
use super::Segment;

impl Bridge {
    pub(in crate::anthropic) async fn retry_after_provider_failure(
        &self,
        mut turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ActiveTurn> {
        if !crate::anthropic::token_efficiency::should_retry_provider_failure(&error)
            || !super::is_provider_stream_closed(&error)
            || self
                .app_for_session(&turn.session)
                .model_is_alive(&turn.session.model)
        {
            self.remove_session(&turn.session).await;
            return Err(error);
        }
        let Some(retry) = turn.retry.take() else {
            self.remove_session(&turn.session).await;
            return Err(error);
        };
        let model = turn.session.model.clone();
        let input_tokens = turn.input_tokens;
        let previous = std::sync::Arc::clone(&turn.session);
        tracing::warn!(
            model = %model,
            "retrying a provider failure after the routed ACP process was recycled"
        );
        self.retry_after_context_window(retry, &previous, input_tokens)
            .await
    }

    pub(in crate::anthropic) async fn context_retry_or_error(
        &self,
        turn: &mut ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ContextRetry> {
        let retry = super::super::session::is_context_window_exceeded(&error)
            .then(|| turn.retry.take())
            .flatten();
        match retry {
            Some(retry) => Ok(retry),
            None => {
                self.remove_session(&turn.session).await;
                Err(error)
            }
        }
    }

    pub(in crate::anthropic) async fn non_streaming_response(
        &self,
        mut turn: ActiveTurn,
    ) -> Result<Response<Body>> {
        loop {
            let segment = self
                .wait_for_segment(
                    &turn.session,
                    &turn.events,
                    turn.input_tokens,
                    &turn.extras,
                    &turn.routing_system,
                    None,
                )
                .await;
            match self.advance_non_streaming_turn(turn, segment).await? {
                ControlFlow::Break(response) => return Ok(response),
                ControlFlow::Continue(next) => turn = next,
            }
        }
    }

    async fn advance_non_streaming_turn(
        &self,
        turn: ActiveTurn,
        segment: Result<Segment>,
    ) -> Result<ControlFlow<Response<Body>, ActiveTurn>> {
        match segment {
            Ok(segment) => Ok(ControlFlow::Break(
                self.finish_non_streaming_turn(turn, segment).await,
            )),
            Err(error) if is_usage_limit_exceeded(&error) => {
                self.advance_after_usage_limit(turn, error).await
            }
            Err(error) => Ok(ControlFlow::Continue(
                self.retry_non_streaming_after_context(turn, error).await?,
            )),
        }
    }

    async fn finish_non_streaming_turn(
        &self,
        turn: ActiveTurn,
        segment: Segment,
    ) -> Response<Body> {
        super::commit_transcript(&turn.session, turn.extras, &segment).await;
        anthropic_response(segment, &turn.response_model)
    }

    async fn advance_after_usage_limit(
        &self,
        turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ControlFlow<Response<Body>, ActiveTurn>> {
        match self.failover_usage_limit_turn(turn, error).await? {
            UsageLimitOutcome::Continue(turn) => Ok(ControlFlow::Continue(*turn)),
            UsageLimitOutcome::Response(response) => Ok(ControlFlow::Break(*response)),
        }
    }

    async fn retry_non_streaming_after_context(
        &self,
        mut turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ActiveTurn> {
        let error_text = error.to_string();
        let retry = self.context_retry_or_error(&mut turn, error).await?;
        tracing::warn!(
            error = %error_text,
            thread_id = %turn.session.thread_id,
            "retrying completed turn on a fresh thread after context window exceeded"
        );
        self.retry_after_context_window(retry, &turn.session, turn.input_tokens)
            .await
    }
}

#[path = "context_retry_failover.rs"]
mod failover;

pub(super) enum UsageLimitOutcome {
    Continue(Box<ActiveTurn>),
    Response(Box<Response<Body>>),
}
