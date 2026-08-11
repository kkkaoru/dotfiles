use anyhow::Result;
use serde_json::{Value, json};
use super::{
    StreamSender, send_stream_frame,
    thinking_support::{has_answer_text, thinking_signature},
};
pub(super) use super::thinking_support::{has_visible_output, summary_delta};
const HEARTBEAT: &str = "\u{200b}";
#[derive(Default)]
pub(super) struct ThinkingState {
    open: Option<OpenThinking>,
}
struct OpenThinking {
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
    pub(super) async fn close(
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
    pub(super) fn open_holds_collapsed_subagent_launch(&self) -> bool {
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

    pub(super) async fn progress_status(
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
    pub(super) async fn progress_status_keep_open(
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
    async fn progress_status_on(
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
        let status_trimmed = status.trim_end_matches(|c: char| c == '\u{200b}' || c.is_whitespace());
        let buffer_trimmed =
            open.text.trim_end_matches(|c: char| c == '\u{200b}' || c.is_whitespace());
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

    /// First heartbeat is visible; later ones are ZWSP. Pure keepalive thinking
    /// is stripped from the committed segment. Anthropic `ping` does not reset
    /// Claude Code's decoded-event idle timer (~600s).
    pub(super) async fn activity_status(
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

    pub(super) fn prime_silent_heartbeat(&mut self, blocks: &mut Vec<Value>) {
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

    pub(super) async fn activity_keepalive(
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
    pub(super) async fn elapsed_keepalive(
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

#[cfg(test)]
include!("thinking_tests.rs");
