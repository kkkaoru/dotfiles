//! Live provider progress for ACP tools that must not become `tool_use`.

use anyhow::Result;
use serde_json::{Value, json};

use super::SegmentBuilder;
use crate::anthropic::stream::{
    protocol::{StreamSender, send_stream_frame},
    sanitize::is_provider_status_line,
};

impl SegmentBuilder {
    /// Stream ACP/Cursor/Qwen tool progress without executable `tool_use`.
    ///
    /// SubAgent panels paint live thinking chrome and hide assistant text until
    /// end_turn. Qwen often emits `AgentMessageChunk` before `ToolCall`; the old
    /// path then appended ▶ to `text_delta`, so the panel stayed on
    /// "Thought for Xs" + spinner. Always paint ▶/✓/✗ as `thinking_delta`.
    /// `sanitize_committed_blocks` still strips those markers from the transcript.
    pub(in crate::anthropic::stream) async fn stream_progress_text(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        self.close_text_block(stream).await?;
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
        // ACP status lines (`…:status`) and Qwen/Cursor prose that already
        // contains ▶/✓ must ride thinking chrome. SubAgent TUI hides text_delta.
        let status_item = event
            .pointer("/params/itemId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.ends_with(":status"));
        if status_item
            || delta
                .lines()
                .any(|line| is_provider_status_line(line.trim()))
        {
            return self.stream_progress_text(delta, stream).await;
        }
        if self.is_subagent {
            // SubAgent TUI hides text_delta until end_turn. Cline/Qwen narrate
            // mid-turn in AgentMessage, so mirror into thinking chrome live and
            // flush the answer only when the turn completes.
            self.thinking
                .progress_status(&mut self.blocks, delta, stream)
                .await?;
            self.pending_answer.push_str(delta);
            return Ok(());
        }
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
