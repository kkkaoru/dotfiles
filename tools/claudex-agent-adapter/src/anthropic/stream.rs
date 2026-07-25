use std::{ops::ControlFlow, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::{Value, json};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep},
};

use super::{
    ActiveTurn, Bridge, MessagesRequest, Segment, Session,
    content::anthropic_response,
    stream_batch::{NextEvent, next_event},
    subscription::{SubscriptionOptions, run_subscription_model, subscription_prompt},
};

mod builder;
mod disconnect;
mod prepare;
mod protocol;
mod provider_tool;
mod thinking;

use builder::SegmentBuilder;
use prepare::prepare_with_activity;

#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, send_stream_completion, send_stream_error, sse_response};
pub(super) use protocol::{message_start, send_stream_frame, streaming_sse_response};

const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_secs(2);

struct ToolCall<'a> {
    call_id: &'a str,
    name: &'a str,
    arguments: &'a Value,
    request_id: Value,
}

pub(super) enum StreamTurn {
    Segment {
        segment: Segment,
        provider_settled: bool,
    },
    Disconnected,
}

impl Bridge {
    pub(super) async fn non_streaming_response(&self, turn: ActiveTurn) -> Result<Response<Body>> {
        let _gate = turn.gate;
        let segment = match self
            .wait_for_segment(
                &turn.session,
                &turn.events,
                turn.input_tokens,
                &turn.extras,
                None,
            )
            .await
        {
            Ok(segment) => segment,
            Err(error) => {
                self.remove_session(&turn.session).await;
                return Err(error);
            }
        };
        commit_transcript(&turn.session, turn.extras, &segment).await;
        Ok(anthropic_response(segment, &turn.response_model))
    }

    pub(super) fn streaming_messages(
        self: &Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
    ) -> Response<Body> {
        let (sender, receiver) = mpsc::channel(256);
        let response_model = self.request_model(&request);
        sender
            .try_send(Ok(Bytes::from(message_start(
                &response_model,
                input_tokens,
            ))))
            .expect("new streaming response channel has capacity");
        tokio::spawn(Arc::clone(self).drive_prepared_stream(request, input_tokens, effort, sender));
        sse_response(receiver)
    }

    async fn drive_prepared_stream(
        self: Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        sender: StreamSender,
    ) {
        let (turn, mut builder) = prepare_with_activity(
            self.prepare_turn(&request, input_tokens, effort),
            input_tokens,
            &sender,
            INITIAL_ACTIVITY_DELAY,
            ACTIVITY_KEEPALIVE_INTERVAL,
        )
        .await;
        match turn {
            Ok(Some(turn)) => self.drive_stream(turn, sender, builder).await,
            Ok(None) => {}
            Err(error) => {
                let _ = builder.close_open_blocks(Some(&sender)).await;
                send_stream_error(&sender, error).await;
            }
        }
    }

