use std::ops::ControlFlow;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{ToolCall, error_flow, turn_flow};
use crate::anthropic::{
    Bridge, Segment, Session, Usage,
    content::{estimated_block_tokens, estimated_tokens},
};

use super::{
    protocol::{StreamSender, send_stream_frame},
    sanitize::sanitize_committed_blocks,
    thinking::ThinkingState,
};

mod external_tool;

pub(super) use super::tool_call_parser::parse_tool_call;

pub(super) struct SegmentBuilder {
    pub(super) blocks: Vec<Value>,
    pub(super) thinking: ThinkingState,
    pub(super) open_text_block: Option<(usize, String)>,
    external_tool_calls: usize,
    /// Provider call IDs already shown as progress text. ACP can report the same
    /// call first as ToolCall and again as a populated ToolCallUpdate.
    pub(super) provider_tool_calls: Vec<(String, String)>,
    usage: Usage,
}

impl SegmentBuilder {
    pub(super) fn new(input_tokens: u64) -> Self {
        Self {
            blocks: Vec::new(),
            thinking: ThinkingState::default(),
            open_text_block: None,
            external_tool_calls: 0,
            provider_tool_calls: Vec::new(),
            usage: Usage {
                input_tokens,
                ..Usage::default()
            },
        }
    }

    pub(super) fn has_external_tool_calls(&self) -> bool {
        self.external_tool_calls > 0
    }

    pub(super) fn has_committed_output(&self) -> bool {
        if self
            .open_text_block
            .as_ref()
            .is_some_and(|(_, text)| !text.is_empty())
        {
            return true;
        }
        let mut blocks = self.blocks.clone();
        sanitize_committed_blocks(&mut blocks);
        !blocks.is_empty()
    }

    pub(super) async fn handle_event(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        system: &Value,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<ControlFlow<()>> {
        if self.model_output_event(event, stream).await? {
            return Ok(ControlFlow::Continue(()));
        }
        match event.get("method").and_then(Value::as_str) {
            Some("item/tool/call") => {
                let call = parse_tool_call(event)?;
                self.tool_call(bridge, session, current_messages, system, call, stream)
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
        // ACP status lines reuse agentMessage/delta with itemId "...:status".
        // Keep them live-only so the final answer matches Claude Code more closely.
        if event
            .pointer("/params/itemId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.ends_with(":status"))
        {
            return self.stream_ephemeral_status(delta, stream).await;
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
    ///
    /// Heartbeats are stream-only when assistant text is already open so the
    /// final answer/transcript never accumulates zero-width junk the way a
    /// wall-clock injector would. Otherwise use a disposable thinking block.
    pub(super) async fn activity_keepalive(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        const HEARTBEAT: &str = "\u{200b}";
        if let Some((index, _)) = self.open_text_block {
            // Stream-only: do not mutate the committed text buffer.
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

    /// Live-only provider progress (ACP tools). Streamed for WIP visibility but
    /// excluded from committed answer text so history matches Claude Code more
    /// closely (tool cards are not part of the assistant answer).
    pub(super) async fn stream_ephemeral_status(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        // Never splice status into open answer text — that scrambled token order
        // in Claude Code's log (▶ lines mid-sentence). After text has started,
        // drop chrome; the tool already ran on the provider.
        if self.open_text_block.is_some()
            || self.thinking.has_visible_answer(self.blocks.as_slice())
        {
            return Ok(());
        }
        self.thinking
            .status_progress(delta, &mut self.blocks, stream)
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
        system: &Value,
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
        let context = external_tool::ExternalToolContext {
            bridge,
            session,
            current_messages,
            system,
            stream,
        };
        if let Some(original_name) = crate::anthropic::agent_batch::original_name(original_name) {
            let tasks = call
                .arguments
                .get("tasks")
                .and_then(Value::as_array)
                .context("batch Agent tasks missing")?;
            let minimum_batch = crate::anthropic::agent_batch::minimum_batch_size();
            let maximum_batch = crate::anthropic::agent_batch::maximum_batch_size();
            if !(minimum_batch..=maximum_batch).contains(&tasks.len()) {
                anyhow::bail!(
                    "batch Agent tasks must contain between {minimum_batch} and {maximum_batch} launches"
                );
            }
            let run_in_background = tasks.iter().all(|arguments| {
                arguments
                    .get("run_in_background")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            });
            for (index, arguments) in tasks.iter().enumerate() {
                let mut nested_arguments = arguments.clone();
                normalize_batch_launch(&mut nested_arguments, run_in_background);
                let nested = ToolCall {
                    call_id: call.call_id,
                    name: call.name,
                    arguments: &nested_arguments,
                    request_id: crate::anthropic::agent_batch::pending_marker(
                        call.request_id.clone(),
                        index,
                        tasks.len(),
                    ),
                };
                self.external_tool_call(context, original_name, nested)
                    .await?;
            }
            return Ok(());
        }
        self.external_tool_call(context, original_name, call).await
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

    pub(super) async fn finish(&mut self, stream: Option<&StreamSender>) -> Result<Segment> {
        self.close_open_blocks(stream).await?;
        sanitize_committed_blocks(&mut self.blocks);
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
        let stop_reason = if self.external_tool_calls > 0 {
            "tool_use"
        } else {
            "end_turn"
        };
        let blocks = std::mem::take(&mut self.blocks);
        Ok(Segment {
            blocks,
            stop_reason,
            usage: self.usage,
        })
    }
}

fn normalize_batch_launch(arguments: &mut Value, run_in_background: bool) {
    // Keep a batch in one lifecycle mode. A caller that requires results in
    // the current answer marks any member foreground; all peers then remain
    // foreground so their completed results can be synthesized by the parent.
    if let Some(arguments) = arguments.as_object_mut() {
        arguments.insert(
            "run_in_background".to_owned(),
            Value::Bool(run_in_background),
        );
    }
}

#[cfg(test)]
// Coverage gates measure production behavior; this inline test is excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::normalize_batch_launch;

    #[test]
    fn leaves_non_object_batch_arguments_unchanged() {
        let mut arguments = json!(null);
        normalize_batch_launch(&mut arguments, true);
        assert_eq!(arguments, json!(null));
    }
}
