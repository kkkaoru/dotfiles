use std::{ops::ControlFlow, sync::Arc};

use anyhow::Result;
use serde_json::Value;

use super::{
    Bridge, Segment, SegmentBuilder, Session, StreamEventState, StreamSender, StreamTurn,
    acp_tool_bridge, commit_transcript, context_window, is_provider_stream_closed,
    send_stream_completion, usage_limit,
};
use crate::anthropic::ActiveTurn;

impl Bridge {
    pub(super) async fn external_batch_segment(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        builder: &mut SegmentBuilder,
        sender: Option<&StreamSender>,
    ) -> Result<StreamTurn> {
        let is_subagent = builder.is_subagent;
        let segment = builder.finish(sender).await?;
        let sse_open = sender.is_some_and(|sender| !sender.is_closed());
        if !sse_open && is_subagent {
            return Ok(StreamTurn::Segment {
                segment,
                provider_settled: false,
            });
        }
        if !sse_open {
            return Ok(self.disconnect_stream(session, events).await);
        }
        // ACP-bridged Agent/spawn: cancel provider so Grok does not also native-spawn.
        let bridge = session
            .pending_tools
            .lock()
            .await
            .values()
            .any(acp_tool_bridge::is_acp_bridge_request_id);
        if segment.stop_reason == "tool_use" && bridge {
            let _ = self
                .app_for_session(session)
                .cancel_turn(&session.thread_id)
                .await;
        }
        Ok(StreamTurn::Segment {
            segment,
            provider_settled: false,
        })
    }

    pub(super) async fn consume_stream_event(
        &self,
        session: &Arc<Session>,
        sender: &StreamSender,
        current_messages: &[Value],
        system: &Value,
        event: &Value,
        builder: &mut SegmentBuilder,
    ) -> Result<StreamEventState> {
        let flow = match builder
            .handle_event(self, session, current_messages, system, event, Some(sender))
            .await
        {
            Ok(flow) => flow,
            Err(error)
                if context_window::is_context_window_event(event)
                    && !builder.has_committed_output() =>
            {
                builder.close_open_blocks(Some(sender)).await?;
                return Ok(StreamEventState::ContextWindow(error));
            }
            Err(error)
                if (usage_limit::is_usage_limit_event(event)
                    || super::super::provider_auth::is_auth_failure_event(event))
                    && !builder.has_committed_output() =>
            {
                builder.close_open_blocks(Some(sender)).await?;
                return Ok(StreamEventState::UsageLimit(error));
            }
            Err(error) => return Err(error),
        };
        if flow == ControlFlow::Break(()) {
            Ok(StreamEventState::Done(Box::new(StreamTurn::Segment {
                segment: builder.finish(Some(sender)).await?,
                provider_settled: true,
            })))
        } else {
            Ok(StreamEventState::Continue)
        }
    }

    pub(super) async fn finish_completed_stream(
        &self,
        turn: ActiveTurn,
        sender: &StreamSender,
        segment: Segment,
        provider_settled: bool,
        is_subagent: bool,
    ) {
        if sender.is_closed() && is_subagent {
            commit_transcript(&turn.session, turn.extras, &segment).await;
            // Settled end_turn and tool_use handoffs keep the idle provider
            // thread so Task resume / tool_result can reuse prompt-cache.
            // Other unsettled closes still tear down.
            let retain = retain_closed_subagent_session(provider_settled, segment.stop_reason);
            self.finish_closed_stream(&turn.session, &turn.events, retain)
                .await;
            return;
        }
        if self
            .finish_if_stream_closed(sender, &turn.session, &turn.events, provider_settled)
            .await
        {
            return;
        }
        commit_transcript(&turn.session, turn.extras, &segment).await;
        send_stream_completion(sender, &segment).await;
        self.finish_if_stream_closed(sender, &turn.session, &turn.events, provider_settled)
            .await;
    }

    pub(super) async fn finish_if_stream_closed(
        &self,
        sender: &StreamSender,
        session: &Arc<Session>,
        events: &Arc<crate::app_server::ThreadEvents>,
        provider_settled: bool,
    ) -> bool {
        if !sender.is_closed() {
            return false;
        }
        self.finish_closed_stream(session, events, provider_settled)
            .await;
        true
    }
}

pub(super) fn retain_closed_subagent_session(provider_settled: bool, stop_reason: &str) -> bool {
    provider_settled || stop_reason == "tool_use"
}

pub(super) fn stream_provider_failure(
    error: &anyhow::Error,
    bridge: &Bridge,
    session: &Session,
    builder: &SegmentBuilder,
) -> bool {
    is_provider_stream_closed(error)
        && !bridge.app.model_is_alive(&session.model)
        && !builder.has_committed_output()
}

pub(super) fn finish_stream_event_state(
    state: StreamEventState,
    builder: SegmentBuilder,
) -> ControlFlow<StreamTurn, SegmentBuilder> {
    match state {
        StreamEventState::Continue => ControlFlow::Continue(builder),
        StreamEventState::Done(turn) => ControlFlow::Break(*turn),
        StreamEventState::ContextWindow(error) => {
            ControlFlow::Break(StreamTurn::ContextWindow { error, builder })
        }
        StreamEventState::UsageLimit(error) => {
            ControlFlow::Break(StreamTurn::UsageLimit { error, builder })
        }
    }
}
