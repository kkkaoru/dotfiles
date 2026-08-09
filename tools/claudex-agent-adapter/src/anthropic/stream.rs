use std::{ops::ControlFlow, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::Value;
use tokio::{sync::mpsc, time::sleep};

use super::{
    Bridge, MessagesRequest, Segment, Session,
    model_concurrency::Ticket,
    stream_batch::{NextEvent, next_event},
};

mod acp_launch_queue;
pub(in crate::anthropic) mod acp_tool_bridge;
mod builder;
mod context_retry;
mod context_window;
mod control;
mod disconnect;
mod drive;
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
pub(super) mod usage_limit;
pub(in crate::anthropic) use turn::StreamTurn;
use types::{StreamEventState, StreamWaitResult, reset_activity_deadline};
pub(super) use types::{StreamWaitInput, ToolCall, is_provider_stream_closed};

use builder::SegmentBuilder;
pub(in crate::anthropic) use control::commit_transcript;
use control::refresh_activity_keepalive;
use prepare::PreparedStream;
#[cfg(test)]
use prepare::prepare_with_activity;

pub(super) use control::{error_flow, turn_flow};
#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, send_stream_error, sse_response};
pub(super) use protocol::{
    message_start, send_stream_completion, send_stream_frame, streaming_sse_response,
};
pub(super) const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_secs(30);

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
        sender
            .try_send(Ok(Bytes::from(message_start(
                &response_model,
                input_tokens,
            ))))
            .expect("new streaming response channel has capacity");
        tokio::spawn(
            Arc::clone(self).drive_prepared_subagent_stream(PreparedStream {
                request,
                input_tokens,
                effort,
                concurrency_ticket,
                is_subagent,
                run_in_background,
                sender,
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
        self.wait_for_stream_segment_with_interval(StreamWaitInput {
            session,
            events,
            current_messages,
            system,
            sender,
            builder,
            activity_interval: ACTIVITY_KEEPALIVE_INTERVAL,
        })
        .await
    }

    // This event loop owns the live SegmentBuilder across keepalive, completion,
    // disconnect, and context-window transitions.
    #[allow(clippy::excessive_nesting)]
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
        } = input;
        // Emit keepalive content during silence to avoid timeout while preserving
        // visible progress semantics during active output.
        let mut activity_deadline = Box::pin(sleep(activity_interval));
        loop {
            let wait = match self
                .wait_for_stream_event(
                    session,
                    Arc::clone(&events),
                    sender,
                    &mut builder,
                    activity_interval,
                    &mut activity_deadline,
                )
                .await
            {
                Ok(wait) => wait,
                Err(error)
                    if is_provider_stream_closed(&error)
                        && !self.app.model_is_alive(&session.model)
                        && !builder.has_committed_output() =>
                {
                    return Ok(StreamTurn::ProviderFailure { error });
                }
                Err(error) => return Err(error),
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
            match state {
                StreamEventState::Continue => continue,
                StreamEventState::Done(turn) => return Ok(*turn),
                StreamEventState::ContextWindow(error) => {
                    return Ok(StreamTurn::ContextWindow { error, builder });
                }
                StreamEventState::UsageLimit(error) => {
                    return Ok(StreamTurn::UsageLimit { error, builder });
                }
            }
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

    async fn wait_for_stream_event(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        sender: &StreamSender,
        builder: &mut SegmentBuilder,
        activity_interval: Duration,
        activity_deadline: &mut std::pin::Pin<Box<tokio::time::Sleep>>,
    ) -> Result<StreamWaitResult> {
        let next = tokio::select! {
            biased;
            () = sender.closed() => {
                return Ok(StreamWaitResult::Done(Box::new(
                    self.disconnect_stream(session, events).await,
                )));
            }
            next = next_event(&events, builder.has_external_tool_calls()) => next,
            () = &mut *activity_deadline => {
                refresh_activity_keepalive(
                    builder,
                    sender,
                    activity_deadline.as_mut(),
                    activity_interval,
                )
                .await?;
                return Ok(StreamWaitResult::NoEvent);
            }
        };
        match next {
            NextEvent::Event(event) => {
                reset_activity_deadline(&event, activity_deadline, activity_interval);
                Ok(StreamWaitResult::Event(event))
            }
            NextEvent::ExternalBatchReady => Ok(StreamWaitResult::Done(Box::new(
                self.external_batch_segment(session, events, builder, sender)
                    .await?,
            ))),
            NextEvent::Closed => bail!("app-server event stream closed"),
        }
    }

    async fn external_batch_segment(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        builder: &mut SegmentBuilder,
        sender: &StreamSender,
    ) -> Result<StreamTurn> {
        let segment = builder.finish(Some(sender)).await?;
        if sender.is_closed() {
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

#[cfg(test)]
mod tests;
