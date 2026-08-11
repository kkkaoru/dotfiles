use std::{sync::Arc, time::Duration};

use super::super::{SegmentBuilder, StreamSender, StreamTurn, send_stream_error};
use super::{ContextRetryStream, StreamDriveOptions, response_timeout};
use crate::anthropic::{
    ActiveTurn, Bridge, model_concurrency::ModelPermit, subagent_timeout::completes_within,
    usage_limit_failover::streaming_provider_retry,
};

impl Bridge {
    #[cfg(test)]
    pub(in crate::anthropic::stream) async fn drive_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
    ) {
        self.drive_subagent_stream(turn, sender, builder, model_permit, false, false)
            .await;
    }

    pub(in crate::anthropic::stream) async fn drive_subagent_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        let timeout = response_timeout(self.subagent_hard_timeout, is_subagent, run_in_background);
        self.drive_subagent_stream_with_timeout(
            turn,
            sender,
            builder,
            StreamDriveOptions {
                model_permit,
                is_subagent,
                run_in_background,
                timeout,
            },
        )
        .await;
    }

    pub(in crate::anthropic::stream) async fn drive_subagent_stream_with_timeout(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        options: StreamDriveOptions,
    ) {
        let StreamDriveOptions {
            model_permit,
            is_subagent,
            run_in_background,
            timeout,
        } = options;
        let waited = self
            .wait_for_stream_turn(&turn, &sender, builder, timeout)
            .await;
        let Some(waited) = waited else {
            let timeout = timeout.expect("elapsed stream wait has a configured timeout");
            let error = self.expire_subagent_turn(&turn, timeout).await;
            drop(model_permit);
            send_stream_error(&sender, error).await;
            return;
        };
        self.finish_stream_turn(
            turn,
            sender,
            waited,
            model_permit,
            is_subagent,
            run_in_background,
        )
        .await;
    }

    async fn wait_for_stream_turn(
        &self,
        turn: &ActiveTurn,
        sender: &StreamSender,
        builder: SegmentBuilder,
        timeout: Option<Duration>,
    ) -> Option<anyhow::Result<StreamTurn>> {
        let wait = self.wait_for_stream_segment(
            &turn.session,
            Arc::clone(&turn.events),
            &turn.extras,
            &turn.routing_system,
            sender,
            builder,
        );
        completes_within(timeout, wait).await
    }

    async fn finish_stream_turn(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        waited: anyhow::Result<StreamTurn>,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        match waited {
            Ok(outcome) => {
                self.finish_stream_turn_ok(
                    turn,
                    sender,
                    outcome,
                    model_permit,
                    is_subagent,
                    run_in_background,
                )
                .await;
            }
            Err(error) => {
                self.finish_stream_turn_err(
                    turn,
                    sender,
                    error,
                    model_permit,
                    is_subagent,
                    run_in_background,
                )
                .await;
            }
        }
    }

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
                send_stream_error(&sender, retry_error).await;
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
            send_stream_error(&input.sender, input.error).await;
            return;
        };
        let retried = match self
            .retry_after_context_window(retry, &session, input_tokens)
            .await
        {
            Ok(retried) => retried,
            Err(error) => {
                send_stream_error(&input.sender, error).await;
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
        let Some(mut retry) = retry else {
            self.remove_session(&session).await;
            send_stream_error(&input.sender, input.error).await;
            return;
        };
        // SubAgent empty-ACP/billing failures must switch to a sibling Provider
        // (for example Qwen Cloud). Subscription failover cannot continue an
        // already-open SubAgent SSE stream, so the old path returned the empty
        // ACP error to Claude Code and killed the agent.
        let Some(failover) = streaming_provider_retry(
            self.failover_for_stream_turn(&exhausted_model, input.is_subagent),
        ) else {
            self.remove_session(&session).await;
            send_stream_error(&input.sender, input.error).await;
            return;
        };
        tracing::warn!(
            exhausted_model = %exhausted_model,
            failover_model = %failover.model,
            is_subagent = input.is_subagent,
            "retrying stream on a sibling provider after provider exhaustion"
        );
        retry.request.model = failover.model;
        if let Some(effort) = failover.effort {
            retry.effort = Some(effort);
        }
        let retried = match self
            .retry_after_context_window(retry, &session, input_tokens)
            .await
        {
            Ok(retried) => retried,
            Err(error) => {
                send_stream_error(&input.sender, error).await;
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
