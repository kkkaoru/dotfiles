use std::{ops::ControlFlow, sync::Arc};

use anyhow::{Result, bail};
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

mod builder;
mod protocol;
mod thinking;

use builder::SegmentBuilder;
#[cfg(test)]
use builder::parse_tool_call;

#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, send_stream_completion, send_stream_error, sse_response};
pub(super) use protocol::{message_start, send_stream_frame};

struct ToolCall<'a> {
    call_id: &'a str,
    name: &'a str,
    arguments: &'a Value,
    request_id: Value,
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
        let _gate = turn.gate;
        let result = self
            .wait_for_segment(
                &turn.session,
                &turn.events,
                turn.input_tokens,
                &turn.extras,
                Some(&sender),
            )
            .await;
        match result {
            Ok(segment) => {
                commit_transcript(&turn.session, turn.extras, &segment).await;
                send_stream_completion(&sender, &segment).await;
            }
            Err(error) => {
                self.remove_session(&turn.session).await;
                send_stream_error(&sender, error).await;
            }
        }
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
