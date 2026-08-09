//! Live provider progress for ACP tools that must not become `tool_use`.

use anyhow::Result;
use serde_json::{Value, json};

use super::SegmentBuilder;
use crate::anthropic::stream::{
    protocol::{StreamSender, send_stream_frame},
    sanitize::{
        compact_live_prose, is_bulk_tool_dump, is_canned_worker_filler, is_provider_status_line,
        latest_worker_status,
    },
    thinking::summary_delta,
};

impl SegmentBuilder {
    /// Stream ACP/Cursor/Qwen/Command Code tool progress without executable `tool_use`.
    ///
    /// SubAgent panels paint live thinking chrome and hide assistant text until
    /// end_turn. Qwen often emits `AgentMessageChunk` before `ToolCall`; the old
    /// path then appended ▶ to `text_delta`, so the panel stayed on
    /// "Thought for Xs" + spinner. Always paint ▶/✓/✗ as `thinking_delta`.
    /// ACP SubAgents keep one native thinking block open for the whole turn
    /// (Command Code still uses display-only `server_tool_use`). Canned ●/still-
    /// working worker text is still dropped. `sanitize_committed_blocks` strips
    /// markers from the transcript.
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
        self.close_text_block(stream).await?;
        if self.is_subagent && !self.is_command_code_subagent() {
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, delta, stream)
                .await;
        }
        self.thinking
            .progress_status(&mut self.blocks, delta, stream)
            .await
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
        if self.is_subagent {
            if let Some(status) = latest_worker_status(delta) {
                return self.replace_live_worker_status(&status, stream).await;
            }
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
            let Some(delta) = self.filter_subagent_live_delta(delta) else {
                return Ok(());
            };
            let dump_hint = delta.contains("large tool output omitted");
            if self.is_command_code_subagent() && self.paints_progress_as_text() && !dump_hint {
                // Command Code: server_tool_use unlocks live text_delta.
                return self.stream_answer_delta(&delta, stream).await;
            }
            // ACP SubAgents keep native thinking open for the whole turn.
            // Streaming text_delta here would close thinking and collapse CC 2.1
            // to repeating "Thought for Xs".
            self.thinking
                .progress_status_keep_open(&mut self.blocks, &delta, stream)
                .await?;
            if !dump_hint {
                self.pending_answer.push_str(&delta);
            }
            return Ok(());
        }
        self.stream_answer_delta(delta, stream).await
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
        if let Some(status) = latest_worker_status(raw) {
            return self.replace_live_worker_status(&status, stream).await;
        }
        match self.filter_subagent_live_delta(raw) {
            Some(delta) if delta.contains("large tool output omitted") => {
                if self.is_subagent && !self.is_command_code_subagent() {
                    self.thinking
                        .progress_status_keep_open(&mut self.blocks, &delta, stream)
                        .await?;
                } else {
                    if self.thinking.is_native_thought_open() {
                        self.thinking.close(&mut self.blocks, stream).await?;
                    }
                    self.thinking
                        .progress_status(&mut self.blocks, &delta, stream)
                        .await?;
                    self.paint_post_thought_status(stream).await?;
                }
            }
            Some(delta) => {
                let was_open = self.thinking.is_open();
                if self.is_subagent && !self.is_command_code_subagent() {
                    self.thinking
                        .delta_text_coalesced(
                            item_id,
                            summary_index,
                            &delta,
                            &mut self.blocks,
                            stream,
                        )
                        .await?;
                } else {
                    self.thinking
                        .delta_text(item_id, summary_index, &delta, &mut self.blocks, stream)
                        .await?;
                    if was_open && !self.thinking.is_open() {
                        self.paint_post_thought_status(stream).await?;
                    }
                }
            }
            None => {
                if self.thinking.is_native_thought_open()
                    && !(self.is_subagent && !self.is_command_code_subagent())
                {
                    self.thinking.close(&mut self.blocks, stream).await?;
                    self.paint_post_thought_status(stream).await?;
                }
            }
        }
        Ok(())
    }

    async fn replace_live_worker_status(
        &mut self,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.is_subagent && !self.is_command_code_subagent() {
            return self
                .thinking
                .progress_status_keep_open(&mut self.blocks, status, stream)
                .await;
        }
        if self.thinking.is_open() {
            self.thinking.close(&mut self.blocks, stream).await?;
        }
        self.thinking
            .progress_status(&mut self.blocks, status, stream)
            .await
    }

    async fn paint_post_thought_status(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        if let Some((_, title)) = self.provider_tool_calls.last() {
            let title = compact_keepalive_title(title);
            self.stream_progress_text(&format!("\n▶ {title}\n"), stream)
                .await?;
        }
        self.activity_keepalive(stream).await
    }

    fn filter_subagent_live_delta(&mut self, delta: &str) -> Option<String> {
        if delta.trim().is_empty() || is_canned_worker_filler(delta) {
            return None;
        }
        let stripped = delta
            .lines()
            .filter(|line| !is_canned_worker_filler(line))
            .collect::<Vec<_>>()
            .join("\n");
        if stripped.trim().is_empty() {
            return None;
        }
        if is_bulk_tool_dump(&stripped) {
            if self.bulk_dump_hinted {
                return None;
            }
            self.bulk_dump_hinted = true;
            return Some("… large tool output omitted\n".to_owned());
        }
        Some(compact_live_prose(&stripped))
    }

    async fn stream_answer_delta(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
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
            let last_tool = self
                .provider_tool_calls
                .last()
                .map(|(_, title)| compact_keepalive_title(title));
            if !self.thinking.is_open() {
                let resume = last_tool
                    .as_deref()
                    .filter(|title| !title.is_empty())
                    .map(|title| format!("▶ {title}\n"))
                    .unwrap_or_else(|| "\u{200b}".to_owned());
                self.thinking
                    .progress_status_keep_open(&mut self.blocks, &resume, stream)
                    .await?;
            }
            return self
                .thinking
                .elapsed_keepalive(
                    &mut self.blocks,
                    self.turn_started_at.elapsed(),
                    last_tool.as_deref(),
                    stream,
                )
                .await;
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

async fn send_activity_heartbeat(
    stream: Option<&StreamSender>,
    index: usize,
    heartbeat: &str,
) -> Result<()> {
    send_stream_frame(stream, "content_block_delta", || {
        json!({
            "type":"content_block_delta",
            "index":index,
            "delta":{"type":"text_delta","text":heartbeat}
        })
    })
    .await
}

fn is_adapter_tool_marker(delta: &str) -> bool {
    delta.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('▶') || trimmed.starts_with('✓') || trimmed.starts_with('✗')
    }) && !delta.contains("Command Code")
}

fn compact_keepalive_title(title: &str) -> String {
    let trimmed = title.trim();
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(48).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
