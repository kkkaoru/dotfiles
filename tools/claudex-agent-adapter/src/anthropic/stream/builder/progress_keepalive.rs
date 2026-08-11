use anyhow::Result;
use serde_json::json;

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

pub(super) fn is_adapter_tool_marker(delta: &str) -> bool {
    delta.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('▶') || trimmed.starts_with('✓') || trimmed.starts_with('✗')
    }) && !delta.contains("Command Code")
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

pub(super) fn keepalive_elapsed_chrome(last_tool: Option<&str>, elapsed: std::time::Duration) -> String {
    let clock = format_keepalive_clock(elapsed);
    match last_tool.filter(|title| !title.is_empty()) {
        Some(title) => format!("▶ {title} · {clock}\n"),
        None => format!("▶ Thinking… · {clock}\n"),
    }
}

fn format_keepalive_clock(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}
