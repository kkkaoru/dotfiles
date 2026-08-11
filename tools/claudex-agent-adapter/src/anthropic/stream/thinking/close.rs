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

    /// Launch prose (`SubAgent starting` / `effort=`) that CC 2.1 folds away.
    pub(in crate::anthropic::stream) fn open_holds_collapsed_subagent_launch(&self) -> bool {
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        if open.item_id == "claudex_provider_progress" {
            return false;
        }
        let text = open.text.as_str();
        text.contains("SubAgent starting")
            || text.contains("effort=")
            || text.contains("still thinking with high effort")
    }

    /// Replace live ▶ tip chrome with buffered CoT for the committed transcript.
    /// Does not stream the CoT body — live SSE already showed tip-only progress.
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
