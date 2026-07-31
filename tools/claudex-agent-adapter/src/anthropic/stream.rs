use std::{ops::ControlFlow, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::Value;
use tokio::{
    sync::mpsc,
    time::{Instant, sleep},
};

use super::{
    Bridge, MessagesRequest, Segment, Session,
    model_concurrency::Ticket,
    stream_batch::{NextEvent, next_event},
};

mod builder;
mod context_retry;
mod context_window;
mod control;
mod disconnect;
mod drive;
mod internal_tools;
mod prepare;
mod protocol;
mod provider_tool;
mod sanitize;
mod thinking;
mod tool_call_parser;

use builder::SegmentBuilder;
pub(in crate::anthropic) use control::commit_transcript;
use control::refresh_activity_keepalive;
use prepare::{PreparedStream, prepare_with_activity};
use sanitize::is_visible_activity_event;

pub(super) use control::{error_flow, turn_flow};
#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, send_stream_completion, send_stream_error, sse_response};
pub(super) use protocol::{message_start, send_stream_frame, streaming_sse_response};
// Match Claude Code's quieter idle UX: visible status after long silence, while Anthropic `ping` handles short raw-byte watchdogs.
const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_secs(30);
struct ToolCall<'a> {
    call_id: &'a str,
    name: &'a str,
    arguments: &'a Value,
    request_id: Value,
}
struct StreamWaitInput<'a> {
    session: &'a Arc<Session>,
    events: Arc<crate::app_server::ThreadEvents>,
    current_messages: &'a [Value],
    system: &'a Value,
    sender: &'a StreamSender,
    builder: SegmentBuilder,
    activity_interval: Duration,
}
enum StreamWaitResult {
    Event(Value),
    Done(StreamTurn),
    NoEvent,
}
enum StreamEventState {
    Continue,
    Done(StreamTurn),
}

pub(super) enum StreamTurn {
    Segment {
        segment: Segment,
        provider_settled: bool,
    },
    ContextWindow(anyhow::Error),
    Disconnected,
}

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

    async fn drive_prepared_subagent_stream(self: Arc<Self>, prepared: PreparedStream) {
        let PreparedStream {
            request,
            input_tokens,
            effort,
            concurrency_ticket,
            is_subagent,
            run_in_background,
            sender,
        } = prepared;
        let prepare = async {
            let permit = match concurrency_ticket {
                Some(ticket) => Some(ticket.acquire().await?),
                None => None,
            };
            let turn = self.prepare_turn(&request, input_tokens, effort).await?;
            Ok((turn, permit))
        };
        let (turn, mut builder) = prepare_with_activity(
            prepare,
            input_tokens,
            &sender,
            INITIAL_ACTIVITY_DELAY,
            ACTIVITY_KEEPALIVE_INTERVAL,
        )
        .await;
        match turn {
            Ok(Some((turn, permit))) => {
                self.drive_subagent_stream(
                    turn,
                    sender,
                    builder,
                    permit,
                    is_subagent,
                    run_in_background,
                )
                .await
            }
            Ok(None) => {}
            Err(error) => {
                let _ = builder.close_open_blocks(Some(&sender)).await;
                send_stream_error(&sender, error).await;
            }
        }
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
            let wait = self
                .wait_for_stream_event(
                    session,
                    Arc::clone(&events),
                    sender,
                    &mut builder,
                    activity_interval,
                    &mut activity_deadline,
                )
                .await?;
            if let Some(turn) = self
                .resolve_stream_wait(
                    wait,
                    session,
                    sender,
                    current_messages,
                    system,
                    &mut builder,
                )
                .await?
            {
                return Ok(turn);
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
    ) -> Result<Option<StreamTurn>> {
        match wait {
            StreamWaitResult::Done(turn) => Ok(Some(turn)),
            StreamWaitResult::NoEvent => Ok(None),
            StreamWaitResult::Event(event) => {
                match self
                    .consume_stream_event(
                        session,
                        sender,
                        current_messages,
                        system,
                        &event,
                        builder,
                    )
                    .await?
                {
                    StreamEventState::Done(turn) => Ok(Some(turn)),
                    StreamEventState::Continue => Ok(None),
                }
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
                return Ok(StreamWaitResult::Done(self.disconnect_stream(session, events).await));
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
                if is_visible_activity_event(&event) {
                    activity_deadline
                        .as_mut()
                        .reset(Instant::now() + activity_interval);
                }
                Ok(StreamWaitResult::Event(event))
            }
            NextEvent::ExternalBatchReady => {
                let segment = builder.finish(Some(sender)).await?;
                if sender.is_closed() {
                    Ok(StreamWaitResult::Done(
                        self.disconnect_stream(session, events).await,
                    ))
                } else {
                    Ok(StreamWaitResult::Done(StreamTurn::Segment {
                        segment,
                        provider_settled: false,
                    }))
                }
            }
            NextEvent::Closed => bail!("app-server event stream closed"),
        }
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
                return Ok(StreamEventState::Done(StreamTurn::ContextWindow(error)));
            }
            Err(error) => return Err(error),
        };
        if flow == ControlFlow::Break(()) {
            Ok(StreamEventState::Done(StreamTurn::Segment {
                segment: builder.finish(Some(sender)).await?,
                provider_settled: true,
            }))
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

    #[cfg(test)]
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
        Ok(StreamTurn::Segment {
            segment,
            provider_settled: false,
        })
    }

    pub(in crate::anthropic) async fn wait_for_segment(
        &self,
        session: &Session,
        events: &crate::app_server::ThreadEvents,
        input_tokens: u64,
        current_messages: &[Value],
        system: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<Segment> {
        let mut builder = SegmentBuilder::new(input_tokens);
        loop {
            let event = match next_event(events, builder.has_external_tool_calls()).await {
                NextEvent::Event(event) => event,
                NextEvent::ExternalBatchReady => return builder.finish(stream).await,
                NextEvent::Closed => bail!("app-server event stream closed"),
            };
            if builder
                .handle_event(self, session, current_messages, system, &event, stream)
                .await?
                == ControlFlow::Break(())
            {
                return builder.finish(stream).await;
            }
        }
    }
}
#[cfg(test)]
mod tests;
