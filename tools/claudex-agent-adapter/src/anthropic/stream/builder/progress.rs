//! Live provider progress for ACP tools that must not become `tool_use`.

use anyhow::Result;
use serde_json::Value;

use super::SegmentBuilder;
use crate::anthropic::stream::{
    protocol::StreamSender,
    sanitize::{is_canned_worker_filler, is_provider_status_line},
    thinking::summary_delta,
};

#[path = "progress_activity.rs"]
mod progress_activity;
#[path = "progress_filter.rs"]
mod progress_filter;

impl SegmentBuilder {
    /// Stream ACP tool progress as thinking chrome (not executable `tool_use`).
    /// SubAgent panels hide assistant text until end_turn; keep one thinking
    /// block open. Canned ●/still-working worker text is dropped.
    pub(in crate::anthropic::stream) async fn stream_progress_text(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.is_empty() || self.is_command_code_subagent() {
            return Ok(());
        }
        self.note_provider_turn_activity();
        self.close_text_block(stream).await?;
        if self.is_subagent {
            self.close_collapsed_prime_before_visible(stream).await?;
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, delta, stream)
                .await;
        }
        self.thinking
            .progress_status(&mut self.blocks, delta, stream)
            .await
    }

    pub(super) async fn close_collapsed_prime_before_visible(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if !self.is_subagent || !self.thinking.open_holds_collapsed_subagent_launch() {
            return Ok(());
        }
        self.thinking.close(&mut self.blocks, stream).await
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
        if let Some(remainder) = self.take_subagent_status_remainder(delta, stream).await? {
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
        if self.is_subagent {
            return self.stream_subagent_raw_reasoning(event, stream).await;
        }
        self.reasoning_delta(event, stream).await
    }

    async fn stream_subagent_raw_reasoning(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.note_provider_turn_activity();
        let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) else {
            return Ok(());
        };
        if delta.trim().is_empty() {
            return Ok(());
        }
        self.pending_reasoning.push_str(delta);
        self.close_collapsed_prime_before_visible(stream).await?;
        self.thinking
            .progress_status_keep_open(&mut self.blocks, delta, stream)
            .await
    }

    pub(in crate::anthropic::stream) async fn reasoning_delta(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some((item_id, summary_index, raw)) = summary_delta(event) else {
            tracing::debug!(
                has_item_id = event
                    .pointer("/params/itemId")
                    .and_then(|value| value.as_str())
                    .is_some(),
                has_summary_index = event
                    .pointer("/params/summaryIndex")
                    .and_then(|value| value.as_u64())
                    .is_some(),
                has_delta = event
                    .pointer("/params/delta")
                    .and_then(|value| value.as_str())
                    .is_some(),
                "ignored malformed summarized reasoning delta"
            );
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
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "progress_activity_tests.rs"]
mod progress_activity_tests;
