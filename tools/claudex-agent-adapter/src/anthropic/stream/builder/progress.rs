//! Live provider progress for ACP tools that must not become `tool_use`.

use anyhow::Result;
use serde_json::{Value, json};

use super::SegmentBuilder;
use super::progress_keepalive::{
    compact_keepalive_title, is_adapter_tool_marker, keepalive_elapsed_chrome,
    send_activity_heartbeat,
};
use crate::anthropic::stream::{
    protocol::{StreamSender, send_stream_frame},
    sanitize::{
        compact_live_prose, is_bulk_tool_dump, is_canned_worker_filler, is_provider_status_line,
        latest_worker_status, strip_canned_preserving_structure, strip_worker_status_lines,
    },
    thinking::summary_delta,
};

impl SegmentBuilder {
    /// Stream ACP tool progress as thinking chrome (not executable `tool_use`).
    /// SubAgent panels hide assistant text until end_turn; keep one thinking
    /// block open. Canned ●/still-working worker text is dropped.
    pub(in crate::anthropic::stream) async fn stream_progress_text(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        if self.is_command_code_subagent() && !is_adapter_tool_marker(delta) {
            return Ok(());
        }
        self.note_provider_turn_activity();
        self.close_text_block(stream).await?;
        if self.is_subagent {
            // Close launch prose before ▶ so tool chrome is not folded into Wandering.
            self.close_collapsed_launch_before_tool_marker(delta, stream)
                .await?;
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, delta, stream)
                .await;
        }
        self.thinking
            .progress_status(&mut self.blocks, delta, stream)
            .await
    }

    async fn close_collapsed_launch_before_tool_marker(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let should_close = is_adapter_tool_marker(delta)
            && self.thinking.open_holds_collapsed_subagent_launch();
        if should_close {
            self.thinking.close(&mut self.blocks, stream).await?;
        }
        Ok(())
    }

    pub(in crate::anthropic::stream) async fn text_delta(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) else {
            return Ok(());
        };
        if delta.is_empty() {
            return Ok(());
        }
        if let Some(remainder) = self
            .take_subagent_status_remainder(delta, stream)
            .await?
        {
            return self.stream_subagent_text_delta(&remainder, stream).await;
        }
        // ACP status lines (`…:status`) and Qwen/Cursor prose that already
        // contains ▶/✓ must ride thinking chrome. SubAgent TUI hides text_delta.
        // Mixed ▶ + answer chunks (Command Code) must not dump the answer.
        let status_item = event
            .pointer("/params/itemId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.ends_with(":status"));
        let non_empty: Vec<&str> = delta
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if !non_empty.is_empty() && non_empty.iter().all(|line| is_canned_worker_filler(line)) {
            return Ok(());
        }
        if status_item
            || (!non_empty.is_empty() && non_empty.iter().all(|line| is_provider_status_line(line)))
        {
            return self.stream_progress_text(delta, stream).await;
        }
        if self.is_subagent {
            return self.stream_subagent_text_delta(delta, stream).await;
        }
        self.stream_answer_delta(delta, stream).await
    }

    async fn take_subagent_status_remainder(
        &mut self,
        raw: &str,
        stream: Option<&StreamSender>,
    ) -> Result<Option<String>> {
        if !self.is_subagent {
            return Ok(None);
        }
        let Some(status) = latest_worker_status(raw) else {
            return Ok(None);
        };
        self.replace_live_worker_status(&status, stream).await?;
        let remainder = strip_worker_status_lines(raw);
        if remainder.trim().is_empty() {
            return Ok(Some(String::new()));
        }
        Ok(Some(remainder))
    }

    async fn stream_subagent_text_delta(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some((display, committed)) = self.filter_subagent_live_delta(delta) else {
            return Ok(());
        };
        let dump_hint = display.contains("large tool output omitted");
        // Including Command Code: streaming text_delta closes thinking and
        // collapses CC 2.1 to repeating "Thought for Xs".
        self.note_provider_turn_activity();
        self.thinking
            .progress_status_keep_open(&mut self.blocks, &display, stream)
            .await?;
        if !dump_hint {
            // Keep the full answer for flush; display chrome may be compacted.
            self.pending_answer.push_str(&committed);
        }
        Ok(())
    }

    pub(super) fn note_summarized_reasoning(&mut self, event: &Value) {
        let Some(item_id) = event.pointer("/params/itemId").and_then(Value::as_str) else {
            return;
        };
        if !self.summarized_reasoning_ids.iter().any(|id| id == item_id) {
            self.summarized_reasoning_ids.push(item_id.to_owned());
        }
    }

    pub(super) async fn raw_reasoning_delta(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(item_id) = event.pointer("/params/itemId").and_then(Value::as_str) else {
            return Ok(());
        };
        if self.summarized_reasoning_ids.iter().any(|id| id == item_id) {
            return Ok(());
        }
        // GPT/Codex SubAgents stream long raw CoT as `textDelta`. Dumping it
        // into live thinking buried ▶ Read/Bash and left Claude Code 2.1 on
        // "Thought for Xs" with no mid-turn body. Keep thinking free for tool
        // chrome, but paint a compact tip so long CoT silence is not blank.
        // textDelta still counts as watchdog activity. Main sessions still
        // surface textDelta as native Thinking.
        if self.is_subagent {
            self.note_provider_turn_activity();
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, "▶ Thinking…\n", stream)
                .await;
        }
        self.reasoning_delta(event, stream).await
    }

    pub(in crate::anthropic::stream) async fn reasoning_delta(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some((item_id, summary_index, raw)) = summary_delta(event) else {
            return Ok(());
        };
        if !self.is_subagent {
            return self.thinking.delta(event, &mut self.blocks, stream).await;
        }
        if let Some(remainder) = self.take_subagent_status_remainder(raw, stream).await? {
            return self
                .emit_filtered_subagent_reasoning(item_id, summary_index, &remainder, stream)
                .await;
        }
        self.emit_filtered_subagent_reasoning(item_id, summary_index, raw, stream)
            .await
    }

    async fn emit_filtered_subagent_reasoning(
        &mut self,
        item_id: &str,
        summary_index: i64,
        raw: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if raw.trim().is_empty() {
            return Ok(());
        }
        if let Some((display, _)) = self.filter_subagent_live_delta(raw) {
            self.emit_subagent_reasoning_delta(item_id, summary_index, &display, stream)
                .await?;
        }
        Ok(())
    }

    async fn emit_subagent_reasoning_delta(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.note_provider_turn_activity();
        if delta.contains("large tool output omitted") {
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, delta, stream)
                .await;
        }
        self.thinking
            .delta_text_coalesced(item_id, summary_index, delta, &mut self.blocks, stream)
            .await
    }

    async fn replace_live_worker_status(
        &mut self,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.note_provider_turn_activity();
        // SSE can only append, but keep the open thinking buffer to a single
        // Status line so mid-turn chrome / Thought-for length stay coherent.
        self.thinking
            .rewrite_open_text(&mut self.blocks, strip_worker_status_lines);
        self.thinking
            .progress_status_keep_open(&mut self.blocks, status, stream)
            .await
    }

    fn filter_subagent_live_delta(&mut self, delta: &str) -> Option<(String, String)> {
        if delta.trim().is_empty() {
            return None;
        }
        let committed = strip_canned_preserving_structure(delta);
        if committed.trim().is_empty() {
            return None;
        }
        if is_bulk_tool_dump(committed.trim()) {
            return self.bulk_dump_hint().map(|hint| (hint.clone(), hint));
        }
        // Live tip drops blank lines; pending_answer keeps paragraph breaks.
        let display_source = committed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Some((compact_live_prose(&display_source), committed))
    }

    fn bulk_dump_hint(&mut self) -> Option<String> {
        if self.bulk_dump_hinted {
            return None;
        }
        self.bulk_dump_hinted = true;
        Some("… large tool output omitted\n".to_owned())
    }

    async fn stream_answer_delta(
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

    async fn subagent_activity_keepalive(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let last_tool = self
            .provider_tool_calls
            .last()
            .map(|(_, title)| compact_keepalive_title(title));
        // Stream-only ZWSP keeps the decoded-event watchdog fed even when the
        // tip text would otherwise be unchanged for a fraction of a second.
        if self.thinking.is_open() {
            self.thinking
                .elapsed_keepalive(
                    &self.blocks,
                    self.turn_started_at.elapsed(),
                    last_tool.as_deref(),
                    stream,
                )
                .await?;
        }
        // Advancing clock keeps progress_status_keep_open from deduping the tip
        // into stream-only ZWSP. CC 2.1 otherwise freezes on the first ▶ line
        // for the whole Bash/CoT silence window.
        let tip = keepalive_elapsed_chrome(last_tool.as_deref(), self.turn_started_at.elapsed());
        if self.thinking.open_holds_collapsed_subagent_launch() {
            self.thinking.close(&mut self.blocks, stream).await?;
        }
        self.thinking
            .progress_status_keep_open(&mut self.blocks, &tip, stream)
            .await?;
        Ok(())
    }

    pub(in crate::anthropic::stream::builder) async fn flush_pending_answer(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.pending_answer.is_empty() {
            return Ok(());
        }
        let text = std::mem::take(&mut self.pending_answer);
        let index = self.start_text_block(&text, stream).await?;
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{"type":"text_delta","text":text}
            })
        })
        .await?;
        self.close_text_block(stream).await
    }
}
