use std::sync::Arc;

use super::{
    SegmentBuilder, StreamSender, StreamTurn, drive::ContextRetryStream,
    empty_turn::EmptyAssistantStream, protocol::send_stream_graceful_stop_for_error,
};
use crate::anthropic::{
    ActiveTurn, Bridge, model_concurrency::ModelPermit,
    usage_limit_failover::is_usage_limit_exceeded,
};

fn rewrite_subagent_segment(
    segment: crate::anthropic::segment::Segment,
    is_subagent: bool,
) -> crate::anthropic::segment::Segment {
    match is_subagent {
        true => super::sanitize::rewrite_premature_status_only_segment(segment),
        false => segment,
    }
}

impl Bridge {
    pub(super) async fn finish_stream_turn_ok(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        outcome: StreamTurn,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        match outcome {
            StreamTurn::Segment {
                segment,
                provider_settled,
            } if segment.is_empty_end_turn() => {
                let _ = provider_settled;
                self.finish_empty_assistant_stream(EmptyAssistantStream {
                    turn,
                    sender,
                    segment,
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await;
            }
            StreamTurn::Segment {
                segment,
                provider_settled,
            } => {
                self.finish_completed_stream(
                    turn,
                    &sender,
                    rewrite_subagent_segment(segment, is_subagent),
                    provider_settled,
                    is_subagent,
                )
                .await
            }
            StreamTurn::ContextWindow { error, builder } => {
                self.retry_context_stream(ContextRetryStream {
                    turn,
                    sender,
                    error,
                    next_sse_index: builder.next_sse_index(),
                    builder,
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await
            }
            StreamTurn::UsageLimit { error, builder } => {
                self.retry_usage_limit_stream(ContextRetryStream {
                    turn,
                    sender,
                    error,
                    next_sse_index: builder.next_sse_index(),
                    builder,
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await
            }
            StreamTurn::ProviderFailure { error } => {
                self.note_provider_exhaustion(&error, Some(&turn.session.model));
                self.retry_provider_stream(
                    turn,
                    sender,
                    error,
                    model_permit,
                    is_subagent,
                    run_in_background,
                )
                .await;
            }
            StreamTurn::Disconnected => {}
        }
    }

    pub(super) async fn finish_stream_turn_err(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        error: anyhow::Error,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        if is_usage_limit_exceeded(&error) {
            let input_tokens = turn.input_tokens;
            let model = turn.session.model.clone();
            self.retry_usage_limit_stream(ContextRetryStream {
                turn,
                sender,
                error,
                builder: SegmentBuilder::for_turn(input_tokens, is_subagent, &model),
                model_permit,
                is_subagent,
                run_in_background,
                next_sse_index: 0,
            })
            .await;
            return;
        }
        tracing::warn!(?error, "streaming turn failed before message_stop");
        self.note_provider_exhaustion(&error, Some(&turn.session.model));
        let _ = if run_in_background {
            self.disconnect_stream_for_async_handoff(&turn.session, Arc::clone(&turn.events))
                .await
        } else {
            self.disconnect_stream(&turn.session, Arc::clone(&turn.events))
                .await
        };
        send_stream_graceful_stop_for_error(&sender, &error).await;
    }
}
