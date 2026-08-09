use std::{sync::Arc, time::Duration};

use anyhow::anyhow;

use super::{SegmentBuilder, StreamSender, StreamTurn, send_stream_error};
use crate::anthropic::{
    ActiveTurn, Bridge,
    model_concurrency::ModelPermit,
    segment::EMPTY_ACP_END_TURN,
    subagent_timeout::completes_within,
    usage_limit_failover::{is_usage_limit_exceeded, streaming_provider_retry},
};

struct ContextRetryStream {
    turn: ActiveTurn,
    sender: StreamSender,
    error: anyhow::Error,
    builder: SegmentBuilder,
    model_permit: Option<ModelPermit>,
    is_subagent: bool,
    run_in_background: bool,
}

pub(super) struct StreamDriveOptions {
    pub(super) model_permit: Option<ModelPermit>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
    pub(super) timeout: Option<Duration>,
}

impl Bridge {
    #[cfg(test)]
    pub(super) async fn drive_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
    ) {
        self.drive_subagent_stream(turn, sender, builder, model_permit, false, false)
            .await;
    }

    pub(super) async fn drive_subagent_stream(
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

    pub(super) async fn drive_subagent_stream_with_timeout(
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
            Ok(StreamTurn::Segment {
                segment,
                provider_settled,
            }) if is_subagent && segment.is_empty_end_turn() => {
                // Cline Credits $0 (and similar) finishes as empty end_turn; route through
                // usage-limit failover instead of returning a blank assistant message.
                let input_tokens = turn.input_tokens;
                let model = turn.session.model.clone();
                self.retry_usage_limit_stream(ContextRetryStream {
                    turn,
                    sender,
                    error: anyhow!("{EMPTY_ACP_END_TURN}"),
                    builder: SegmentBuilder::for_turn(input_tokens, is_subagent, &model),
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await;
                let _ = provider_settled;
            }
            Ok(StreamTurn::Segment {
                segment,
                provider_settled,
            }) => {
                self.finish_completed_stream(turn, &sender, segment, provider_settled, is_subagent)
                    .await
            }
            Ok(StreamTurn::ContextWindow { error, builder }) => {
                self.retry_context_stream(ContextRetryStream {
                    turn,
                    sender,
                    error,
                    builder,
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await
            }
            Ok(StreamTurn::UsageLimit { error, builder }) => {
                self.retry_usage_limit_stream(ContextRetryStream {
                    turn,
                    sender,
                    error,
                    builder,
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await
            }
            Ok(StreamTurn::ProviderFailure { error }) => {
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
            Ok(StreamTurn::Disconnected) => {}
            Err(error) if is_usage_limit_exceeded(&error) => {
                // Qwen token-plan (and similar ACP quota) often fails the wait
                // before message_stop instead of emitting a usage-limit event.
                // Treat it like empty-ACP so the current SubAgent retries a
                // sibling instead of 502-ing every parallel launch onto Qwen.
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
                })
                .await;
            }
            Err(error) => {
                tracing::warn!(?error, "streaming turn failed before message_stop");
                self.note_provider_exhaustion(&error, Some(&turn.session.model));
                self.remove_session(&turn.session).await;
                send_stream_error(&sender, error).await;
            }
        }
    }

    async fn retry_provider_stream(
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

    async fn retry_context_stream(self: Arc<Self>, input: ContextRetryStream) {
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

    async fn retry_usage_limit_stream(self: Arc<Self>, input: ContextRetryStream) {
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

fn response_timeout(
    configured: Option<Duration>,
    is_subagent: bool,
    run_in_background: bool,
) -> Option<Duration> {
    (is_subagent && run_in_background)
        .then_some(configured)
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::response_timeout;

    #[test]
    fn only_background_subagents_get_an_explicit_hard_timeout() {
        let configured = Some(Duration::from_secs(7));
        assert_eq!(response_timeout(configured, false, false), None);
        assert_eq!(response_timeout(configured, false, true), None);
        assert_eq!(response_timeout(configured, true, false), None);
        assert_eq!(response_timeout(configured, true, true), configured);
        assert_eq!(response_timeout(None, true, true), None);
    }
}
