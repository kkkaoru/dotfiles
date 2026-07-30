use std::{sync::Arc, time::Duration};

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
        let timeout = response_timeout(is_subagent, run_in_background);
        self.drive_subagent_stream_with_timeout(
            turn,
            sender,
            builder,
            model_permit,
            is_subagent,
            timeout,
        )
        .await;
    }

    pub(super) async fn drive_subagent_stream_with_timeout(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        timeout: Option<Duration>,
    ) {
        let input_tokens = turn.input_tokens;
        let waited = self
            .wait_for_stream_turn(&turn, &sender, builder, timeout)
            .await;
        let Some(waited) = waited else {
            self.continue_subagent_in_background(turn, model_permit);
            self.send_background_notice(input_tokens, &sender).await;
            return;
        };
        self.finish_stream_turn(
            turn,
            sender,
            waited,
            model_permit,
            is_subagent,
            timeout.is_some(),
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
        match timeout {
            Some(timeout) => completes_within(timeout, wait).await,
            None => Some(wait.await),
        }
    }

    async fn send_background_notice(&self, input_tokens: u64, sender: &StreamSender) {
        let mut notice = SegmentBuilder::new(input_tokens);
        if notice
            .text_delta(
                &serde_json::json!({"params":{"delta":BACKGROUND_NOTICE}}),
                Some(sender),
            )
            .await
            .is_ok()
            && let Ok(segment) = notice.finish(Some(sender)).await
        {
            send_stream_completion(sender, &segment).await;
        }
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
                            run_in_background,
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

fn response_timeout(is_subagent: bool, run_in_background: bool) -> Option<Duration> {
    (is_subagent && run_in_background).then(subagent_response_timeout)
}

#[cfg(test)]
mod tests {
    use super::response_timeout;

    #[test]
    fn only_background_subagents_get_a_response_timeout() {
        assert!(response_timeout(false, false).is_none());
        assert!(response_timeout(false, true).is_none());
        assert!(response_timeout(true, false).is_none());
        assert!(response_timeout(true, true).is_some());
    }
}
