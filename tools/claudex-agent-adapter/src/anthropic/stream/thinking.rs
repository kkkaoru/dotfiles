pub(super) use super::thinking_support::{has_visible_output, summary_delta};
use super::{
    StreamSender, send_stream_frame,
    thinking_support::{has_answer_text, thinking_signature},
};
use anyhow::Result;
use serde_json::{Value, json};
mod activity;
mod close;
pub(super) const HEARTBEAT: &str = "\u{200b}";
#[derive(Default)]
pub(super) struct ThinkingState {
    open: Option<OpenThinking>,
}
pub(super) struct OpenThinking {
    index: usize,
    item_id: String,
    summary_index: i64,
    signature: String,
    text: String,
}
impl ThinkingState {
    fn promote_keepalive_progress(&mut self, item_id: &str) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.item_id != "claudex_provider_progress"
            && open.item_id != "claudex_activity_keepalive"
        {
            return;
        }
        open.item_id = item_id.to_owned();
        open.signature = thinking_signature(item_id);
    }
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }
    pub(super) fn rewrite_open_text(
        &mut self,
        blocks: &mut [Value],
        rewrite: impl FnOnce(&str) -> String,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.text = rewrite(&open.text);
        blocks[open.index]["thinking"] = json!(open.text);
    }
    pub(super) async fn delta(
        &mut self,
        event: &Value,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some((item_id, summary_index, delta)) = summary_delta(event) else {
            return Ok(());
        };
        self.delta_text(item_id, summary_index, delta, blocks, stream)
            .await
    }
    pub(super) async fn delta_text(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.delta_text_on(item_id, summary_index, delta, false, blocks, stream)
            .await
    }
    /// SubAgent turns keep one native thinking block open so Claude Code's
    /// standard thinking chrome stays live. Closing on every ACP itemId /
    /// summaryIndex collapse the viewer to repeating "Thought for Xs".
    pub(super) async fn delta_text_coalesced(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.delta_text_on(item_id, summary_index, delta, true, blocks, stream)
            .await
    }
    async fn delta_text_on(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        coalesce: bool,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.trim().is_empty()
            || (!coalesce && has_visible_output(blocks))
            || (coalesce && has_answer_text(blocks))
        {
            return Ok(());
        }
        // Main: one thought per itemId. SubAgents coalesce after Read/Grep.
        let unit_changed = !coalesce
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.item_id != item_id || open.summary_index != summary_index);
        if unit_changed {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, item_id, summary_index, stream).await?;
        } else if coalesce {
            // Tool/prose may open progress chrome first. Promote it to native
            // thought so sanitize keeps the reasoning in the transcript.
            self.promote_keepalive_progress(item_id);
        }
        let open = self.open.as_mut().expect("thinking block just opened");
        open.summary_index = summary_index;
        open.text.push_str(delta);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"thinking_delta","thinking":delta}
            })
        })
        .await
    }
    async fn start(
        &mut self,
        blocks: &mut Vec<Value>,
        item_id: &str,
        summary_index: i64,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let index = blocks.len();
        blocks.push(json!({"type":"thinking","thinking":"","signature":""}));
        send_stream_frame(stream, "content_block_start", || {
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            })
        })
        .await?;
        self.open = Some(OpenThinking {
            index,
            item_id: item_id.to_owned(),
            summary_index,
            signature: thinking_signature(item_id),
            text: String::new(),
        });
        Ok(())
    }
}

#[path = "thinking_progress.rs"]
mod progress;

#[cfg(test)]
include!("thinking_tests.rs");
