use std::sync::Arc;

use super::{
    SegmentBuilder, StreamSender, drive::ContextRetryStream, protocol::send_stream_graceful_stop_at,
};

use crate::anthropic::{
    ActiveTurn, Bridge, Segment,
    provider_kind::is_cline_model,
    segment::{
        EMPTY_ACP_END_TURN, EMPTY_ASSISTANT_TURN, empty_assistant_retry_user_message,
        messages_already_retried_empty_assistant,
    },
};

pub(super) struct EmptyAssistantStream {
    pub(super) turn: ActiveTurn,
    pub(super) sender: StreamSender,
    pub(super) segment: Segment,
    pub(super) model_permit: Option<crate::anthropic::model_concurrency::ModelPermit>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
}

enum EmptyAssistantPlan {
    ClineFailover,
    SameProviderRetry,
    VisibleFailure,
}

fn empty_assistant_plan(turn: &ActiveTurn) -> EmptyAssistantPlan {
    if is_cline_model(&turn.session.model) {
        return EmptyAssistantPlan::ClineFailover;
    }
    let Some(retry) = turn.retry.as_ref() else {
        return EmptyAssistantPlan::VisibleFailure;
    };
    if messages_already_retried_empty_assistant(&retry.request.messages) {
        return EmptyAssistantPlan::VisibleFailure;
    }
    EmptyAssistantPlan::SameProviderRetry
}

impl Bridge {
    pub(super) async fn finish_empty_assistant_stream(
        self: Arc<Self>,
        input: EmptyAssistantStream,
    ) {
        match empty_assistant_plan(&input.turn) {
            EmptyAssistantPlan::ClineFailover => self.failover_empty_cline_stream(input).await,
            EmptyAssistantPlan::SameProviderRetry => {
                self.retry_empty_assistant_once(input).await;
            }
            EmptyAssistantPlan::VisibleFailure => self.fail_empty_assistant_error(input).await,
        }
    }

    async fn failover_empty_cline_stream(self: Arc<Self>, input: EmptyAssistantStream) {
        let EmptyAssistantStream {
            turn,
            sender,
            segment,
            model_permit,
            is_subagent,
            run_in_background,
        } = input;
        let input_tokens = turn.input_tokens;
        let model = turn.session.model.clone();
        self.retry_usage_limit_stream(ContextRetryStream {
            turn,
            sender,
            error: anyhow::anyhow!("{EMPTY_ACP_END_TURN}"),
            builder: SegmentBuilder::for_turn(input_tokens, is_subagent, &model)
                .with_reserved_sse_slots(segment.next_sse_index),
            model_permit,
            is_subagent,
            run_in_background,
            next_sse_index: segment.next_sse_index,
        })
        .await;
    }

    async fn retry_empty_assistant_once(self: Arc<Self>, input: EmptyAssistantStream) {
        let EmptyAssistantStream {
            mut turn,
            sender,
            segment,
            model_permit,
            is_subagent,
            run_in_background,
        } = input;
        let input_tokens = turn.input_tokens;
        let model = turn.session.model.clone();
        let Some(mut retry) = turn.retry.take() else {
            self.fail_empty_assistant_error(EmptyAssistantStream {
                turn,
                sender,
                segment,
                model_permit,
                is_subagent,
                run_in_background,
            })
            .await;
            return;
        };
        retry
            .request
            .messages
            .push(empty_assistant_retry_user_message());
        tracing::warn!(
            model = %model,
            "retrying empty assistant turn once with a visible provider error"
        );
        let retried = match self
            .retry_after_context_window(retry, &turn.session, input_tokens)
            .await
        {
            Ok(retried) => retried,
            Err(error) => {
                tracing::warn!(?error, "empty assistant retry failed after message_start");
                self.fail_empty_assistant_error(EmptyAssistantStream {
                    turn,
                    sender,
                    segment,
                    model_permit,
                    is_subagent,
                    run_in_background,
                })
                .await;
                return;
            }
        };
        drop(turn);
        Box::pin(
            self.drive_subagent_stream(
                retried,
                sender,
                SegmentBuilder::for_turn(input_tokens, is_subagent, &model)
                    .with_reserved_sse_slots(segment.next_sse_index),
                model_permit,
                is_subagent,
                run_in_background,
            ),
        )
        .await;
    }

    async fn fail_empty_assistant_error(&self, input: EmptyAssistantStream) {
        let EmptyAssistantStream {
            turn,
            sender,
            segment,
            model_permit,
            ..
        } = input;
        drop(model_permit);
        tracing::warn!(
            model = %turn.session.model,
            next_sse_index = segment.next_sse_index,
            "provider completed with no assistant content"
        );
        self.remove_session(&turn.session).await;
        send_stream_graceful_stop_at(&sender, segment.next_sse_index, EMPTY_ASSISTANT_TURN).await;
    }
}