    async fn drive_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
    ) {
        let ActiveTurn {
            session,
            events,
            extras,
            gate,
            ..
        } = turn;
        let _gate = gate;
        match self
            .wait_for_stream_segment(&session, &events, &extras, &sender, builder)
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
            Ok(StreamTurn::Disconnected) => {}
            Err(error) => {
                self.remove_session(&session).await;
                send_stream_error(&sender, error).await;
            }
        }
    }

    async fn wait_for_stream_segment(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
        current_messages: &[Value],
        sender: &StreamSender,
        builder: SegmentBuilder,
    ) -> Result<StreamTurn> {
        self.wait_for_stream_segment_with_interval(
            session,
            events,
            current_messages,
            sender,
            builder,
            ACTIVITY_KEEPALIVE_INTERVAL,
        )
        .await
    }

    async fn wait_for_stream_segment_with_interval(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
        current_messages: &[Value],
        sender: &StreamSender,
        mut builder: SegmentBuilder,
        activity_interval: Duration,
    ) -> Result<StreamTurn> {
        // Claude Code's decoded-event idle watchdog is ~300s. Anthropic `ping`
        // frames only satisfy the ~180s raw-byte watchdog, so emit a content
        // delta after provider silence well under that ceiling.
        let mut activity_deadline = Box::pin(sleep(activity_interval));
        loop {
            let next = tokio::select! {
                biased;
                () = sender.closed() => {
                    return Ok(self.disconnect_stream(session, events).await);
                }
                () = &mut activity_deadline => {
                    refresh_activity_keepalive(
                        &mut builder,
                        sender,
                        activity_deadline.as_mut(),
                        activity_interval,
                    ).await?;
                    continue;
                }
                next = next_event(events, builder.has_external_tool_calls()) => next,
            };
            let event = match next {
                NextEvent::Event(event) => event,
                NextEvent::ExternalBatchReady => {
                    return self
                        .external_batch_segment(session, events, builder, sender)
                        .await;
                }
                NextEvent::Closed => bail!("app-server event stream closed"),
            };
            if builder
                .handle_event(self, session, current_messages, &event, Some(sender))
                .await?
                == ControlFlow::Break(())
            {
                return Ok(StreamTurn::Segment {
                    segment: builder.finish(Some(sender)).await?,
                    provider_settled: true,
                });
            }
        }
    }

    async fn finish_if_stream_closed(
        &self,
        sender: &StreamSender,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
        provider_settled: bool,
    ) -> bool {
        if !sender.is_closed() {
            return false;
        }
        self.finish_closed_stream(session, events, provider_settled)
            .await;
        true
    }

    async fn external_batch_segment(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
        builder: SegmentBuilder,
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

    async fn wait_for_segment(
        &self,
        session: &Session,
        events: &crate::app_server::ThreadEvents,
        input_tokens: u64,
        current_messages: &[Value],
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
                .handle_event(self, session, current_messages, &event, stream)
                .await?
                == ControlFlow::Break(())
            {
                return builder.finish(stream).await;
            }
        }
    }

    async fn spawn_internal_tool(
        &self,
        session: &Session,
        current_messages: &[Value],
        call: &ToolCall<'_>,
        model: &str,
    ) {
        let transcript = session.transcript.lock().await;
        let context = transcript
            .iter()
            .chain(current_messages)
            .cloned()
            .collect::<Vec<_>>();
        drop(transcript);
        let prompt = subscription_prompt(call.name, call.arguments, &context);
        let app = Arc::clone(&self.app);
        let model = model.to_owned();
        let program = self.subscription_program.clone();
        let subscription_slots = Arc::clone(&self.subscription_slots);
        let subscription_timeout = self.subscription_timeout;
        let request_id = call.request_id.clone();
        let parent_model = session.model.clone();
        tokio::spawn(async move {
            let options = SubscriptionOptions::internal(subscription_slots, subscription_timeout);
            let result = run_subscription_model(&program, &model, &prompt, options).await;
            let (text, success) = match result {
                Ok(text) => (text, true),
                Err(error) => (format!("Claude subscription call failed: {error:#}"), false),
            };
            let response = json!({
                "contentItems":[{"type":"inputText","text":text}],
                "success":success
            });
            if let Err(error) = app
                .respond_for_model(&parent_model, request_id, response)
                .await
            {
                tracing::error!(%error, "failed to return internal Claude tool result");
            }
        });
    }
}

async fn refresh_activity_keepalive(
    builder: &mut SegmentBuilder,
    sender: &StreamSender,
    mut deadline: std::pin::Pin<&mut tokio::time::Sleep>,
    interval: Duration,
) -> Result<()> {
    builder.activity_keepalive(Some(sender)).await?;
    deadline.as_mut().reset(Instant::now() + interval);
    Ok(())
}

pub(super) fn turn_flow(event: &Value) -> Result<ControlFlow<()>> {
    match event.pointer("/params/turn/status").and_then(Value::as_str) {
        Some("completed") | None => Ok(ControlFlow::Break(())),
        Some("inProgress") => Ok(ControlFlow::Continue(())),
        Some(status) => bail!("codex app-server turn ended with status {status}"),
    }
}

pub(super) fn error_flow(event: &Value) -> Result<ControlFlow<()>> {
    if event
        .pointer("/params/willRetry")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        tracing::warn!(
            error = %event.get("params").unwrap_or(event),
            "codex app-server is retrying the turn"
        );
        return Ok(ControlFlow::Continue(()));
    }
    bail!(
        "codex app-server turn failed: {}",
        event.get("params").unwrap_or(event)
    )
}

async fn commit_transcript(session: &Session, extras: Vec<Value>, segment: &Segment) {
    let mut transcript = session.transcript.lock().await;
    transcript.extend(extras);
    transcript.push(json!({"role":"assistant","content":segment.blocks}));
}

#[cfg(test)]
mod tests;
