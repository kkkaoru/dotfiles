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
