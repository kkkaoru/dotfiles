use std::ops::ControlFlow;

use anyhow::Result;
use serde_json::{Value, json};

use super::{ToolCall, error_flow, turn_flow};
use crate::anthropic::{
    Bridge, Segment, Session, Usage, WebEvidenceSummary,
    content::{estimated_block_tokens, estimated_tokens},
};

use super::{
    protocol::{StreamSender, send_stream_frame},
    sanitize::sanitize_committed_blocks,
    thinking::ThinkingState,
};

mod batch;
mod external_tool;
mod provider_launch;
mod visibility;
#[path = "web_provenance.rs"]
mod web_provenance;

pub(super) use super::tool_call_parser::parse_tool_call;

pub(in crate::anthropic) struct SegmentBuilder {
    pub(super) blocks: Vec<Value>,
    pub(super) thinking: ThinkingState,
    pub(super) open_text_block: Option<(usize, String)>,
    external_tool_calls: usize,
    /// Provider call IDs already shown as progress text. ACP can report the same
    /// call first as ToolCall and again as a populated ToolCallUpdate.
    pub(super) provider_tool_calls: Vec<(String, String)>,
    /// Launch-shaped provider tools already bridged to Claude Code tool_use.
    /// Cursor may emit call → update → completed for the same callId.
    bridged_provider_launch_ids: Vec<String>,
    /// Cursor MCP launch callIds seen with empty args. Later generic
    /// `provider tool` updates for these ids still consult the MCP launch queue.
    mcp_provider_call_ids: Vec<String>,
    requires_verified_web_evidence: bool,
    /// Completed provider-native web calls whose provenance has already been
    /// counted. A provider may repeat its final ToolCallUpdate while reconnecting.
    verified_web_evidence_call_ids: Vec<String>,
    injected_output_tokens: u64,
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
            bridged_provider_launch_ids: Vec::new(),
            mcp_provider_call_ids: Vec::new(),
            requires_verified_web_evidence: false,
            verified_web_evidence_call_ids: Vec::new(),
            injected_output_tokens: 0,
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
        self.record_web_evidence_requirement(current_messages, system);
        if self.model_output_event(event, stream).await? {
            return Ok(ControlFlow::Continue(()));
        }
        match event.get("method").and_then(Value::as_str) {
            Some("item/tool/call") => {
                let call = parse_tool_call(event)?;
                self.tool_call(bridge, session, current_messages, system, call, stream)
                    .await?;
            }
            Some("item/providerTool/call") | Some("item/providerTool/update") => {
                self.provider_launch_event(
                    bridge,
                    session,
                    current_messages,
                    system,
                    event,
                    stream,
                )
                .await?;
            }
            Some("item/started") => {
                self.native_web_search_event(event, stream).await?;
            }
            Some("item/completed") => {
                self.native_web_search_event(event, stream).await?;
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
        // Commit them as visible text so Cursor/Grok SubAgent panels show progress
        // instead of a silent multi-minute spinner.
        if event
            .pointer("/params/itemId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.ends_with(":status"))
        {
            return self.stream_progress_text(delta, stream).await;
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
    pub(super) async fn subagent_start_status(
        &mut self,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.thinking
            .activity_status(&mut self.blocks, status, stream)
            .await
    }

    pub(super) async fn activity_keepalive(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        const HEARTBEAT: &str = "\u{200b}";
        if let Some((index, _)) = self.open_text_block {
            // Stream-only: do not mutate the committed text buffer.
            return Self::send_activity_heartbeat(stream, index, HEARTBEAT).await;
        }
        self.thinking
            .activity_keepalive(&mut self.blocks, stream)
            .await
    }

    async fn send_activity_heartbeat(
        stream: Option<&StreamSender>,
        index: usize,
        heartbeat: &str,
    ) -> Result<()> {
        send_stream_frame(stream, "content_block_delta", || {
            heartbeat_delta(index, heartbeat)
        })
        .await
    }

    /// Provider-native tool / plan progress as durable assistant text.
    ///
    /// Cursor and other ACP providers never surface native tools as Claude Code
    /// `tool_use` cards (double-execution risk). Progress must therefore appear
    /// as text: stream it live and keep it in the committed segment so SubAgent
    /// panels and transcripts show work after the first model sentence.
    pub(super) async fn stream_progress_text(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        // Close thinking first so progress is not buried under thought chrome and
        // so Claude Code's decoded-event stream stays on visible text deltas.
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
        call: ToolCall,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(original_name) =
            external_tool::requested_external_tool_name(&session.external_tool_names, &call.name)
        else {
            external_tool::reject_unrequested_tool(bridge, session, call).await?;
            return Ok(());
        };
        let context = external_tool::ExternalToolContext {
            bridge,
            session,
            current_messages,
            system,
            stream,
        };
        if let Some(original_name) = crate::anthropic::agent_batch::original_name(original_name) {
            return batch::dispatch(self, context, original_name, call).await;
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
        self.report_no_subagent_action(stream).await?;
        self.close_open_blocks(stream).await?;
        sanitize_committed_blocks(&mut self.blocks);
        let stop_reason = if self.external_tool_calls > 0 {
            "tool_use"
        } else {
            "end_turn"
        };
        let web_answer_replaced = self.gate_unverified_web_response(stop_reason);
        if web_answer_replaced || self.usage.output_tokens == 0 {
            self.usage.output_tokens = self.blocks.iter().map(estimated_output_tokens).sum();
        } else {
            let output_tokens = self.usage.output_tokens;
            self.usage.output_tokens = output_tokens.saturating_add(self.injected_output_tokens);
        }
        let blocks = std::mem::take(&mut self.blocks);
        Ok(Segment {
            blocks,
            stop_reason,
            usage: self.usage,
            web_evidence: WebEvidenceSummary::default(),
        }
        .with_web_evidence(WebEvidenceSummary::from_verified_count(
            self.verified_web_evidence_count(),
        )))
    }
}

fn heartbeat_delta(index: usize, heartbeat: &str) -> Value {
    json!({
        "type":"content_block_delta", "index":index,
        "delta":{"type":"text_delta","text":heartbeat}
    })
}

fn estimated_output_tokens(block: &Value) -> u64 {
    let thinking = block
        .get("thinking")
        .and_then(Value::as_str)
        .map_or(0, estimated_tokens);
    estimated_block_tokens(block).saturating_add(thinking)
}

fn ensure_background_batch_launch(arguments: &mut Value) {
    // A batch is the adapter's explicit parallel primitive. Normalize every
    // member to a background launch so one slow worker cannot hold the
    // Claude Code turn open, while leaving ordinary single Agent/Task calls
    // untouched.
    if let Some(arguments) = arguments.as_object_mut() {
        arguments.insert("run_in_background".to_owned(), Value::Bool(true));
    }
}

#[cfg(test)]
// Coverage gates measure production behavior; this inline test is excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
