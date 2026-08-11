//! Strip adapter-only WIP noise from committed assistant segments.

mod filler;
pub(in crate::anthropic::stream) use filler::{compact_live_prose, is_canned_worker_filler, strip_canned_preserving_structure, is_provider_status_line, latest_worker_status, strip_worker_status_lines};

use serde_json::{Value, json};

use crate::anthropic::Segment;

pub(in crate::anthropic::stream) const LIVE_PROSE_CHAR_LIMIT: usize = 280;

pub(super) const PREMATURE_STATUS_ONLY_NOTICE: &str = "Claudex: ACP worker ended with a status-only \
toolless reply. This is not a completed result. Reroute the same scope; do not treat the provider \
as exhausted.";

/// Events that count as decoded activity for Claude Code's idle watchdog.
pub(super) fn is_visible_activity_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some(
            "item/agentMessage/delta"
                | "item/reasoning/summaryTextDelta"
                | "item/reasoning/textDelta"
                | "item/tool/call"
                | "item/providerTool/call"
                | "item/providerTool/update"
        )
    )
}

/// Drop keepalive / provider-status pollution from the final message/transcript.
pub(super) fn sanitize_committed_blocks(blocks: &mut Vec<Value>) {
    const ZWSP: char = '\u{200b}';
    const STATUS: &str = "Claudex is still working; waiting for provider output\u{2026}";
    blocks.retain_mut(|block| match block.get("type").and_then(Value::as_str) {
        Some("text") => {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                let cleaned = text
                    .replace(ZWSP, "")
                    .lines()
                    .filter(|line| !is_live_chrome_line(line.trim()))
                    .collect::<Vec<_>>()
                    .join("\n");
                block["text"] = json!(cleaned);
            }
            true
        }
        Some("thinking") => {
            let signature = block.get("signature").and_then(Value::as_str).unwrap_or("");
            if signature == "claudex_provider_progress" || signature == "claudex_activity_keepalive"
            {
                return false;
            }
            let Some(thinking) = block.get("thinking").and_then(Value::as_str) else {
                return true;
            };
            let cleaned = thinking
                .replace(ZWSP, "")
                .replace(STATUS, "")
                .lines()
                .filter(|line| !is_live_chrome_line(line.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed = cleaned.trim();
            // Drop pure keepalive / provider-status thinking so transcript
            // history stays answer-focused like native Claude Code turns.
            if trimmed.is_empty() || is_provider_status_only(trimmed) {
                return false;
            }
            block["thinking"] = json!(cleaned);
            true
        }
        _ => true,
    });
}

fn is_provider_status_only(text: &str) -> bool {
    text.lines()
        .all(|line| line.trim().is_empty() || is_live_chrome_line(line.trim()))
}

fn is_live_chrome_line(line: &str) -> bool {
    is_provider_status_line(line) || is_canned_worker_filler(line)
}

pub(super) fn is_premature_worker_status_reply(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 160 {
        return false;
    }
    if is_provider_status_only(trimmed) {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("status:")
        || lower.contains("phase update")
        || lower.contains("short status")
        || lower.contains("each phase")
        || lower.starts_with("starting phase")
        || lower.starts_with("still working")
        || trimmed.contains("フェーズ")
        || trimmed.contains("ステータス")
}

fn segment_has_tool_payload(blocks: &[Value]) -> bool {
    blocks.iter().any(|block| {
        matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use")
        )
    })
}

fn segment_visible_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|&block| block.get("type").and_then(Value::as_str) == Some("text"))
        .map(|block| block.get("text").and_then(Value::as_str).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn rewrite_premature_status_only_segment(mut segment: Segment) -> Segment {
    if segment_has_tool_payload(&segment.blocks) {
        return segment;
    }
    if !is_premature_worker_status_reply(&segment_visible_text(&segment.blocks)) {
        return segment;
    }
    segment.blocks = vec![json!({
        "type": "text",
        "text": PREMATURE_STATUS_ONLY_NOTICE,
    })];
    segment
}

/// SubAgent live chrome: wrangler/JSON dumps freeze the viewer and inflate tokens.
pub(super) fn is_bulk_tool_dump(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.len() < 96 {
        return false;
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return true;
    }
    text.bytes()
        .filter(|byte| *byte == b'{' || *byte == b'}')
        .count()
        >= 6
}


#[cfg(test)]
#[path = "sanitize_tests.rs"]
mod tests;
