use anyhow::Result;
use serde_json::json;

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
        &self,
        _stream: Option<&StreamSender>,
    ) -> Result<()> {
        // `KeepaliveStream` sends SSE comments while the provider is quiet.
        // Never create/delta a text or thinking block for elapsed/status chrome.
        Ok(())
    }
}
