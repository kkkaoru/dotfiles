use std::sync::Arc;

use super::{
    SegmentBuilder, StreamSender, StreamTurn, commit_transcript, send_stream_completion,
    send_stream_error,
};
use crate::anthropic::{ActiveTurn, Bridge, model_concurrency::ModelPermit};

impl Bridge {
    pub(super) async fn drive_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
    ) {
        let ActiveTurn {
            session,
            events,
            extras,
            routing_system,
            input_tokens,
            retry,
            gate,
            ..
        } = turn;
        let _gate = gate;
        match self
            .wait_for_stream_segment(
                &session,
                &events,
                &extras,
                &routing_system,
                &sender,
                builder,
            )
            .await
        {
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
                        Box::pin(self.drive_stream(
                            retried,
                            sender,
                            SegmentBuilder::new(input_tokens),
                            model_permit,
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
