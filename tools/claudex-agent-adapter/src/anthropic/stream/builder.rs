use std::ops::ControlFlow;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ToolCall, error_flow, turn_flow};
use crate::anthropic::{
    Bridge, Segment, Session, Usage,
    content::{estimated_block_tokens, estimated_tokens},
    retention::record_pending_tool,
};

use super::{
    protocol::{StreamSender, send_stream_frame, send_tool_use},
    thinking::ThinkingState,
};

pub(super) struct SegmentBuilder {
    pub(super) blocks: Vec<Value>,
    pub(super) thinking: ThinkingState,
    pub(super) open_text_block: Option<(usize, String)>,
    external_tool_calls: usize,
    /// Grok-ACP (or other provider-owned) tools shown as Claude tool cards
    /// without waiting for Claude Code to execute them.
    pub(super) provider_tool_ids: Vec<String>,
    usage: Usage,
}

impl SegmentBuilder {
    pub(super) fn new(input_tokens: u64) -> Self {
        Self {
            blocks: Vec::new(),
            thinking: ThinkingState::default(),
            open_text_block: None,
            external_tool_calls: 0,
            provider_tool_ids: Vec::new(),
            usage: Usage {
                input_tokens,
                ..Usage::default()
            },
        }
    }

    pub(super) fn has_external_tool_calls(&self) -> bool {
        self.external_tool_calls > 0
    }

    pub(super) async fn handle_event(
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
                let call = parse_tool_call(event)?;
                self.tool_call(bridge, session, current_messages, call, stream)
                    .await?;
            }
            Some("item/providerTool/call") => {
                self.provider_tool_call(event, stream).await?;
            }
            Some("item/providerTool/update") => {
                self.provider_tool_update(event, stream).await?;
            }
            Some("thread/tokenUsage/updated") => self.update_usage(event),
            Some("error") => return error_flow(event),
            Some("turn/completed") => return turn_flow(event),
            _ => {}
        }
        Ok(ControlFlow::Continue(()))
    }

    pub(super) async fn model_output_event(
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

    pub(super) async fn text_delta(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
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
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{"type":"text_delta","text":delta}
            })
        })
        .await
    }

    /// Keep Claude Code's decoded-event idle watchdog alive during provider
    /// silence (long Grok/Codex tool runs with no model tokens).
    pub(super) async fn activity_keepalive(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        const HEARTBEAT: &str = "\u{200b}";
        if let Some((index, text)) = &mut self.open_text_block {
            text.push_str(HEARTBEAT);
            let index = *index;
            return send_stream_frame(stream, "content_block_delta", || {
                json!({
                    "type":"content_block_delta", "index":index,
                    "delta":{"type":"text_delta","text":HEARTBEAT}
                })
            })
            .await;
        }
        self.thinking
            .activity_keepalive(&mut self.blocks, stream)
            .await
    }

    pub(super) async fn start_text_block(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<usize> {
        let index = self.blocks.len();
        self.blocks.push(json!({"type":"text","text":""}));
        send_stream_frame(stream, "content_block_start", || {
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"text","text":""}
            })
        })
        .await?;
        self.open_text_block = Some((index, delta.to_owned()));
        Ok(index)
    }

    async fn tool_call(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        call: ToolCall<'_>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
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
        self.external_tool_call(
            bridge,
            session,
            current_messages,
            original_name,
            call,
            stream,
        )
        .await
    }

    async fn external_tool_call(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        original_name: &str,
        call: ToolCall<'_>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let tool_use_id = format!("toolu_{}", Uuid::new_v4().simple());
        let (intent_arguments, claude_arguments) =
            crate::anthropic::agent_effort::prepare_arguments_for_user(
                original_name,
                &tool_use_id,
                call.arguments,
                current_messages,
            );
        if let Some(arguments) = intent_arguments.as_ref() {
            bridge.agent_efforts.record_from_user_messages(
                session.client_user_id.as_deref(),
                original_name,
                tool_use_id.clone(),
                &session.model,
                arguments,
                current_messages,
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
            || json!({"type":"content_block_stop","index":index}),
        )
        .await
    }

    pub(super) async fn close_open_blocks(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        self.thinking.close(&mut self.blocks, stream).await?;
        self.close_text_block(stream).await
    }

    pub(super) fn update_usage(&mut self, event: &Value) {
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

    pub(super) async fn finish(mut self, stream: Option<&StreamSender>) -> Result<Segment> {
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
        // Provider display tools are tool_use cards only — never ask Claude Code
        // to execute them. external_tool_calls tracks true Claude-owned tools.
        for block in &mut self.blocks {
            if let Some(object) = block.as_object_mut() {
                object.remove("_claudex_provider");
            }
        }
        let stop_reason = if self.external_tool_calls > 0 {
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

pub(super) fn parse_tool_call(event: &Value) -> Result<ToolCall<'_>> {
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
