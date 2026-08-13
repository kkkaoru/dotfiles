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

#[cfg(test)]
mod tests {
    use super::{format_keepalive_clock, keepalive_elapsed_chrome};
    use std::time::Duration;

    #[test]
    fn nested_think_titles_are_not_elapsed_chrome() {
        assert_eq!(
            keepalive_elapsed_chrome(Some("think"), Duration::from_secs(5)),
            None
        );
        assert_eq!(
            keepalive_elapsed_chrome(Some("Thinking…"), Duration::from_secs(5)),
            None
        );
        assert_eq!(
            keepalive_elapsed_chrome(Some("THINK"), Duration::from_secs(12)),
            None
        );
    }

    #[test]
    fn formats_elapsed_clock_past_one_minute() {
        assert_eq!(format_keepalive_clock(Duration::from_secs(59)), "59s");
        assert_eq!(format_keepalive_clock(Duration::from_secs(60)), "1m00s");
        assert_eq!(format_keepalive_clock(Duration::from_secs(61)), "1m01s");
        let chrome = keepalive_elapsed_chrome(Some("Read"), Duration::from_secs(61))
            .expect("non-thinking tool titles paint elapsed chrome");
        assert!(chrome.contains("1m01s"), "{chrome}");
        assert!(chrome.contains("Read"), "{chrome}");
    }
}
