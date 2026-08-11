use anyhow::Result;

use super::super::progress_keepalive::is_adapter_tool_marker;
use super::SegmentBuilder;
use crate::anthropic::stream::{
    protocol::StreamSender,
    sanitize::{
        compact_live_prose, is_bulk_tool_dump, is_provider_status_line, latest_worker_status,
        strip_canned_preserving_structure, strip_worker_status_lines,
    },
};

impl SegmentBuilder {
    pub(super) async fn take_subagent_status_remainder(
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

    pub(super) async fn stream_subagent_text_delta(
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
        if !dump_hint {
            // Keep the full answer for flush; display chrome may be compacted.
            self.pending_answer.push_str(&committed);
        }
        let live = if dump_hint {
            display
        } else if self.is_command_code_subagent() {
            // Command Code keeps answer chrome visible in thinking before tools.
            display
        } else if is_adapter_tool_marker(&display)
            || display
                .lines()
                .any(|line| is_provider_status_line(line.trim()))
        {
            display
        } else {
            // Full AgentMessage prose collapses Claude Code 2.1 SubAgent chrome
            // to Frolicking/Wandering and hides later ▶ Bash (same class as CoT
            // dump). Tip-only; body stays in pending_answer until finish.
            "▶ Working…\n".to_owned()
        };
        self.thinking
            .progress_status_keep_open(&mut self.blocks, &live, stream)
            .await
    }

    pub(super) async fn emit_filtered_subagent_reasoning(
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

    pub(super) async fn emit_subagent_reasoning_delta(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let _ = (item_id, summary_index);
        self.note_provider_turn_activity();
        if delta.contains("large tool output omitted") {
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, delta, stream)
                .await;
        }
        // ACP AgentThoughtChunk is mapped to summaryTextDelta. Dumping that CoT
        // into live thinking collapses Claude Code 2.1 SubAgent chrome to
        // Frolicking/Wandering and hides later ▶ Bash for long tools (same
        // failure mode as raw textDelta). Tip-only keeps tool chrome visible;
        // buffer CoT for the committed transcript at finish.
        self.pending_reasoning.push_str(delta);
        self.thinking
            .progress_status_keep_open(&mut self.blocks, "▶ Thinking…\n", stream)
            .await
    }

    pub(super) async fn replace_live_worker_status(
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

    pub(super) fn filter_subagent_live_delta(&mut self, delta: &str) -> Option<(String, String)> {
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

    pub(super) fn bulk_dump_hint(&mut self) -> Option<String> {
        if self.bulk_dump_hinted {
            return None;
        }
        self.bulk_dump_hinted = true;
        Some("… large tool output omitted\n".to_owned())
    }
}
