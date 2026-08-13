use anyhow::Result;
use serde_json::json;

use super::super::progress_keepalive::{
    compact_keepalive_title, keepalive_elapsed_chrome, send_activity_heartbeat,
};
use super::SegmentBuilder;
use crate::anthropic::stream::{
    protocol::{StreamSender, send_stream_frame},
    sanitize::is_canned_worker_filler,
};

impl SegmentBuilder {
    pub(super) async fn stream_answer_delta(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if is_canned_worker_filler(delta) {
            return Ok(());
        }
        self.note_provider_turn_activity();
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

    pub(in crate::anthropic::stream) async fn subagent_start_status(
        &mut self,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.is_command_code_subagent() {
            return Ok(());
        }
        self.thinking
            .activity_status(&mut self.blocks, status, stream)
            .await
    }

    pub(in crate::anthropic::stream) async fn activity_keepalive(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.is_subagent {
            return self.subagent_activity_keepalive(stream).await;
        }
        const HEARTBEAT: &str = "\u{200b}";
        if let Some((index, _)) = self.open_text_block {
            // Stream-only: do not mutate the committed text buffer.
            return send_activity_heartbeat(stream, index, HEARTBEAT).await;
        }
        self.thinking
            .activity_keepalive(&mut self.blocks, stream)
            .await
    }

    async fn subagent_activity_keepalive(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        let last_tool = self
            .provider_tool_calls
            .last()
            .map(|(_, title)| compact_keepalive_title(title));
        let elapsed = self.turn_started_at.elapsed();
        let tip = keepalive_elapsed_chrome(last_tool.as_deref(), elapsed);
        if tip.is_none() {
            self.open_toolless_thinking(stream).await?;
        }
        if self.thinking.is_open() {
            self.thinking
                .elapsed_keepalive(&self.blocks, elapsed, last_tool.as_deref(), stream)
                .await?;
        }
        let Some(tip) = tip else {
            return Ok(());
        };
        self.paint_tool_elapsed_tip(&tip, stream).await
    }

    async fn open_toolless_thinking(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        if self.external_tool_calls > 0 || self.thinking.is_open() {
            return Ok(());
        }
        self.thinking.ensure_open(&mut self.blocks, stream).await
    }

    async fn paint_tool_elapsed_tip(
        &mut self,
        tip: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.external_tool_calls > 0 && !self.thinking.is_open() {
            return Ok(());
        }
        if self.thinking.open_holds_zwsp_or_launch_prose() {
            self.thinking.close(&mut self.blocks, stream).await?;
        }
        self.thinking
            .progress_status_keep_open(&mut self.blocks, tip, stream)
            .await
    }
}
