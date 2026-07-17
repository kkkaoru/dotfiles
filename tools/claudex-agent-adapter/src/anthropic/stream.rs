use std::{ops::ControlFlow, sync::Arc};

use anyhow::{Context, Result, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    ActiveTurn, Bridge, Segment, Session, Usage,
    content::{anthropic_response, estimated_block_tokens, estimated_tokens},
    retention::record_pending_tool,
    stream_batch::{NextEvent, next_event},
    subscription::{SubscriptionOptions, run_subscription_model, subscription_prompt},
};

mod protocol;
mod thinking;

#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{
    StreamSender, send_stream_completion, send_stream_error, send_tool_use, sse_response,
};
pub(super) use protocol::{message_start, send_stream_frame};
use thinking::ThinkingState;

struct SegmentBuilder {
    blocks: Vec<Value>,
    thinking: ThinkingState,
    open_text_block: Option<(usize, String)>,
    external_tool_calls: usize,
    usage: Usage,
}

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
            let event = match next_event(events, builder.external_tool_calls > 0).await {
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
            if let Err(error) = app.respond(request_id, response).await {
                tracing::error!(%error, "failed to return internal Claude tool result");
            }
        });
    }
}

impl SegmentBuilder {
    fn new(input_tokens: u64) -> Self {
        Self {
            blocks: Vec::new(),
            thinking: ThinkingState::default(),
            open_text_block: None,
            external_tool_calls: 0,
            usage: Usage {
                input_tokens,
                ..Usage::default()
            },
        }
    }

    async fn handle_event(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<ControlFlow<()>> {
        if self.model_output_event(event, stream).await? {
            return Ok(ControlFlow::Continue(()));
        }
        match event.get("method").and_then(Value::as_str) {
            Some("item/tool/call") => {
                self.tool_call(bridge, session, current_messages, event, stream)
                    .await?;
            }
            Some("thread/tokenUsage/updated") => self.update_usage(event),
            Some("error") => return error_flow(event),
            Some("turn/completed") => return turn_flow(event),
            _ => {}
        }
        Ok(ControlFlow::Continue(()))
    }

    async fn model_output_event(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        match event.get("method").and_then(Value::as_str) {
            Some("item/agentMessage/delta") => self.text_delta(event, stream).await?,
            Some("item/reasoning/summaryTextDelta") => {
                self.thinking.delta(event, &mut self.blocks, stream).await?;
            }
            Some("item/reasoning/textDelta") => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    async fn text_delta(&mut self, event: &Value, stream: Option<&StreamSender>) -> Result<()> {
        let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) else {
            return Ok(());
        };
        if delta.is_empty() {
            return Ok(());
        }
        self.thinking.close(&mut self.blocks, stream).await?;
        let index = match &mut self.open_text_block {
            Some((index, text)) => {
                text.push_str(delta);
                *index
            }
            None => self.start_text_block(delta, stream).await?,
        };
        send_stream_frame(
            stream,
            "content_block_delta",
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{"type":"text_delta","text":delta}
            }),
        )
        .await
    }

    async fn start_text_block(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<usize> {
        let index = self.blocks.len();
        self.blocks.push(json!({"type":"text","text":""}));
        send_stream_frame(
            stream,
            "content_block_start",
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"text","text":""}
            }),
        )
        .await?;
        self.open_text_block = Some((index, delta.to_owned()));
        Ok(index)
    }

    async fn tool_call(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let call = parse_tool_call(event)?;
        if let Some(model) = session.internal_tools.get(call.name) {
            bridge
                .spawn_internal_tool(session, current_messages, &call, model)
                .await;
            return Ok(());
        }
        let original_name = session
            .external_tool_names
            .get(call.name)
            .map(String::as_str)
            .unwrap_or(call.name);
        self.external_tool_call(bridge, session, original_name, call, stream)
            .await
    }

    async fn external_tool_call(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        original_name: &str,
        call: ToolCall<'_>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let tool_use_id = format!("toolu_{}", Uuid::new_v4().simple());
        let (intent_arguments, claude_arguments) =
            super::agent_effort::prepare_arguments(original_name, &tool_use_id, call.arguments);
        if let Some(arguments) = intent_arguments.as_ref() {
            bridge.agent_efforts.record(
                session.client_user_id.as_deref(),
                original_name,
                tool_use_id.clone(),
                arguments,
            );
        }
        tracing::debug!(call_id = %call.call_id, %tool_use_id, "mapped app-server tool call");
        record_pending_tool(
            session,
            tool_use_id.clone(),
            call.request_id,
            std::time::Instant::now(),
        )
        .await;
        self.close_open_blocks(stream).await?;
        let block = json!({
            "type": "tool_use",
            "id": tool_use_id,
            "name": session.external_tool_names.get(call.name)
                .map(String::as_str).unwrap_or(call.name),
            "input": claude_arguments
        });
        let index = self.blocks.len();
        send_tool_use(stream, index, &block).await?;
        self.blocks.push(block);
        self.external_tool_calls += 1;
        Ok(())
    }

    async fn close_text_block(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        let Some((index, text)) = self.open_text_block.take() else {
            return Ok(());
        };
        self.blocks[index]["text"] = json!(text);
        send_stream_frame(
            stream,
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        )
        .await
    }

    async fn close_open_blocks(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        self.thinking.close(&mut self.blocks, stream).await?;
        self.close_text_block(stream).await
    }

    fn update_usage(&mut self, event: &Value) {
        self.usage.input_tokens = event
            .pointer("/params/tokenUsage/last/inputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(self.usage.input_tokens);
        if let Some(output_tokens) = event
            .pointer("/params/tokenUsage/last/outputTokens")
            .and_then(Value::as_u64)
        {
            let reasoning_tokens = event
                .pointer("/params/tokenUsage/last/reasoningOutputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            self.usage.output_tokens = output_tokens.saturating_add(reasoning_tokens);
        }
    }

    async fn finish(mut self, stream: Option<&StreamSender>) -> Result<Segment> {
        self.close_open_blocks(stream).await?;
        if self.usage.output_tokens == 0 {
            self.usage.output_tokens = self
                .blocks
                .iter()
                .map(|block| {
                    let thinking = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .map_or(0, estimated_tokens);
                    estimated_block_tokens(block).saturating_add(thinking)
                })
                .sum();
        }
        let stop_reason = if self.blocks.iter().any(|block| block["type"] == "tool_use") {
            "tool_use"
        } else {
            "end_turn"
        };
        Ok(Segment {
            blocks: self.blocks,
            stop_reason,
            usage: self.usage,
        })
    }
}

fn parse_tool_call(event: &Value) -> Result<ToolCall<'_>> {
    let params = event.get("params").context("tool call params missing")?;
    Ok(ToolCall {
        call_id: params
            .get("callId")
            .and_then(Value::as_str)
            .context("tool call callId missing")?,
        name: params
            .get("tool")
            .and_then(Value::as_str)
            .context("tool call name missing")?,
        arguments: params.get("arguments").unwrap_or(&Value::Null),
        request_id: event
            .get("id")
            .cloned()
            .context("tool request id missing")?,
    })
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
