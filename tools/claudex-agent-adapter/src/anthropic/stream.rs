use std::{ops::ControlFlow, sync::Arc};

use anyhow::Result;
use axum::{body::Body, http::Response};
use serde_json::Value;
use tokio::{sync::mpsc, time::sleep};

use super::{Bridge, MessagesRequest, Segment, Session, model_concurrency::Ticket};

mod acp_launch_queue;
pub(in crate::anthropic) mod acp_tool_bridge;
mod builder;
mod context_retry;
mod context_window;
mod control;
mod disconnect;
mod drive;
mod drive_finish;
mod event_consume;
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
mod thinking_support;
mod tool_call_parser;
mod turn;
mod types;
pub(super) mod usage_limit;
mod wait_event;
pub(in crate::anthropic) use turn::StreamTurn;
pub(super) use types::{
    ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
    StreamWaitInput, ToolCall, is_provider_stream_closed,
};
use types::{StreamEventState, StreamWaitResult, stream_activity_delays};

use builder::SegmentBuilder;
pub(in crate::anthropic) use control::commit_transcript;
use event_consume::{finish_stream_event_state, stream_provider_failure};
#[cfg(test)]
use prepare::{PrepareActivityOptions, prepare_with_activity};
use prepare::{PreparedStream, prime_subagent_sse};

pub(super) use crate::anthropic::stream_batch::{NextEvent, next_event};
pub(super) use control::{error_flow, turn_flow};
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
}

#[cfg(test)]
mod tests;
