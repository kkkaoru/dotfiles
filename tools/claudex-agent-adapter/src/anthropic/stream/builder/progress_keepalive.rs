use anyhow::Result;
use serde_json::json;

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::{StreamSender, send_stream_frame};

pub(super) async fn send_activity_heartbeat(
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

pub(super) fn compact_keepalive_title(title: &str) -> String {
    let trimmed = title.trim();
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(48).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

pub(super) fn keepalive_elapsed_chrome(
    last_tool: Option<&str>,
    elapsed: std::time::Duration,
) -> Option<String> {
    let title = last_tool.filter(|title| !title.is_empty())?;
    if is_nested_thinking_title(title) {
        return None;
    }
    Some(format!(
        "▶ {title} · {clock}\n",
        clock = format_keepalive_clock(elapsed)
    ))
}

fn is_nested_thinking_title(title: &str) -> bool {
    let trimmed = title
        .trim()
        .trim_end_matches('…')
        .trim_end_matches('.')
        .trim();
    let lower = trimmed.to_ascii_lowercase();
    lower == "think" || lower.starts_with("thinking")
}

fn format_keepalive_clock(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

impl SegmentBuilder {
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
