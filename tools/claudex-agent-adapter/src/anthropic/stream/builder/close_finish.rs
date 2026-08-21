use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::batch::estimated_output_tokens;
use super::{SegmentBuilder, batch, external_tool};

#[path = "unusable_tool.rs"]
mod unusable_tool;
use crate::anthropic::stream::{
    ToolCall,
    protocol::{StreamSender, send_stream_frame},
    sanitize::sanitize_committed_blocks,
};
use crate::anthropic::{Bridge, Segment, Session, WebEvidenceSummary};

impl SegmentBuilder {
    pub(in crate::anthropic::stream) async fn start_text_block(
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

    pub(in crate::anthropic::stream) async fn tool_call(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        system: &Value,
        call: ToolCall,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let context = external_tool::ExternalToolContext {
            bridge,
            session,
            current_messages,
            system,
            stream,
        };
        if self.skip_unusable_or_tripped_tool(context, &call).await? {
            return Ok(());
        }
        if self
            .reject_nested_subagent_launch(
                context,
                &call.name,
                &call.arguments,
                call.request_id.clone(),
            )
            .await?
        {
            return Ok(());
        }
        let Some(original_name) =
            external_tool::requested_external_tool_name(&session.external_tool_names, &call.name)
        else {
            external_tool::reject_unrequested_tool(bridge, session, call).await?;
            return Ok(());
        };
        if let Some(original_name) = crate::anthropic::agent_batch::original_name(original_name) {
            return batch::dispatch(self, context, original_name, call).await;
        }
        self.external_tool_call(context, original_name, call).await
    }

    pub(in crate::anthropic::stream::builder) async fn close_text_block(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
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

    pub(in crate::anthropic::stream) async fn close_open_blocks(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.thinking.close(&mut self.blocks, stream).await?;
        self.flush_pending_answer(stream).await?;
        self.close_text_block(stream).await
    }

    pub(super) async fn commit_pending_reasoning_for_transcript(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.pending_reasoning.trim().is_empty() {
            return Ok(());
        }
        let text = std::mem::take(&mut self.pending_reasoning);
        if !self.thinking.is_open() && thinking_contains_pending(&self.blocks, &text) {
            return Ok(());
        }
        self.thinking
            .commit_buffered_reasoning(&mut self.blocks, "subagent:reasoning", &text, stream)
            .await
    }

    async fn close_blocks_for_finish(
        &mut self,
        tool_handoff: bool,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if tool_handoff {
            self.commit_pending_reasoning_for_transcript(None).await?;
            self.flush_pending_answer(None).await?;
            self.close_text_block(stream).await?;
            self.stop_open_streaming_tool(stream).await?;
            return self
                .thinking
                .close_before_executable_tool_use(&mut self.blocks, stream)
                .await;
        }
        self.commit_pending_reasoning_for_transcript(stream).await?;
        self.stop_open_streaming_tool(stream).await?;
        self.close_open_blocks(stream).await
    }

    async fn stop_open_streaming_tool(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        self.stop_or_reject_open_streaming_tool(stream).await
    }

    pub(in crate::anthropic::stream) fn update_provider_stop_reason(
        &mut self,
        event: &Value,
    ) -> Result<()> {
        self.recoverable_empty_output = event
            .pointer("/params/turn/terminal/state")
            .and_then(Value::as_str)
            == Some("recoverable_error");
        let Some(reason) = event
            .pointer("/params/turn/providerStopReason")
            .and_then(Value::as_str)
        else {
            return Ok(());
        };
        self.provider_stop_reason = Some(match reason {
            "end_turn" => "end_turn",
            "max_tokens" => "max_tokens",
            "tool_use" => "tool_use",
            "pause_turn" => "pause_turn",
            other => bail!("unsupported provider stop reason `{other}`"),
        });
        Ok(())
    }

    pub(in crate::anthropic::stream) fn update_usage(&mut self, event: &Value) {
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
            self.usage.reasoning_output_tokens = reasoning_tokens;
            self.usage.output_tokens = output_tokens.saturating_add(reasoning_tokens);
        }
        self.usage.cache_read_input_tokens = event
            .pointer("/params/tokenUsage/last/cacheReadInputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(self.usage.cache_read_input_tokens);
        self.usage.cache_creation_input_tokens = event
            .pointer("/params/tokenUsage/last/cacheCreationInputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(self.usage.cache_creation_input_tokens);
        if let Some(one_hour) = event
            .pointer("/params/tokenUsage/last/cacheCreation1hInputTokens")
            .and_then(Value::as_u64)
        {
            self.usage.cache_creation_1h_input_tokens = Some(one_hour);
        }
    }

    pub(in crate::anthropic::stream) async fn finish(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<Segment> {
        self.report_incomplete_launches(stream).await?;
        if crate::anthropic::token_efficiency::circuit_is_open(self.consecutive_invalid_tool_json) {
            self.suppressed_tool_use = true;
        }
        let tool_handoff = self.is_subagent && self.external_tool_calls > 0;
        self.close_blocks_for_finish(tool_handoff, stream).await?;
        self.emit_unusable_tool_failure_text(stream).await?;
        self.report_no_subagent_action();
        let next_sse_index = self.blocks.len();
        self.blocks.retain(|block| {
            block.get("type").and_then(Value::as_str) != Some(super::SSE_INDEX_PAD)
        });
        sanitize_committed_blocks(&mut self.blocks);
        if self.recoverable_empty_output {
            remove_recoverable_terminal_output(&mut self.blocks);
        }
        let has_tool_calls = self.external_tool_calls > 0;
        if self.provider_stop_reason == Some("tool_use")
            && !has_tool_calls
            && !self.suppressed_tool_use
        {
            bail!("provider stopped for tool use without emitting a tool call");
        }
        if has_tool_calls
            && self
                .provider_stop_reason
                .is_some_and(|reason| reason != "tool_use")
        {
            bail!("provider emitted a tool call with a non-tool stop reason");
        }
        let stop_reason = if has_tool_calls {
            "tool_use"
        } else if self.suppressed_tool_use {
            // Blocked SubAgent notices are assistant text; keep the turn alive.
            "end_turn"
        } else {
            self.provider_stop_reason.unwrap_or("end_turn")
        };
        let web_answer_replaced = self.gate_unverified_web_response(stop_reason);
        if web_answer_replaced || self.usage.output_tokens == 0 {
            self.usage.output_tokens = self.blocks.iter().map(estimated_output_tokens).sum();
        } else {
            let output_tokens = self.usage.output_tokens;
            self.usage.output_tokens = output_tokens.saturating_add(self.injected_output_tokens);
        }
        self.last_turn_progress = self.snapshot_turn_progress();
        let blocks = std::mem::take(&mut self.blocks);
        Ok(Segment {
            blocks,
            stop_reason,
            usage: self.usage,
            web_evidence: WebEvidenceSummary::default(),
            next_sse_index,
        }
        .with_web_evidence(WebEvidenceSummary::from_verified_count(
            self.verified_web_evidence_count(),
        )))
    }

    async fn emit_unusable_tool_failure_text(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if !self.suppressed_tool_use || self.blocks.iter().any(block_has_visible_text) {
            return Ok(());
        }
        let notice = crate::anthropic::segment::UNUSABLE_TOOLS_SUBSTITUTE;
        let index = self.start_text_block(notice, stream).await?;
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta",
                "index":index,
                "delta":{"type":"text_delta","text":notice}
            })
        })
        .await?;
        self.close_text_block(stream).await
    }
}

fn block_has_visible_text(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.replace('\u{200b}', "").trim().is_empty())
}

fn remove_recoverable_terminal_output(blocks: &mut Vec<Value>) {
    blocks.retain(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("text" | "thinking")
        )
    });
}

fn thinking_contains_pending(blocks: &[Value], pending: &str) -> bool {
    let needle = pending.trim();
    if needle.is_empty() {
        return false;
    }
    blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("thinking")
            && block
                .get("thinking")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(needle))
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "close_finish_tests.rs"]
mod tests;
