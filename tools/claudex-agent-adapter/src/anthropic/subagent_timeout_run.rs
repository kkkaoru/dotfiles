use std::{sync::Arc, time::Duration};

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::{ActiveTurn, Bridge, MessagesRequest, model_concurrency::ModelPermit};
use super::completes_within;
use super::completion;

impl Bridge {
    pub(in crate::anthropic) async fn provider_messages(
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

    pub(in crate::anthropic) async fn non_streaming_subagent_response(
        self: &Arc<Self>,
        turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) -> Result<Response<Body>> {
        self.non_streaming_subagent_response_with_timeout(turn, permit, self.subagent_hard_timeout)
            .await
    }

    // This bounded retry loop retains ActiveTurn until completion or provider cleanup.
    pub(in crate::anthropic) async fn non_streaming_subagent_response_with_timeout(
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
    ) -> Result<Result<super::super::Segment, anyhow::Error>> {
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

    pub(in crate::anthropic) fn track_active_subagent(
        &self,
        is_subagent: bool,
        request: &super::super::MessagesRequest,
    ) -> Option<super::super::active_subagent_models::ActiveSubagentGuard> {
        if !is_subagent {
            return None;
        }
        let agent_id = super::super::request_identity::request_agent_id(request);
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
