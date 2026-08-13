use anyhow::Result;
use serde_json::{Value, json};

use super::super::thinking_support::{has_visible_output, thinking_signature};
use super::{HEARTBEAT, OpenThinking, ThinkingState};
use crate::anthropic::stream::{StreamSender, send_stream_frame};

fn is_keepalive_item(item_id: &str) -> bool {
    item_id == "claudex_activity_keepalive" || item_id == "claudex_provider_progress"
}

fn is_silent_status_chrome(status: &str) -> bool {
    let trimmed = status.trim();
    let lower = trimmed.to_ascii_lowercase();
    trimmed.is_empty()
        || trimmed.contains('\u{200b}') && trimmed.replace('\u{200b}', "").trim().is_empty()
        || trimmed.starts_with("Claudex is still working")
        || trimmed.starts_with("Preparing provider session")
        || lower.starts_with("nucleating")
        || (trimmed.starts_with('▶') && lower.contains("thinking"))
}

impl ThinkingState {
    /// First heartbeat is visible; later ones are ZWSP. Pure keepalive thinking
    /// is stripped from the committed segment. Anthropic `ping` does not reset
    /// Claude Code's decoded-event idle timer (~600s).
    pub(in crate::anthropic::stream) async fn activity_status(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if status.is_empty() || has_visible_output(blocks) || is_silent_status_chrome(status) {
            return Ok(());
        }
        if self.open.is_none() {
            self.start(blocks, "claudex_activity_keepalive", 0, stream)
                .await?;
        }
        let open = self.open.as_mut().expect("activity block just opened");
        if open.item_id != "claudex_activity_keepalive" || !open.text.is_empty() {
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

    pub(in crate::anthropic::stream) fn prime_silent_heartbeat(&mut self, blocks: &mut Vec<Value>) {
        if self.open.is_some() {
            return;
        }
        let index = blocks.len();
        // First-flush start only. Do not park ZWSP/STATUS in the buffer.
        blocks.push(json!({
            "type":"thinking",
            "thinking": "",
            "signature":""
        }));
        self.open = Some(OpenThinking {
            index,
            item_id: "claudex_activity_keepalive".to_owned(),
            summary_index: 0,
            signature: thinking_signature("claudex_activity_keepalive"),
            text: String::new(),
        });
    }

    pub(in crate::anthropic::stream) fn holds_live_cot_or_tip(&self) -> bool {
        self.open.as_ref().is_some_and(|open| {
            let visible = open.text.replace('\u{200b}', "");
            let trimmed = visible.trim();
            !trimmed.is_empty()
                && (trimmed.contains('▶')
                    || trimmed.contains('✓')
                    || trimmed.contains('✗')
                    || trimmed.contains('🔎')
                    || !is_keepalive_item(&open.item_id))
        })
    }

    pub(in crate::anthropic::stream) async fn activity_keepalive(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.emit_activity_heartbeat(blocks, stream).await
    }

    async fn emit_activity_heartbeat(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.open.as_ref().is_some_and(|open| {
            !is_keepalive_item(&open.item_id) && open.text.replace('\u{200b}', "").trim().is_empty()
        }) {
            self.close(blocks, stream).await?;
        }
        // Never open thinking for silence. Stream-only ZWSP if a real block exists.
        if self.open.is_none() {
            return Ok(());
        }
        self.elapsed_keepalive(blocks, std::time::Duration::ZERO, None, stream)
            .await
    }

    /// Stream-only ZWSP; leave tip buffer unchanged so ▶ stays last-visible.
    pub(in crate::anthropic::stream) async fn elapsed_keepalive(
        &self,
        _blocks: &[Value],
        _elapsed: std::time::Duration,
        _last_tool: Option<&str>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(open) = self.open.as_ref() else {
            return Ok(());
        };
        let index = open.index;
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta",
                "index":index,
                "delta":{"type":"thinking_delta","thinking":HEARTBEAT}
            })
        })
        .await
    }
}
