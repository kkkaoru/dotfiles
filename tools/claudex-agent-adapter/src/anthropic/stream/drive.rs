use std::sync::Arc;

use super::{
    SegmentBuilder, StreamSender, StreamTurn, commit_transcript, send_stream_completion,
    send_stream_error,
};
use crate::anthropic::{
    ActiveTurn, Bridge,
    model_concurrency::ModelPermit,
    subagent_timeout::{BACKGROUND_NOTICE, completes_within, subagent_response_timeout},
};

impl Bridge {
    #[cfg(test)]
    pub(super) async fn drive_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
    ) {
        self.drive_subagent_stream(turn, sender, builder, model_permit, false)
            .await;
    }

    pub(super) async fn drive_subagent_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
    ) {
        let input_tokens = turn.input_tokens;
        let waited = if is_subagent {
            completes_within(
                subagent_response_timeout(),
                self.wait_for_stream_segment(
                    &turn.session,
                    Arc::clone(&turn.events),
                    &turn.extras,
                    &turn.routing_system,
                    &sender,
                    builder,
                ),
            )
            .await
        } else {
            Some(
                self.wait_for_stream_segment(
                    &turn.session,
                    Arc::clone(&turn.events),
                    &turn.extras,
                    &turn.routing_system,
                    &sender,
                    builder,
                )
                .await,
            )
        };
        let Some(waited) = waited else {
            self.continue_subagent_in_background(turn, model_permit);
            let mut notice = SegmentBuilder::new(input_tokens);
            if notice
                .start_text_block(BACKGROUND_NOTICE, Some(&sender))
                .await
                .is_ok()
                && let Ok(segment) = notice.finish(Some(&sender)).await
            {
                send_stream_completion(&sender, &segment).await;
            }
            return;
        };
        let ActiveTurn {
            session,
            events,
            extras,
            routing_system: _,
            input_tokens,
            retry,
            gate,
            ..
        } = turn;
        let _gate = gate;
        match waited {
            Ok(StreamTurn::Segment {
                segment,
                provider_settled,
            }) => {
                if self
                    .finish_if_stream_closed(&sender, &session, &events, provider_settled)
                    .await
                {
                    return;
                }
                commit_transcript(&session, extras, &segment).await;
                send_stream_completion(&sender, &segment).await;
                self.finish_if_stream_closed(&sender, &session, &events, provider_settled)
                    .await;
            }
            Ok(StreamTurn::ContextWindow(error)) => {
                drop(_gate);
                let Some(retry) = retry else {
                    self.remove_session(&session).await;
                    send_stream_error(&sender, error).await;
                    return;
                };
                match self
                    .retry_after_context_window(retry, &session, input_tokens)
                    .await
                {
                    Ok(retried) => {
                        Box::pin(self.drive_subagent_stream(
                            retried,
                            sender,
                            SegmentBuilder::new(input_tokens),
                            model_permit,
                            is_subagent,
                        ))
                        .await;
                    }
                    Err(retry_error) => send_stream_error(&sender, retry_error).await,
                }
            }
            Ok(StreamTurn::Disconnected) => {}
            Err(error) => {
                tracing::warn!(?error, "streaming turn failed before message_stop");
                self.remove_session(&session).await;
                send_stream_error(&sender, error).await;
            }
        }
    }
}
