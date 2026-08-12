use anyhow::Result;
use serde_json::{Value, json};

use super::batch::estimated_output_tokens;
use super::{SegmentBuilder, batch, external_tool};
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
            return self
                .thinking
                .close_before_executable_tool_use(&mut self.blocks, stream)
                .await;
        }
        self.commit_pending_reasoning_for_transcript(stream).await?;
        self.close_open_blocks(stream).await
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
            self.usage.output_tokens = output_tokens.saturating_add(reasoning_tokens);
        }
    }

    pub(in crate::anthropic::stream) async fn finish(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<Segment> {
        self.report_incomplete_launches(stream).await?;
        self.report_no_subagent_action(stream).await?;
        let tool_handoff = self.is_subagent && self.external_tool_calls > 0;
        self.close_blocks_for_finish(tool_handoff, stream).await?;
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
