use anyhow::Result;
use serde_json::{Value, json};

use super::super::thinking_support::{has_visible_output, thinking_signature};
use super::{HEARTBEAT, OpenThinking, ThinkingState};
use crate::anthropic::stream::{StreamSender, send_stream_frame};

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
        if status.is_empty() || has_visible_output(blocks) {
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
        blocks.push(json!({
            "type":"thinking",
            "thinking": HEARTBEAT,
            "signature":""
        }));
        self.open = Some(OpenThinking {
            index,
            item_id: "claudex_activity_keepalive".to_owned(),
            summary_index: 0,
            signature: thinking_signature("claudex_activity_keepalive"),
            text: HEARTBEAT.to_owned(),
        });
    }

    pub(in crate::anthropic::stream) async fn activity_keepalive(
        &mut self,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.emit_activity_heartbeat(blocks, stream).await
    }

    async fn emit_activity_heartbeat(
        &mut self,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Keep emitting decoded deltas after text/tool_use. Spark/Codex
        // SubAgents often go silent during native tools once a Bash/WebSearch
        // block exists; stopping heartbeats here used to trip Claude Code's
        // 600s "stream watchdog did not recover".
        const STATUS: &str = "Claudex is still working; waiting for provider output\u{2026}";
        if self.open.as_ref().is_some_and(|open| {
            open.item_id != "claudex_activity_keepalive"
                && open.item_id != "claudex_provider_progress"
                && open.text.trim().is_empty()
        }) {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, "claudex_activity_keepalive", 0, stream)
                .await?;
        }
        let open = self.open.as_mut().expect("thinking block just opened");
        let delta = if open.item_id == "claudex_activity_keepalive" && open.text.is_empty() {
            STATUS
        } else {
            HEARTBEAT
        };
        open.text.push_str(delta);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta",
                "index":open.index,
                "delta":{"type":"thinking_delta","thinking":delta}
            })
        })
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
