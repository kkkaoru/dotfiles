use anyhow::Result;
use serde_json::{Value, json};

use super::super::{StreamSender, send_stream_frame};
use super::ThinkingState;

impl ThinkingState {
    pub(in crate::anthropic::stream) async fn progress_status(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if status.is_empty() {
            return Ok(());
        }
        self.progress_status_on(blocks, status, false, stream).await
    }
    /// Same as [`progress_status`] but never closes an open thought first.
    pub(in crate::anthropic::stream) async fn progress_status_keep_open(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if status.is_empty() {
            return Ok(());
        }
        self.progress_status_on(blocks, status, true, stream).await
    }
    pub(super) async fn progress_status_on(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        keep_open: bool,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Closing blank CoT chrome left SubAgent TUI on "Thought for Xs".
        if !keep_open
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.item_id != "claudex_provider_progress")
        {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, "claudex_provider_progress", 0, stream)
                .await?;
        }
        let open = self.open.as_mut().expect("progress block just opened");
        // Dedupe Status/▶; strip keepalive ZWSP (not whitespace) so tips stick.
        let status_trimmed =
            status.trim_end_matches(|c: char| c == '\u{200b}' || c.is_whitespace());
        let buffer_trimmed = open
            .text
            .trim_end_matches(|c: char| c == '\u{200b}' || c.is_whitespace());
        if !status_trimmed.is_empty() && buffer_trimmed.ends_with(status_trimmed) {
            return Ok(());
        }
        open.text.push_str(status);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"thinking_delta","thinking":status}
            })
        })
        .await
    }
}
