use std::sync::Arc;

use super::super::super::{
    SegmentBuilder, StreamSender,
    protocol::{send_empty_turn_or_overflow_error, send_stream_graceful_stop_for_error_at},
};
use super::super::ContextRetryStream;
use crate::anthropic::{
    ActiveTurn, Bridge, model_concurrency::ModelPermit,
    usage_limit_failover::streaming_provider_retry,
};

async fn stop_retry_after_message_start(
    sender: &StreamSender,
    next_sse_index: usize,
    error: &anyhow::Error,
) {
    send_stream_graceful_stop_for_error_at(sender, next_sse_index, error).await;
}

fn rewrite_usage_limit_failover(
    bridge: &Bridge,
    exhausted_model: &str,
    is_subagent: bool,
    mut retry: crate::anthropic::ContextRetry,
) -> Option<crate::anthropic::ContextRetry> {
    let failover =
        streaming_provider_retry(bridge.failover_for_stream_turn(exhausted_model, is_subagent))?;
    tracing::warn!(
        exhausted_model,
        failover_model = %failover.model,
        is_subagent,
        "retrying stream on a sibling provider after provider exhaustion"
    );
    retry.request.model = failover.model;
    if let Some(effort) = failover.effort {
        retry.effort = Some(effort);
    }
    Some(retry)
}

impl Bridge {
    pub(in crate::anthropic::stream) async fn retry_provider_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        error: anyhow::Error,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        let input_tokens = turn.input_tokens;
        match self.retry_after_provider_failure(turn, error).await {
            Ok(retried) => {
                let model = retried.session.model.clone();
                Box::pin(self.drive_subagent_stream(
                    retried,
                    sender,
                    SegmentBuilder::for_turn(input_tokens, is_subagent, &model),
                    model_permit,
                    is_subagent,
                    run_in_background,
                ))
                .await;
            }
            Err(retry_error) => {
                drop(model_permit);
                tracing::warn!(?retry_error, "provider retry failed after message_start");
                stop_retry_after_message_start(&sender, 0, &retry_error).await;
            }
        }
    }

    pub(in crate::anthropic::stream) async fn retry_context_stream(
        self: Arc<Self>,
        input: ContextRetryStream,
    ) {
        let ActiveTurn {
            session,
            input_tokens,
            retry,
            gate,
            ..
        } = input.turn;
        drop(gate);
        let Some(retry) = retry else {
            self.remove_session(&session).await;
            tracing::warn!(
                error = ?input.error,
                "context retry unavailable after message_start"
            );
            send_empty_turn_or_overflow_error(&input.sender, input.next_sse_index, &input.error)
                .await;
            return;
        };
        let retried = match self
            .retry_after_context_window(retry, &session, input_tokens)
            .await
        {
            Ok(retried) => retried,
            Err(error) => {
                tracing::warn!(?error, "context window retry failed after message_start");
                let reported = anyhow::anyhow!(
                    "context window retry failed after {}: {error:#}",
                    input.error
                );
                send_empty_turn_or_overflow_error(&input.sender, input.next_sse_index, &reported)
                    .await;
                return;
            }
        };
        Box::pin(self.drive_subagent_stream(
            retried,
            input.sender,
            input.builder,
            input.model_permit,
            input.is_subagent,
            input.run_in_background,
        ))
        .await;
    }

    pub(in crate::anthropic::stream) async fn retry_usage_limit_stream(
        self: Arc<Self>,
        input: ContextRetryStream,
    ) {
        let ActiveTurn {
            session,
            input_tokens,
            retry,
            gate,
            ..
        } = input.turn;
        self.note_provider_exhaustion(&input.error, Some(&session.model));
        drop(gate);
        let exhausted_model = session.model.clone();
        let Some(retry) = retry else {
            self.remove_session(&session).await;
            tracing::warn!(
                error = ?input.error,
                "usage-limit retry unavailable after message_start"
            );
            stop_retry_after_message_start(&input.sender, input.next_sse_index, &input.error).await;
            return;
        };
        let Some(retry) =
            rewrite_usage_limit_failover(&self, &exhausted_model, input.is_subagent, retry)
        else {
            self.remove_session(&session).await;
            tracing::warn!(
                error = ?input.error,
                "usage-limit failover unavailable after message_start"
            );
            stop_retry_after_message_start(&input.sender, input.next_sse_index, &input.error).await;
            return;
        };
        let retried = match self
            .retry_after_context_window(retry, &session, input_tokens)
            .await
        {
            Ok(retried) => retried,
            Err(error) => {
                tracing::warn!(?error, "usage-limit retry failed after message_start");
                stop_retry_after_message_start(&input.sender, input.next_sse_index, &error).await;
                return;
            }
        };
        Box::pin(self.drive_subagent_stream(
            retried,
            input.sender,
            input.builder,
            input.model_permit,
            input.is_subagent,
            input.run_in_background,
        ))
        .await;
    }
}
