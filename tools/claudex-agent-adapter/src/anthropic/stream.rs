use std::{ops::ControlFlow, sync::Arc};

use anyhow::Result;
use axum::{body::Body, http::Response};
use serde_json::Value;
use tokio::{sync::mpsc, time::sleep};

use super::{
    Bridge, MessagesRequest, Segment, Session,
    model_concurrency::Ticket,
};

mod acp_launch_queue;
pub(in crate::anthropic) mod acp_tool_bridge;
mod builder;
mod context_retry;
mod context_window;
mod control;
mod disconnect;
mod drive;
mod drive_finish;
mod non_stream;
mod prepare;
mod protocol;
mod provider_tool;
mod sanitize;
#[cfg(test)]
mod subagent_live_view;
#[cfg(test)]
mod subagent_progress_models_tests;
mod thinking;
mod tool_call_parser;
mod turn;
mod types;
mod wait_event;
pub(super) mod usage_limit;
pub(in crate::anthropic) use turn::StreamTurn;
use types::{StreamEventState, StreamWaitResult, stream_activity_delays};
pub(super) use types::{
    ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
    StreamWaitInput, ToolCall, is_provider_stream_closed,
};

use builder::SegmentBuilder;
pub(in crate::anthropic) use control::commit_transcript;
#[cfg(test)]
use prepare::{PrepareActivityOptions, prepare_with_activity};
use prepare::{PreparedStream, prime_subagent_sse};

pub(super) use control::{error_flow, turn_flow};
pub(super) use crate::anthropic::stream_batch::{NextEvent, next_event};
#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, send_stream_error, sse_response};
pub(super) use protocol::{
    message_start, send_stream_completion, send_stream_frame, streaming_sse_response,
};

impl Bridge {
    pub(super) fn streaming_messages(
        self: &Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        concurrency_ticket: Option<Ticket>,
        is_subagent: bool,
        run_in_background: bool,
    ) -> Response<Body> {
        let (sender, receiver) = mpsc::channel(256);
        let response_model = self.request_model(&request);
        let primed_thinking = prime_subagent_sse(
            &sender,
            &response_model,
            input_tokens,
            is_subagent,
            effort.as_deref(),
        );
        tokio::spawn(
            Arc::clone(self).drive_prepared_subagent_stream(PreparedStream {
                request,
                input_tokens,
                effort,
                concurrency_ticket,
                is_subagent,
                run_in_background,
                sender,
                primed_thinking,
            }),
        );
        sse_response(receiver)
    }

    async fn wait_for_stream_segment(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        current_messages: &[Value],
        system: &Value,
        sender: &StreamSender,
        builder: SegmentBuilder,
    ) -> Result<StreamTurn> {
        let (initial_activity_delay, activity_interval) =
            stream_activity_delays(builder.is_subagent);
        self.wait_for_stream_segment_with_interval(StreamWaitInput {
            session,
            events,
            current_messages,
            system,
            sender,
            builder,
            activity_interval,
            initial_activity_delay,
        })
        .await
    }

    // This event loop owns the live SegmentBuilder across keepalive, completion,
    // disconnect, and context-window transitions.
    async fn wait_for_stream_segment_with_interval(
        &self,
        input: StreamWaitInput<'_>,
    ) -> Result<StreamTurn> {
        let StreamWaitInput {
            session,
            events,
            current_messages,
            system,
            sender,
            mut builder,
            activity_interval,
            initial_activity_delay,
        } = input;
        // Emit keepalive content during silence to avoid timeout while preserving
        // visible progress semantics during active output.
        let mut activity_deadline = Box::pin(sleep(initial_activity_delay));
        let mut sse = Some(sender);
        loop {
            let wait = match self
                .take_stream_wait(
                    session,
                    Arc::clone(&events),
                    &mut sse,
                    &mut builder,
                    activity_interval,
                    &mut activity_deadline,
                )
                .await?
            {
                ControlFlow::Break(turn) => return Ok(turn),
                ControlFlow::Continue(wait) => wait,
            };
            let state = self
                .resolve_stream_wait(
                    wait,
                    session,
                    sender,
                    current_messages,
                    system,
                    &mut builder,
                )
                .await?;
            match finish_stream_event_state(state, builder) {
                ControlFlow::Break(turn) => return Ok(turn),
                ControlFlow::Continue(next) => builder = next,
            }
        }
    }

    async fn take_stream_wait(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        sse: &mut Option<&StreamSender>,
        builder: &mut SegmentBuilder,
        activity_interval: std::time::Duration,
        activity_deadline: &mut std::pin::Pin<Box<tokio::time::Sleep>>,
    ) -> Result<ControlFlow<StreamTurn, StreamWaitResult>> {
        match self
            .wait_for_stream_event(
                session,
                events,
                sse,
                builder,
                activity_interval,
                activity_deadline,
            )
            .await
        {
            Ok(wait) => Ok(ControlFlow::Continue(wait)),
            Err(error) if stream_provider_failure(&error, self, session, builder) => {
                Ok(ControlFlow::Break(StreamTurn::ProviderFailure { error }))
            }
            Err(error) => Err(error),
        }
    }

    async fn resolve_stream_wait(
        &self,
        wait: StreamWaitResult,
        session: &Arc<Session>,
        sender: &StreamSender,
        current_messages: &[Value],
        system: &Value,
        builder: &mut SegmentBuilder,
    ) -> Result<StreamEventState> {
        match wait {
            StreamWaitResult::Done(turn) => Ok(StreamEventState::Done(turn)),
            StreamWaitResult::NoEvent => Ok(StreamEventState::Continue),
            StreamWaitResult::Event(event) => {
                self.consume_stream_event(
                    session,
                    sender,
                    current_messages,
                    system,
                    &event,
                    builder,
                )
                .await
            }
        }
    }

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
            let _ = self.app.cancel_turn(&session.thread_id).await;
        }
        Ok(StreamTurn::Segment {
            segment,
            provider_settled: false,
        })
    }

    async fn consume_stream_event(
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
                    || super::provider_auth::is_auth_failure_event(event))
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

    async fn finish_completed_stream(
        &self,
        turn: crate::anthropic::ActiveTurn,
        sender: &StreamSender,
        segment: Segment,
        provider_settled: bool,
        is_subagent: bool,
    ) {
        if sender.is_closed() && is_subagent {
            commit_transcript(&turn.session, turn.extras, &segment).await;
            // Settled turns stay registered so follow-up `resume` can reuse the
            // provider thread (prompt-cache). Unsettled closes still tear down.
            self.finish_closed_stream(
                &turn.session,
                &turn.events,
                provider_settled,
            )
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

    async fn finish_if_stream_closed(
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

fn stream_provider_failure(
    error: &anyhow::Error,
    bridge: &Bridge,
    session: &Session,
    builder: &SegmentBuilder,
) -> bool {
    is_provider_stream_closed(error)
        && !bridge.app.model_is_alive(&session.model)
        && !builder.has_committed_output()
}

fn finish_stream_event_state(
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

#[cfg(test)]
mod tests;
