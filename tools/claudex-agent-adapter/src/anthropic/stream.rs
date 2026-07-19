use std::{ops::ControlFlow, sync::Arc};

use anyhow::{Context, Result, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{
    ActiveTurn, Bridge, Segment, Session,
    content::anthropic_response,
    stream_batch::{NextEvent, next_event},
    subscription::{SubscriptionOptions, run_subscription_model, subscription_prompt},
};
use crate::agent_backend::TurnCancellation;

mod builder;
mod protocol;
mod thinking;

use builder::{SegmentBuilder, parse_tool_call};

#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, send_stream_completion, send_stream_error, sse_response};
pub(super) use protocol::{message_start, send_stream_frame, streaming_sse_response};

struct ToolCall<'a> {
    call_id: &'a str,
    name: &'a str,
    arguments: &'a Value,
    request_id: Value,
}

enum StreamTurn {
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

    pub(super) fn streaming_response(self: &Arc<Self>, turn: ActiveTurn) -> Response<Body> {
        let (sender, receiver) = mpsc::channel(256);
        sender
            .try_send(Ok(Bytes::from(message_start(
                &turn.response_model,
                turn.input_tokens,
            ))))
            .expect("new streaming response channel has capacity");
        tokio::spawn(Arc::clone(self).drive_stream(turn, sender));
        sse_response(receiver)
    }

    async fn drive_stream(self: Arc<Self>, turn: ActiveTurn, sender: StreamSender) {
        let ActiveTurn {
            session,
            events,
            extras,
            input_tokens,
            gate,
            ..
        } = turn;
        let _gate = gate;
        match self
            .wait_for_stream_segment(&session, &events, input_tokens, &extras, &sender)
            .await
        {
            Ok(StreamTurn::Segment {
                segment,
                provider_settled,
            }) => {
                if sender.is_closed() {
                    self.finish_closed_stream(&session, &events, provider_settled)
                        .await;
                    return;
                }
                commit_transcript(&session, extras, &segment).await;
                send_stream_completion(&sender, &segment).await;
                if sender.is_closed() {
                    self.finish_closed_stream(&session, &events, provider_settled)
                        .await;
                }
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
        input_tokens: u64,
        current_messages: &[Value],
        sender: &StreamSender,
    ) -> Result<StreamTurn> {
        let mut builder = SegmentBuilder::new(input_tokens);
        loop {
            let next = tokio::select! {
                biased;
                () = sender.closed() => {
                    return Ok(self.disconnect_stream(session, events).await);
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

    async fn finish_closed_stream(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
        provider_settled: bool,
    ) {
        if provider_settled {
            self.remove_session(session).await;
        } else {
            self.disconnect_stream(session, events).await;
        }
    }

    async fn disconnect_stream(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
    ) -> StreamTurn {
        self.remove_session(session).await;
        match self.app.cancel_turn(&session.thread_id).await {
            Ok(TurnCancellation::Settled) => {}
            Ok(TurnCancellation::Unsupported) => {
                if let Err(error) = self.drain_disconnected_turn(session, events).await {
                    tracing::warn!(
                        %error,
                        thread_id = %session.thread_id,
                        "failed to drain disconnected non-cancellable turn"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    thread_id = %session.thread_id,
                    "failed to cancel disconnected streaming turn"
                );
            }
        }
        StreamTurn::Disconnected
    }

    async fn drain_disconnected_turn(
        &self,
        session: &Session,
        events: &crate::app_server::ThreadEvents,
    ) -> Result<()> {
        self.reject_pending_disconnected_tools(session).await?;
        loop {
            let event = events
                .recv()
                .await
                .context("app-server event stream closed while draining disconnected turn")?;
            match event.get("method").and_then(Value::as_str) {
                Some("item/tool/call") => {
                    let request_id = parse_tool_call(&event)?.request_id;
                    self.reject_disconnected_tool(session, request_id).await?;
                }
                Some("error") => {
                    let _ = error_flow(&event)?;
                }
                Some("turn/completed")
                    if turn_flow(&event)? == ControlFlow::Break(()) =>
                {
                    return Ok(());
                }
                _ => {}
            }
        }
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

    async fn reject_pending_disconnected_tools(&self, session: &Session) -> Result<()> {
        loop {
            let pending = session
                .pending_tools
                .lock()
                .await
                .iter()
                .next()
                .map(|(tool_use_id, request_id)| (tool_use_id.clone(), request_id.clone()));
            let Some((tool_use_id, request_id)) = pending else {
                break;
            };
            self.reject_disconnected_tool(session, request_id).await?;
            session.pending_tools.lock().await.remove(&tool_use_id);
            self.agent_efforts
                .remove_tool_results(std::iter::once(tool_use_id.as_str()));
        }
        *session
            .pending_since
            .lock()
            .expect("pending tool clock poisoned") = None;
        Ok(())
    }

    async fn reject_disconnected_tool(&self, session: &Session, request_id: Value) -> Result<()> {
        self.app
            .respond_for_model(
                &session.model,
                request_id,
                json!({
                    "contentItems":[{
                        "type":"inputText",
                        "text":"Claude Code disconnected before returning this tool result."
                    }],
                    "success":false
                }),
            )
            .await
            .context("failed to reject a tool call from a disconnected turn")
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
