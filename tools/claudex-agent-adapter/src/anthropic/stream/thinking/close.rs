use super::super::thinking_support::is_thinking_elapsed_tip;
use super::{StreamSender, ThinkingState, send_stream_frame};
use anyhow::Result;
use serde_json::{Value, json};

impl ThinkingState {
    pub(in crate::anthropic::stream) async fn close(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        blocks[open.index]["thinking"] = json!(open.text);
        blocks[open.index]["signature"] = json!(open.signature);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"signature_delta","signature":blocks[open.index]["signature"]}
            })
        })
        .await?;
        send_stream_frame(
            stream,
            "content_block_stop",
            || json!({"type":"content_block_stop","index":open.index}),
        )
        .await
    }

    /// Launch prose, ZWSP prime, or keepalive-only `▶ Thinking… · 0s` that
    /// CC 2.1 folds into Nucleating/Slithering. Close before the first visible
    /// chrome so ▶ tools and CoT open a new content_block index.
    pub(in crate::anthropic::stream) fn open_holds_collapsed_subagent_launch(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| is_collapsed_subagent_prime(&open.text))
    }

    /// ZWSP-only or launch prose. Keepalive may close this, but must not close
    /// an elapsed `▶ Thinking… · Ns` tip (that would spawn a new block every tick).
    pub(in crate::anthropic::stream) fn open_holds_zwsp_or_launch_prose(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| is_zwsp_or_launch_prose(&open.text))
    }

    /// Promote keepalive/progress signatures so sanitize keeps streamed CoT,
    /// then close. Codex `tool_use` must not ride an open thinking block.
    pub(in crate::anthropic::stream) async fn close_before_executable_tool_use(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.open.as_ref().is_some_and(open_is_keepalive_progress) {
            self.promote_keepalive_progress("subagent:reasoning");
        }
        self.close(blocks, stream).await
    }

    /// Replace live ▶ chrome with buffered CoT for the committed transcript.
    pub(in crate::anthropic::stream) async fn commit_buffered_reasoning(
        &mut self,
        blocks: &mut Vec<Value>,
        item_id: &str,
        text: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        if self.open.is_none() {
            self.start(blocks, item_id, 0, stream).await?;
        } else {
            self.promote_keepalive_progress(item_id);
        }
        let open = self.open.as_mut().expect("reasoning block just opened");
        open.text = text.to_owned();
        blocks[open.index]["thinking"] = json!(open.text);
        blocks[open.index]["signature"] = json!(open.signature);
        Ok(())
    }
}

fn open_is_keepalive_progress(open: &super::OpenThinking) -> bool {
    open.item_id == "claudex_provider_progress" || open.item_id == "claudex_activity_keepalive"
}

fn is_collapsed_subagent_prime(text: &str) -> bool {
    is_launch_prose(text) || is_zwsp_or_thinking_elapsed_only(text)
}

fn is_zwsp_or_launch_prose(text: &str) -> bool {
    is_launch_prose(text) || text.replace('\u{200b}', "").trim().is_empty()
}

fn is_launch_prose(text: &str) -> bool {
    text.contains("SubAgent starting")
        || text.contains("effort=")
        || text.contains("still thinking with high effort")
}

fn is_zwsp_or_thinking_elapsed_only(text: &str) -> bool {
    let visible = text.replace('\u{200b}', "");
    let lines: Vec<&str> = visible
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.is_empty() || lines.iter().copied().all(is_thinking_elapsed_tip)
}
