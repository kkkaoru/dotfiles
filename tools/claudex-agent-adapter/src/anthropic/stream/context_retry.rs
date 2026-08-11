use std::ops::ControlFlow;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::request_routing::RouteDecision;
use super::super::usage_limit_failover::is_usage_limit_exceeded;
use super::super::{ActiveTurn, Bridge, ContextRetry, content::anthropic_response};
use super::Segment;

impl Bridge {
    pub(in crate::anthropic) async fn retry_after_provider_failure(
        &self,
        mut turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ActiveTurn> {
        if !super::is_provider_stream_closed(&error) || self.app.model_is_alive(&turn.session.model)
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

    async fn finish_non_streaming_turn(&self, turn: ActiveTurn, segment: Segment) -> Response<Body> {
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

    pub(super) async fn failover_usage_limit_turn(
        &self,
        mut turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<UsageLimitOutcome> {
        let exhausted_model = turn.session.model.clone();
        self.note_provider_exhaustion(&error, Some(&exhausted_model));
        let error_text = error.to_string();
        let Some(retry) = turn.retry.take() else {
            self.remove_session(&turn.session).await;
            return Err(error);
        };
        let Some(failover) = self
            .subagent_provider_failover_for(&exhausted_model)
            .or_else(|| self.usage_limit_failover_for(&exhausted_model))
        else {
            self.remove_session(&turn.session).await;
            return Err(error);
        };
        match failover.route {
            RouteDecision::Provider => {
                self.continue_on_sibling_provider(
                    turn,
                    retry,
                    failover,
                    exhausted_model,
                    &error_text,
                )
                .await
            }
            RouteDecision::Subscription => {
                self.failover_completed_to_subscription(
                    turn,
                    retry,
                    failover,
                    exhausted_model,
                    &error_text,
                )
                .await
            }
        }
    }

    async fn continue_on_sibling_provider(
        &self,
        turn: ActiveTurn,
        mut retry: ContextRetry,
        failover: super::super::usage_limit_failover::UsageLimitFailover,
        exhausted_model: String,
        error_text: &str,
    ) -> Result<UsageLimitOutcome> {
        tracing::warn!(
            error = %error_text,
            exhausted_model = %exhausted_model,
            failover_model = %failover.model,
            "retrying completed turn on a sibling provider after provider exhaustion"
        );
        retry.request.model = failover.model;
        if let Some(effort) = failover.effort {
            retry.effort = Some(effort);
        }
        let input_tokens = turn.input_tokens;
        let previous = std::sync::Arc::clone(&turn.session);
        drop(turn);
        Ok(UsageLimitOutcome::Continue(Box::new(
            self.retry_after_context_window(retry, &previous, input_tokens)
                .await?,
        )))
    }

    async fn failover_completed_to_subscription(
        &self,
        turn: ActiveTurn,
        retry: ContextRetry,
        failover: super::super::usage_limit_failover::UsageLimitFailover,
        exhausted_model: String,
        error_text: &str,
    ) -> Result<UsageLimitOutcome> {
        tracing::warn!(
            error = %error_text,
            exhausted_model = %exhausted_model,
            failover_model = %failover.model,
            "failing over completed turn to subscription after usageLimitExceeded"
        );
        let mut request = retry.request;
        request.model = failover.model;
        let effort = failover.effort.or(retry.effort);
        let tools_were_provided = !request.tools.is_empty();
        self.remove_session(&turn.session).await;
        Ok(UsageLimitOutcome::Response(Box::new(
            self.subscription_messages(request, effort, false, tools_were_provided)
                .await?,
        )))
    }
}

pub(super) enum UsageLimitOutcome {
    Continue(Box<ActiveTurn>),
    Response(Box<Response<Body>>),
}
